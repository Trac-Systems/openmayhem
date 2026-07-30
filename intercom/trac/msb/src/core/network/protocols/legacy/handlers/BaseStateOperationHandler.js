import b4a from 'b4a';
import {publicKeyToAddress} from "../../../../../utils/helpers.js";

class BaseStateOperationHandler {
    #state;
    #wallet;
    #rateLimiter;
    #txPoolService;
    #config;

    /**
     * @param {State} state
     * @param {IWallet} wallet
     * @param {TransactionRateLimiterService} rateLimiter
     * @param {TransactionPoolService} txPoolService
     * @param {Config} config
     **/
    constructor(state, wallet, rateLimiter, txPoolService, config) {
        if (new.target === BaseStateOperationHandler) {
            throw new Error('BaseStateOperationHandler is abstract and cannot be instantiated directly');
        }
        this.#state = state;
        this.#wallet = wallet;
        this.#rateLimiter = rateLimiter;
        this.#txPoolService = txPoolService;
        this.#config = config;
    }

    async validateBasicRequirements(payload, connection) {
        // Validate if operation can be processed:
        // - Non-writable nodes cannot process operations
        // - Regular indexers cannot process operations
        // - Admin-indexer can process operations only when network has less than maxWritersForAdminIndexerConnection writers
        const isAllowedToValidate = await this.#state.allowedToValidate(this.#wallet.address);
        const isAdminAllowedToValidate = await this.#state.isAdminAllowedToValidate();
        const canValidate = isAllowedToValidate || isAdminAllowedToValidate;
        if (!canValidate) {
            throw new Error('OperationHandler: State is not writable or is an indexer without admin privileges.');
        }

        this.#txPoolService.validateEnqueue();
        if (b4a.byteLength(JSON.stringify(payload)) > this.#config.transactionPoolSize) {
            throw new Error(
                `OperationHandler: Payload size exceeds maximum limit of ${this.#config.transactionPoolSize} bytes by ${b4a.byteLength(JSON.stringify(payload)) - this.#config.transactionPoolSize} bytes.`);
        }
        
        if (!this.#config.disableRateLimit) {
            const shouldDisconnect = this.#rateLimiter.legacyHandleRateLimit(connection);
            if (shouldDisconnect) {
                throw new Error(
                    `OperationHandler: Rate limit exceeded for ${publicKeyToAddress(connection.remotePublicKey, this.#config)}. Disconnecting...`
                );
            }
        }
    }

    async handle(payload, connection) {
        await this.validateBasicRequirements(payload, connection);
        await this.handleOperation(payload, connection);
    }

    enqueueTransaction(txHash, encodedOperation) {
        try {
            this.#txPoolService.addTransaction(txHash, encodedOperation);
        } catch (error) {
            throw new Error(
                `${this.constructor.name}: Failed to add transaction ${txHash} to transaction pool: ${error?.message ?? String(error)}`
            );
        }
    }

    async handleOperation(_payload, _connection) {
        throw new Error('handleOperation must be implemented by child class');
    }

}
export default BaseStateOperationHandler;

import b4a from 'b4a';
import {MAX_PARTIAL_TX_PAYLOAD_BYTE_SIZE, TRANSACTION_POOL_SIZE} from '../../../../../../utils/constants.js';

class BaseOperationHandler {
    #state;
    #wallet;
    #rateLimiter;
    #txPoolService;
    #config;

    /**
     * @param {State} state
     * @param {PeerWallet} wallet
     * @param {TransactionRateLimiterService} rateLimiter
     * @param {TransactionPoolService} txPoolService
     * @param {object} config
     **/
    constructor(state, wallet, rateLimiter, txPoolService, config) {
        if (new.target === BaseOperationHandler) {
            throw new Error('BaseOperationHandler is abstract and cannot be instantiated directly');
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
        // - Admin-indexer can process operations only when network has less than MAX_WRITERS_FOR_ADMIN_INDEXER_CONNECTION writers
        const isAllowedToValidate = await this.#state.allowedToValidate(this.#wallet.address);
        const isAdminAllowedToValidate = await this.#state.isAdminAllowedToValidate();
        const canValidate = isAllowedToValidate || isAdminAllowedToValidate;
        if (!canValidate) {
            throw new Error('OperationHandler: State is not writable or is an indexer without admin privileges.');
        }

        if (this.#txPoolService.tx_pool.length >= TRANSACTION_POOL_SIZE) {
            throw new Error("OperationHandler: Transaction pool is full, ignoring incoming transaction.");
        }

        if (b4a.byteLength(JSON.stringify(payload)) > MAX_PARTIAL_TX_PAYLOAD_BYTE_SIZE) {
            throw new Error(`OperationHandler: Payload size exceeds maximum limit of ${MAX_PARTIAL_TX_PAYLOAD_BYTE_SIZE} bytes by ${b4a.byteLength(JSON.stringify(payload)) - MAX_PARTIAL_TX_PAYLOAD_BYTE_SIZE} bytes.`);
        }
        
        if (!this.#config.disableRateLimit) {
            const shouldDisconnect = this.#rateLimiter.handleRateLimit(connection);
            if (shouldDisconnect) {
                throw new Error(`OperationHandler: Rate limit exceeded for peer ${b4a.toString(connection.remotePublicKey, 'hex')}. Disconnecting...`);
            }
        }
    }

    async handle(payload, connection) {
        await this.validateBasicRequirements(payload, connection);
        await this.handleOperation(payload, connection);
    }

    async handleOperation(payload, connection) {
        throw new Error('handleOperation must be implemented by child class');
    }

}
export default BaseOperationHandler;

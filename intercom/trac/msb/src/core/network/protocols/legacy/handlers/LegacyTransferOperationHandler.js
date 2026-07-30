import BaseStateOperationHandler from './BaseStateOperationHandler.js';
import {OperationType} from '../../../../../utils/constants.js';
import PartialTransferValidator from "../../shared/validators/PartialTransferValidator.js";
import {normalizeTransferOperation} from "../../../../../utils/normalizers.js"
import {applyStateMessageFactory} from "../../../../../messages/state/applyStateMessageFactory.js";
import {safeEncodeApplyOperation} from "../../../../../utils/protobuf/operationHelpers.js";
import b4a from "b4a";
import {publicKeyToAddress} from "../../../../../utils/helpers.js";

class LegacyTransferOperationHandler extends BaseStateOperationHandler {
    #partialTransferValidator;
    #config;
    #wallet;

    /**
     * @param {State} state
     * @param {IWallet} wallet
     * @param {TransactionRateLimiterService} rateLimiter
     * @param {TransactionPoolService} txPoolService
     * @param {Config} config
     **/
    constructor(state, wallet, rateLimiter, txPoolService, config) {
        super(state, wallet, rateLimiter, txPoolService, config);
        this.#config = config;
        this.#wallet = wallet;
        this.#partialTransferValidator = new PartialTransferValidator(state, this.#wallet.address, this.#config);
    }

    async handleOperation(payload, connection) {
        if (payload.type !== OperationType.TRANSFER) {
            throw new Error(
                `Unsupported operation type for LegacyTransferOperationHandler. Requested by ${publicKeyToAddress(connection.remotePublicKey, this.#config)}`
            );
        }
        await this.#handleTransfer(payload, connection);
    }

    async #handleTransfer(payload, connection) {
        const normalizedPayload = normalizeTransferOperation(payload, this.#config);
        const isValid = await this.#partialTransferValidator.validate(normalizedPayload);
        if (!isValid) {
            throw new Error(
                `TransferHandler: Transfer validation failed. Requested by ${publicKeyToAddress(connection.remotePublicKey, this.#config)}`
            );
        }

        const completeTransferOperation = await applyStateMessageFactory(this.#wallet, this.#config)
            .buildCompleteTransferOperationMessage(
                normalizedPayload.address,
                normalizedPayload.tro.tx,
                normalizedPayload.tro.txv,
                normalizedPayload.tro.in,
                normalizedPayload.tro.to,
                normalizedPayload.tro.am,
                normalizedPayload.tro.is
            )
        const encodedOperation = safeEncodeApplyOperation(completeTransferOperation);
        const txHash =  b4a.toString(normalizedPayload.tro.tx, 'hex');
        this.enqueueTransaction(txHash, encodedOperation);
    }
}

export default LegacyTransferOperationHandler;

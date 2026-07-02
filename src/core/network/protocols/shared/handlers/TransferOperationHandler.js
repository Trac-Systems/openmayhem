import BaseOperationHandler from './base/BaseOperationHandler.js';
import {OperationType} from '../../../../../utils/constants.js';
import PartialTransfer from "../validators/PartialTransfer.js";
import {normalizeTransferOperation} from "../../../../../utils/normalizers.js"
import {applyStateMessageFactory} from "../../../../../messages/state/applyStateMessageFactory.js";
import {safeEncodeApplyOperation} from "../../../../../utils/protobuf/operationHelpers.js";

class TransferOperationHandler extends BaseOperationHandler {
    #partialTransferValidator;
    #config;
    #wallet;
    #txPoolService;

    /**
     * @param {State} state
     * @param {PeerWallet} wallet
     * @param {TransactionRateLimiterService} rateLimiter
     * @param {object} config
     **/
    constructor(state, wallet, rateLimiter, txPoolService, config) {
        super(state, wallet, rateLimiter, txPoolService, config);
        this.#config = config;
        this.#wallet = wallet;
        this.#partialTransferValidator = new PartialTransfer(state, this.#wallet.address, this.#config);
        this.#txPoolService = txPoolService;
    }

    async handleOperation(payload, connection) {
        if (payload.type !== OperationType.TRANSFER) {
            throw new Error('Unsupported operation type for TransferOperationHandler');
        }
        await this.#handleTransfer(payload, connection);
    }

    async #handleTransfer(payload, connection) {
        const normalizedPayload = normalizeTransferOperation(payload, this.#config);
        const isValid = await this.#partialTransferValidator.validate(normalizedPayload);
        if (!isValid) {
            throw new Error("TransferHandler: Transfer validation failed.");
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

        this.#txPoolService.addTransaction(safeEncodeApplyOperation(completeTransferOperation));
    }
}

export default TransferOperationHandler;

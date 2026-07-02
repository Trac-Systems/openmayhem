import {OperationType} from '../../../../../utils/constants.js';
import PartialRoleAccess from "../validators/PartialRoleAccess.js";
import BaseOperationHandler from './base/BaseOperationHandler.js';
import {applyStateMessageFactory} from "../../../../../messages/state/applyStateMessageFactory.js";
import {safeEncodeApplyOperation} from "../../../../../utils/protobuf/operationHelpers.js";
import {normalizeRoleAccessOperation} from "../../../../../utils/normalizers.js";

class RoleOperationHandler extends BaseOperationHandler {
    #partialRoleAccessValidator;
    #wallet;
    #config;
    #txPoolService;

    /**
     * @param {State} state
     * @param {PeerWallet} wallet
     * @param {TransactionRateLimiterService} rateLimiter
     * @param {TransactionPoolService} txPoolService
     * @param {object} config
     **/
    constructor(state, wallet, rateLimiter, txPoolService ,config) {
        super(state, wallet, rateLimiter, txPoolService, config);
        this.#wallet = wallet;
        this.#config = config;
        this.#partialRoleAccessValidator = new PartialRoleAccess(state, this.#wallet.address ,this.#config)
        this.#txPoolService = txPoolService;
    }

    get partialRoleAccessValidator() {
        return this.#partialRoleAccessValidator;
    }

    async handleOperation(message, connection) {
        const normalizedPartialRoleAccessPayload = normalizeRoleAccessOperation(message, this.#config)
        const isValid = await this.partialRoleAccessValidator.validate(normalizedPartialRoleAccessPayload)
        let completePayload = null
        if (!isValid) {
            throw new Error("OperationHandler: partial role access payload validation failed.");
        }
        
        switch (normalizedPartialRoleAccessPayload.type) {
            case OperationType.ADD_WRITER:
                completePayload = await applyStateMessageFactory(this.#wallet, this.#config)
                    .buildCompleteAddWriterMessage(
                        normalizedPartialRoleAccessPayload.address,
                        normalizedPartialRoleAccessPayload.rao.tx,
                        normalizedPartialRoleAccessPayload.rao.txv,
                        normalizedPartialRoleAccessPayload.rao.iw,
                        normalizedPartialRoleAccessPayload.rao.in,
                        normalizedPartialRoleAccessPayload.rao.is,
                    )
                break;
            case OperationType.REMOVE_WRITER:
                completePayload = await applyStateMessageFactory(this.#wallet, this.#config)
                    .buildCompleteRemoveWriterMessage(
                        normalizedPartialRoleAccessPayload.address,
                        normalizedPartialRoleAccessPayload.rao.tx,
                        normalizedPartialRoleAccessPayload.rao.txv,
                        normalizedPartialRoleAccessPayload.rao.iw,
                        normalizedPartialRoleAccessPayload.rao.in,
                        normalizedPartialRoleAccessPayload.rao.is,
                    )
                break;
            case OperationType.ADMIN_RECOVERY:

                completePayload = await applyStateMessageFactory(this.#wallet, this.#config)
                    .buildCompleteAdminRecoveryMessage(
                        normalizedPartialRoleAccessPayload.address,
                        normalizedPartialRoleAccessPayload.rao.tx,
                        normalizedPartialRoleAccessPayload.rao.txv,
                        normalizedPartialRoleAccessPayload.rao.iw,
                        normalizedPartialRoleAccessPayload.rao.in,
                        normalizedPartialRoleAccessPayload.rao.is,
                    )
                break;
            default:
                throw new Error("OperationHandler: Assembling complete role access operation failed due to unsupported operation type.");
        }

        if (!completePayload) {
            throw new Error("OperationHandler: Assembling complete role access operation failed.");
        }

        this.#txPoolService.addTransaction(safeEncodeApplyOperation(completePayload))
    }
}

export default RoleOperationHandler;

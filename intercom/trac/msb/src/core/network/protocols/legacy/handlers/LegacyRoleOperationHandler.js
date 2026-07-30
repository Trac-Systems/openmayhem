import {OperationType} from '../../../../../utils/constants.js';
import PartialRoleAccessValidator from "../../shared/validators/PartialRoleAccessValidator.js";
import BaseStateOperationHandler from './BaseStateOperationHandler.js';
import {applyStateMessageFactory} from "../../../../../messages/state/applyStateMessageFactory.js";
import {safeEncodeApplyOperation} from "../../../../../utils/protobuf/operationHelpers.js";
import {normalizeRoleAccessOperation} from "../../../../../utils/normalizers.js";
import { publicKeyToAddress } from "../../../../../utils/helpers.js"
import b4a from "b4a";

class LegacyRoleOperationHandler extends BaseStateOperationHandler {
    #partialRoleAccessValidator;
    #wallet;
    #config;

    /**
     * @param {State} state
     * @param {IWallet} wallet
     * @param {TransactionRateLimiterService} rateLimiter
     * @param {TransactionPoolService} txPoolService
     * @param {Config} config
     **/
    constructor(state, wallet, rateLimiter, txPoolService, config) {
        super(state, wallet, rateLimiter, txPoolService, config);
        this.#wallet = wallet;
        this.#config = config;
        this.#partialRoleAccessValidator = new PartialRoleAccessValidator(state, this.#wallet.address ,this.#config)
    }

    get partialRoleAccessValidator() {
        return this.#partialRoleAccessValidator;
    }

    async handleOperation(message, connection) {
        const normalizedPartialRoleAccessPayload = normalizeRoleAccessOperation(message, this.#config)
        const isValid = await this.partialRoleAccessValidator.validate(normalizedPartialRoleAccessPayload)
        let completePayload = null
        if (!isValid) {
            throw new Error(
                `OperationHandler: partial role access payload validation failed. Requested by ${publicKeyToAddress(connection.remotePublicKey, this.#config)}`);
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
                throw new Error(
                    `OperationHandler: Assembling complete role access operation failed due to unsupported operation type. Requested by ${publicKeyToAddress(connection.remotePublicKey, this.#config)}`
                );
        }

        if (!completePayload) {
            throw new Error(
                `OperationHandler: Assembling complete role access operation failed. Requested by ${publicKeyToAddress(connection.remotePublicKey, this.#config)}`
            );
        }

        const encodedOperation = safeEncodeApplyOperation(completePayload);
        const txHash =  b4a.toString(completePayload.rao.tx, 'hex');
        this.enqueueTransaction(txHash, encodedOperation);
    }
}

export default LegacyRoleOperationHandler;

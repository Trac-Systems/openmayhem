import BaseStateOperationHandler from './BaseStateOperationHandler.js';
import {
    OperationType
} from '../../../../../utils/constants.js';
import PartialBootstrapDeploymentValidator from "../../shared/validators/PartialBootstrapDeploymentValidator.js";
import PartialTransactionValidator from "../../shared/validators/PartialTransactionValidator.js";
import {applyStateMessageFactory} from "../../../../../messages/state/applyStateMessageFactory.js";
import {safeEncodeApplyOperation} from "../../../../../utils/protobuf/operationHelpers.js";
import {
    normalizeBootstrapDeploymentOperation,
    normalizeTransactionOperation
} from "../../../../../utils/normalizers.js";
import b4a from "b4a";
import {publicKeyToAddress} from "../../../../../utils/helpers.js";


class LegacySubnetworkOperationHandler extends BaseStateOperationHandler {
    #partialBootstrapDeploymentValidator;
    #partialTransactionValidator;
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
        this.#wallet = wallet
        this.#partialBootstrapDeploymentValidator = new PartialBootstrapDeploymentValidator(state, this.#wallet.address, config);
        this.#partialTransactionValidator = new PartialTransactionValidator(state, this.#wallet.address, config);
    }

    async handleOperation(payload, connection) {
        if (payload.type === OperationType.TX) {
            await this.#partialTransactionSubHandler(payload, connection);
        } else if (payload.type === OperationType.BOOTSTRAP_DEPLOYMENT) {
            await this.#partialBootstrapDeploymentSubHandler(payload, connection);
        } else {
            throw new Error(
                `Unsupported operation type for LegacySubnetworkOperationHandler. Requested by ${publicKeyToAddress(connection.remotePublicKey, this.#config)} `
            );
        }
    }

    async #partialTransactionSubHandler(payload, connection) {
        const normalizedPayload = normalizeTransactionOperation(payload, this.#config);
        const isValid = await this.#partialTransactionValidator.validate(normalizedPayload);
        if (!isValid) {
            throw new Error(`
                SubnetworkHandler: Transaction validation failed. Requested by ${publicKeyToAddress(connection.remotePublicKey, this.#config)}`
            );
        }

        const completeTransactionOperation = await applyStateMessageFactory(this.#wallet, this.#config)
            .buildCompleteTransactionOperationMessage(
                normalizedPayload.address,
                normalizedPayload.txo.tx,
                normalizedPayload.txo.txv,
                normalizedPayload.txo.iw,
                normalizedPayload.txo.in,
                normalizedPayload.txo.ch,
                normalizedPayload.txo.is,
                normalizedPayload.txo.bs,
                normalizedPayload.txo.mbs
            )
        const encodedOperation = safeEncodeApplyOperation(completeTransactionOperation);
        const txHash = b4a.toString(normalizedPayload.txo.tx, 'hex');
        this.enqueueTransaction(txHash, encodedOperation);

    }

    async #partialBootstrapDeploymentSubHandler(payload, connection) {
        const normalizedPayload = normalizeBootstrapDeploymentOperation(payload, this.#config);
        const isValid = await this.#partialBootstrapDeploymentValidator.validate(normalizedPayload);
        if (!isValid) {
            throw new Error(
                `SubnetworkHandler: Bootstrap deployment validation failed. Requested by ${publicKeyToAddress(connection.remotePublicKey, this.#config)}`
            );
        }


        const completeBootstrapDeploymentOperation = await applyStateMessageFactory(this.#wallet, this.#config)
            .buildCompleteBootstrapDeploymentMessage(
                normalizedPayload.address,
                normalizedPayload.bdo.tx,
                normalizedPayload.bdo.txv,
                normalizedPayload.bdo.bs,
                normalizedPayload.bdo.ic,
                normalizedPayload.bdo.in,
                normalizedPayload.bdo.is
            )
        const encodedOperation = safeEncodeApplyOperation(completeBootstrapDeploymentOperation);
        const txHash = b4a.toString(normalizedPayload.bdo.tx, 'hex');
        this.enqueueTransaction(txHash, encodedOperation);

    }
}

export default LegacySubnetworkOperationHandler;

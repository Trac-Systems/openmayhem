import {networkMessageFactory} from "../../../../../messages/network/v1/networkMessageFactory.js";
import {
    NETWORK_CAPABILITIES,
    ResultCode
} from "../../../../../utils/constants.js";
import {
    getResultCode,
    V1ProtocolError
} from "../V1ProtocolError.js";
import V1BroadcastTransactionRequest from "../validators/V1BroadcastTransactionRequest.js";
import {
    unsafeDecodeApplyOperation,
    unsafeEncodeApplyOperation
} from "../../../../../utils/protobuf/operationHelpers.js";
import PartialRoleAccessValidator from "../../shared/validators/PartialRoleAccessValidator.js";
import PartialBootstrapDeploymentValidator from "../../shared/validators/PartialBootstrapDeploymentValidator.js";
import PartialTransactionValidator from "../../shared/validators/PartialTransactionValidator.js";
import PartialTransferValidator from "../../shared/validators/PartialTransferValidator.js";
import {applyStateMessageFactory} from "../../../../../messages/state/applyStateMessageFactory.js";
import V1BroadcastTransactionResponse from "../validators/V1BroadcastTransactionResponse.js";
import V1BaseOperationHandler from "./V1BaseOperationHandler.js";
import {
    createBroadcastTransactionOperationStrategies,
    resolveBroadcastTransactionOperationStrategy
} from "./broadcastTransaction/BroadcastTransactionOperationStrategies.js";
import {
    getTxHashFromDecodedTransaction,
    sanitizeDecodedPartialTransaction
} from "./broadcastTransaction/BroadcastTransactionPayloadUtils.js";
import {
    mapPendingCommitRegistrationError,
    mapPendingCommitResolutionError,
    mapTransactionEnqueueError,
    mapTxPoolAvailabilityError
} from "./broadcastTransaction/BroadcastTransactionErrorMapper.js";
import {shouldEndConnection} from "../../connectionPolicies.js";

class V1BroadcastTransactionOperationHandler extends V1BaseOperationHandler {
    #state;
    #wallet;
    #txPoolService;
    #broadcastTransactionRequestValidator;
    #broadcastTransactionResponseValidator;
    #partialRoleAccessValidator;
    #partialBootstrapDeploymentValidator;
    #partialTransactionValidator;
    #partialTransferValidator;
    #transactionCommitService;
    #operationStrategies;

    constructor(state, wallet, rateLimiterService, txPoolService, pendingRequestService, transactionCommitService, config) {
        super(rateLimiterService, pendingRequestService, config);
        this.#state = state;
        this.#wallet = wallet;
        this.#txPoolService = txPoolService;
        this.#broadcastTransactionRequestValidator = new V1BroadcastTransactionRequest(config);
        this.#partialRoleAccessValidator = new PartialRoleAccessValidator(state, this.#wallet.address, config);
        this.#partialBootstrapDeploymentValidator = new PartialBootstrapDeploymentValidator(state, this.#wallet.address, config);
        this.#partialTransactionValidator = new PartialTransactionValidator(state, this.#wallet.address, config);
        this.#partialTransferValidator = new PartialTransferValidator(state, this.#wallet.address, config);
        this.#broadcastTransactionResponseValidator = new V1BroadcastTransactionResponse(state, config);
        this.#transactionCommitService = transactionCommitService;
        this.#operationStrategies = createBroadcastTransactionOperationStrategies({
            partialRoleAccessValidator: this.#partialRoleAccessValidator,
            partialBootstrapDeploymentValidator: this.#partialBootstrapDeploymentValidator,
            partialTransactionValidator: this.#partialTransactionValidator,
            partialTransferValidator: this.#partialTransferValidator,
            createApplyStateMessageFactory: () => applyStateMessageFactory(this.#wallet, this.config)
        });
    }

    async handleRequest(message, connection) {
        const outcome = await this.#processBroadcastTransactionRequest(message, connection);
        await this.#sendBroadcastTransactionResponse(message.id, connection, outcome);
    }

    async handleResponse(message, connection) {
        try {
            this.applyRateLimit(connection);
            await this.resolvePendingResponse(
                message,
                connection,
                this.#broadcastTransactionResponseValidator,
                this.#extractBroadcastResultCode,
            );
        } catch (error) {
            this.handlePendingResponseError(
                message.id,
                connection,
                error,
                "failed to process broadcast transaction response from sender"
            );
        }
    }

    async #buildBroadcastTransactionResponse(id, capabilities, proof, timestamp = null, resultCode) {
        try {
            return await networkMessageFactory(this.#wallet, this.config).buildBroadcastTransactionResponse(
                id,
                capabilities,
                resultCode,
                proof,
                timestamp
            );
        } catch (error) {
            throw new V1ProtocolError(ResultCode.UNEXPECTED_ERROR, `Failed to build broadcast transaction response: ${error.message}`);
        }
    }

    decodeApplyOperation(message) {
        try {
            return unsafeDecodeApplyOperation(message);
        } catch (error) {
            throw new V1ProtocolError(ResultCode.UNEXPECTED_ERROR, `Failed to decode apply operation from message: ${error.message}`);
        }
    }

    async #processBroadcastTransactionRequest(message, connection) {
        let proof = null;
        let timestamp = 0;

        try {
            this.applyRateLimit(connection);
            await this.#broadcastTransactionRequestValidator.validate(message, connection.remotePublicKey);
            await this.#validateWriteCapability();
            this.#validateTxPoolAvailability();
            const decodedTransaction = this.decodeApplyOperation(message.broadcast_transaction_request.data);
            sanitizeDecodedPartialTransaction(decodedTransaction);
            const receipt = await this.dispatchTransaction(decodedTransaction);
            proof = receipt.proof;
            timestamp = receipt.timestamp;
        } catch (error) {
            return this.#buildFailedRequestOutcome(error, connection);
        }

        return {
            endConnection: false,
            proof,
            resultCode: ResultCode.OK,
            timestamp
        };
    }

    #buildFailedRequestOutcome(error, connection) {
        const protocolError = error instanceof V1ProtocolError
            ? error
            : new V1ProtocolError(ResultCode.UNEXPECTED_ERROR, error?.message ?? 'Unexpected error');
        const resultCode = getResultCode(protocolError);
        let timestamp = 0;

        if (
            resultCode === ResultCode.TX_ACCEPTED_PROOF_UNAVAILABLE &&
            Number.isSafeInteger(protocolError.timestamp) &&
            protocolError.timestamp > 0
        ) {
            timestamp = protocolError.timestamp;
        }

        this.displayError(
            "failed to process broadcast transaction request from sender",
            connection.remotePublicKey,
            protocolError
        );

        return {
            endConnection: shouldEndConnection(resultCode),
            proof: null,
            resultCode,
            timestamp
        };
    }

    async #sendBroadcastTransactionResponse(messageId, connection, {proof, timestamp, resultCode, endConnection}) {
        try {
            const response = await this.#buildBroadcastTransactionResponse(
                messageId,
                NETWORK_CAPABILITIES,
                proof,
                timestamp,
                resultCode,
            );

            await this.sendResponseAndMaybeClose(
                connection,
                response,
                endConnection
            );
        } catch (error) {
            this.displayError(
                "failed to build/send response to sender",
                connection.remotePublicKey,
                error
            );
            connection.end();
        }
    }

    #validateTxPoolAvailability() {
        try {
            this.#txPoolService.validateEnqueue();
        } catch (error) {
            throw mapTxPoolAvailabilityError(error);
        }
    }

    async #validateWriteCapability() {
        const isAllowedToValidate = await this.#state.allowedToValidate(this.#wallet.address);
        const isAdminAllowedToValidate = await this.#state.isAdminAllowedToValidate();
        const canValidate = isAllowedToValidate || isAdminAllowedToValidate;
        if (!canValidate) {
            throw new V1ProtocolError(
                ResultCode.NODE_HAS_NO_WRITE_ACCESS,
                'State is not writable or is an indexer without admin privileges.'
            );
        }
    }

    async dispatchTransaction(decodedTransaction) {
        if (!decodedTransaction || !Number.isInteger(decodedTransaction.type) || decodedTransaction.type === 0) {
            throw new V1ProtocolError(ResultCode.TX_INVALID_PAYLOAD, 'Decoded transaction type is missing.');
        }
        if (!this.#transactionCommitService) {
            throw new V1ProtocolError(ResultCode.UNEXPECTED_ERROR, 'TransactionCommitService is not configured.');
        }

        const operationStrategy = resolveBroadcastTransactionOperationStrategy(
            decodedTransaction.type,
            this.#operationStrategies
        );
        const completeTransactionOperation = await operationStrategy.build(decodedTransaction);
        const txHash = getTxHashFromDecodedTransaction(decodedTransaction, operationStrategy.payloadKey);
        const encodedCompleteTransaction = unsafeEncodeApplyOperation(completeTransactionOperation);
        const pendingCommit = this.#registerPendingCommit(txHash);

        this.#enqueueTransaction(txHash, encodedCompleteTransaction);
        return this.#resolvePendingCommit(pendingCommit);
    }

    #registerPendingCommit(txHash) {
        try {
            const pendingCommit = this.#transactionCommitService.registerPendingCommit(txHash);
            pendingCommit.catch(() => {});
            return pendingCommit;
        } catch (error) {
            throw mapPendingCommitRegistrationError(error);
        }
    }

    #enqueueTransaction(txHash, encodedCompleteTransaction) {
        try {
            this.#txPoolService.addTransaction(txHash, encodedCompleteTransaction);
        } catch (error) {
            const mappedError = mapTransactionEnqueueError(error);
            this.#transactionCommitService.rejectPendingCommit(txHash, mappedError);
            throw mappedError;
        }
    }

    async #resolvePendingCommit(pendingCommit) {
        try {
            return await pendingCommit;
        } catch (error) {
            throw mapPendingCommitResolutionError(error);
        }
    }

    #extractBroadcastResultCode(payload) {
        return payload.broadcast_transaction_response.result;
    }
}

export default V1BroadcastTransactionOperationHandler;

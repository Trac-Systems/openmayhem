import {ResultCode} from "../../../../../../utils/constants.js";
import {V1ProtocolError} from "../../V1ProtocolError.js";
import {
    TransactionPoolAlreadyQueuedError,
    TransactionPoolFullError,
    TransactionPoolInvalidIncomingDataError,
    TransactionPoolMissingCommitReceiptError,
    TransactionPoolProofUnavailableError
} from "../../../../services/TransactionPoolService.js";
import {
    PendingCommitAlreadyExistsError,
    PendingCommitBufferFullError,
    PendingCommitInvalidTxHashError,
    PendingCommitTimeoutError
} from "../../../../services/TransactionCommitService.js";

export function mapTxPoolAvailabilityError(error) {
    if (error instanceof TransactionPoolFullError) {
        return new V1ProtocolError(
            ResultCode.NODE_OVERLOADED,
            'Transaction pool is full, ignoring incoming transaction.'
        );
    }

    return error;
}

export function mapPendingCommitRegistrationError(error) {
    if (error instanceof PendingCommitInvalidTxHashError) {
        return new V1ProtocolError(ResultCode.TX_HASH_INVALID_FORMAT, error.message);
    }

    if (error instanceof PendingCommitAlreadyExistsError) {
        return new V1ProtocolError(ResultCode.TX_ALREADY_PENDING, error.message);
    }

    if (error instanceof PendingCommitBufferFullError) {
        return new V1ProtocolError(ResultCode.NODE_OVERLOADED, error.message);
    }

    return error;
}

export function mapTransactionEnqueueError(error) {
    if (error instanceof TransactionPoolFullError) {
        return new V1ProtocolError(ResultCode.NODE_OVERLOADED, error.message);
    }

    if (error instanceof TransactionPoolAlreadyQueuedError) {
        return new V1ProtocolError(ResultCode.TX_ALREADY_PENDING, error.message);
    }

    if (error instanceof TransactionPoolInvalidIncomingDataError) {
        return new V1ProtocolError(
            ResultCode.INTERNAL_ENQUEUE_VALIDATION_FAILED,
            `Internal enqueue validation failed: ${error.message}`
        );
    }

    return error;
}

export function mapPendingCommitResolutionError(error) {
    if (error instanceof TransactionPoolProofUnavailableError) {
        const protocolError = new V1ProtocolError(
            ResultCode.TX_ACCEPTED_PROOF_UNAVAILABLE,
            error.message
        );
        protocolError.timestamp = Number.isSafeInteger(error.timestamp) && error.timestamp > 0
            ? error.timestamp
            : 0;
        return protocolError;
    }

    if (error instanceof TransactionPoolMissingCommitReceiptError) {
        return new V1ProtocolError(ResultCode.TX_COMMITTED_RECEIPT_MISSING, error.message);
    }

    if (error instanceof PendingCommitTimeoutError) {
        return new V1ProtocolError(ResultCode.TIMEOUT, error.message);
    }

    return error;
}

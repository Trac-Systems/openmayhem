import test from 'brittle';

import {ResultCode} from '../../../../../src/utils/constants.js';
import {
    mapPendingCommitRegistrationError,
    mapPendingCommitResolutionError,
    mapTransactionEnqueueError,
    mapTxPoolAvailabilityError
} from '../../../../../src/core/network/protocols/v1/handlers/broadcastTransaction/BroadcastTransactionErrorMapper.js';
import {V1ProtocolError} from '../../../../../src/core/network/protocols/v1/V1ProtocolError.js';
import {
    TransactionPoolAlreadyQueuedError,
    TransactionPoolFullError,
    TransactionPoolInvalidIncomingDataError,
    TransactionPoolMissingCommitReceiptError,
    TransactionPoolProofUnavailableError
} from '../../../../../src/core/network/services/TransactionPoolService.js';
import {
    PendingCommitAlreadyExistsError,
    PendingCommitBufferFullError,
    PendingCommitInvalidTxHashError,
    PendingCommitTimeoutError
} from '../../../../../src/core/network/services/TransactionCommitService.js';

test('mapTxPoolAvailabilityError preserves the precheck overload mapping', t => {
    const mapped = mapTxPoolAvailabilityError(new TransactionPoolFullError(10));

    t.ok(mapped instanceof V1ProtocolError);
    t.is(mapped.resultCode, ResultCode.NODE_OVERLOADED);
    t.is(mapped.message, 'Transaction pool is full, ignoring incoming transaction.');
});

test('mapPendingCommitRegistrationError maps known registration failures', t => {
    const invalidTxHashError = mapPendingCommitRegistrationError(new PendingCommitInvalidTxHashError('abc'));
    t.ok(invalidTxHashError instanceof V1ProtocolError);
    t.is(invalidTxHashError.resultCode, ResultCode.TX_HASH_INVALID_FORMAT);

    const alreadyExistsError = mapPendingCommitRegistrationError(new PendingCommitAlreadyExistsError('abc'));
    t.ok(alreadyExistsError instanceof V1ProtocolError);
    t.is(alreadyExistsError.resultCode, ResultCode.TX_ALREADY_PENDING);

    const bufferFullError = mapPendingCommitRegistrationError(new PendingCommitBufferFullError(4));
    t.ok(bufferFullError instanceof V1ProtocolError);
    t.is(bufferFullError.resultCode, ResultCode.NODE_OVERLOADED);

    const unknownError = new Error('boom');
    t.is(mapPendingCommitRegistrationError(unknownError), unknownError);
});

test('mapTransactionEnqueueError maps known enqueue failures', t => {
    const overloadedError = mapTransactionEnqueueError(new TransactionPoolFullError(10));
    t.ok(overloadedError instanceof V1ProtocolError);
    t.is(overloadedError.resultCode, ResultCode.NODE_OVERLOADED);

    const alreadyQueuedError = mapTransactionEnqueueError(new TransactionPoolAlreadyQueuedError('abc'));
    t.ok(alreadyQueuedError instanceof V1ProtocolError);
    t.is(alreadyQueuedError.resultCode, ResultCode.TX_ALREADY_PENDING);

    const invalidIncomingDataError = mapTransactionEnqueueError(new TransactionPoolInvalidIncomingDataError('payload'));
    t.ok(invalidIncomingDataError instanceof V1ProtocolError);
    t.is(invalidIncomingDataError.resultCode, ResultCode.INTERNAL_ENQUEUE_VALIDATION_FAILED);
    t.is(invalidIncomingDataError.message, 'Internal enqueue validation failed: payload');

    const unknownError = new Error('boom');
    t.is(mapTransactionEnqueueError(unknownError), unknownError);
});

test('mapPendingCommitResolutionError maps receipt failures and preserves proof-unavailable timestamps', t => {
    const proofUnavailableError = mapPendingCommitResolutionError(
        new TransactionPoolProofUnavailableError('abc', 11, 'missing proof', 7)
    );
    t.ok(proofUnavailableError instanceof V1ProtocolError);
    t.is(proofUnavailableError.resultCode, ResultCode.TX_ACCEPTED_PROOF_UNAVAILABLE);
    t.is(proofUnavailableError.timestamp, 7);

    const missingReceiptError = mapPendingCommitResolutionError(
        new TransactionPoolMissingCommitReceiptError('abc')
    );
    t.ok(missingReceiptError instanceof V1ProtocolError);
    t.is(missingReceiptError.resultCode, ResultCode.TX_COMMITTED_RECEIPT_MISSING);

    const timeoutError = mapPendingCommitResolutionError(new PendingCommitTimeoutError('abc', 2000));
    t.ok(timeoutError instanceof V1ProtocolError);
    t.is(timeoutError.resultCode, ResultCode.TIMEOUT);

    const unknownError = new Error('boom');
    t.is(mapPendingCommitResolutionError(unknownError), unknownError);
});

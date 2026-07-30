import { test } from 'brittle';
import b4a from 'b4a';
import sinon from 'sinon';

import TransactionCommitService, {
    PendingCommitAlreadyExistsError,
    PendingCommitBufferFullError,
    PendingCommitCancelledError,
    PendingCommitInvalidTxHashError,
    PendingCommitTimeoutError,
    PendingCommitUnexpectedError
} from '../../../../src/core/network/services/TransactionCommitService.js';
import { TRANSACTION_COMMIT_SERVICE_BUFFER_SIZE } from '../../../../src/utils/constants.js';
import { errorMessageIncludes } from '../../../helpers/regexHelper.js';
import { config } from '../../../helpers/config.js';

function buildTxHashFromIndex(index) {
    const buffer = b4a.alloc(32);
    buffer.writeUInt32BE(index, 28);
    return buffer.toString('hex');
}

test('TransactionCommitService registers and resolves pending commit', async t => {
    const service = new TransactionCommitService(config);
    const txHash = buildTxHashFromIndex(1);
    const receipt = {
        txHash,
        blockNumber: 123,
        proof: b4a.from('abcd', 'hex')
    };

    const promise = service.registerPendingCommit(txHash);
    t.ok(service.has(txHash));

    t.ok(service.resolvePendingCommit(txHash, receipt));
    t.is(service.has(txHash), false);
    t.alike(await promise, receipt);

    t.is(service.resolvePendingCommit(txHash), false);
});

test('TransactionCommitService rejects pending commit with provided Error', async t => {
    const service = new TransactionCommitService(config);
    const txHash = buildTxHashFromIndex(2);
    const promise = service.registerPendingCommit(txHash);
    const expectedError = new Error('commit failed');

    t.ok(service.rejectPendingCommit(txHash, expectedError));
    t.is(service.has(txHash), false);

    try {
        await promise;
        t.fail('Expected pending commit promise to reject');
    } catch (error) {
        t.is(error, expectedError);
    }

    t.is(service.rejectPendingCommit(txHash, new Error('missing')), false);
});

test('TransactionCommitService rejects pending commit with fallback unexpected error', async t => {
    const service = new TransactionCommitService(config);
    const txHash = buildTxHashFromIndex(3);
    const promise = service.registerPendingCommit(txHash);

    t.ok(service.rejectPendingCommit(txHash, { reason: 'not-error-instance' }));

    try {
        await promise;
        t.fail('Expected pending commit promise to reject');
    } catch (error) {
        t.ok(error instanceof PendingCommitUnexpectedError);
        t.is(error.message, 'Unexpected commit error');
    }
});

test('TransactionCommitService throws on duplicate txHash', async t => {
    const service = new TransactionCommitService(config);
    const txHash = buildTxHashFromIndex(4);
    const promise = service.registerPendingCommit(txHash);

    t.exception(
        () => service.registerPendingCommit(txHash),
        PendingCommitAlreadyExistsError
    );

    t.ok(service.resolvePendingCommit(txHash));
    await promise;
});

test('TransactionCommitService validates txHash in all public methods', t => {
    const service = new TransactionCommitService(config);
    const invalidTxHashes = [
        undefined,
        null,
        '',
        'abc',
        'g'.repeat(64),
        'a'.repeat(62),
        'a'.repeat(66),
        b4a.alloc(32)
    ];

    for (const invalidTxHash of invalidTxHashes) {
        t.exception(
            () => service.has(invalidTxHash),
            PendingCommitInvalidTxHashError
        );
        t.exception(
            () => service.registerPendingCommit(invalidTxHash),
            PendingCommitInvalidTxHashError
        );
        t.exception(
            () => service.getAndDeletePendingCommit(invalidTxHash),
            PendingCommitInvalidTxHashError
        );
        t.exception(
            () => service.resolvePendingCommit(invalidTxHash),
            PendingCommitInvalidTxHashError
        );
        t.exception(
            () => service.rejectPendingCommit(invalidTxHash, new Error('invalid')),
            PendingCommitInvalidTxHashError
        );
    }
});

test('TransactionCommitService returns null/false for missing valid txHash', t => {
    const service = new TransactionCommitService(config);
    const missingTxHash = buildTxHashFromIndex(5);

    t.is(service.has(missingTxHash), false);
    t.is(service.getAndDeletePendingCommit(missingTxHash), null);
    t.is(service.resolvePendingCommit(missingTxHash), false);
    t.is(service.rejectPendingCommit(missingTxHash, new Error('missing')), false);
});

test('TransactionCommitService getAndDeletePendingCommit clears pending entry', async t => {
    const clock = sinon.useFakeTimers({ now: 1 });
    try {
        const service = new TransactionCommitService(config);
        const txHash = buildTxHashFromIndex(6);
        const promise = service.registerPendingCommit(txHash);
        const entry = service.getAndDeletePendingCommit(txHash);

        t.ok(entry);
        t.is(entry.txHash, txHash);
        t.is(service.has(txHash), false);
        t.is(service.getAndDeletePendingCommit(txHash), null);

        let settled = false;
        promise.then(
            () => {
                settled = true;
            },
            () => {
                settled = true;
            }
        );

        await clock.runAllAsync();
        await Promise.resolve();
        t.is(settled, false);

        entry.resolve('manual-resolution');
        t.is(await promise, 'manual-resolution');
    } finally {
        clock.restore();
        sinon.restore();
    }
});

test('TransactionCommitService rejects pending commit on timeout', async t => {
    const clock = sinon.useFakeTimers({ now: 1 });
    try {
        const timeoutMs = config.txCommitTimeout;
        const service = new TransactionCommitService(config);
        const txHash = buildTxHashFromIndex(7);
        const promise = service.registerPendingCommit(txHash);
        promise.catch(() => {});

        t.ok(service.has(txHash));

        await clock.runAllAsync();

        try {
            await promise;
            t.fail('Expected pending commit to time out');
        } catch (error) {
            t.ok(error instanceof PendingCommitTimeoutError);
            t.ok(error.message.includes(`timed out after ${timeoutMs} ms`));
        }

        t.is(service.has(txHash), false);
    } finally {
        clock.restore();
        sinon.restore();
    }
});

test('TransactionCommitService enforces pending commit buffer size limit', async t => {
    const service = new TransactionCommitService(config);
    const promises = [];

    for (let i = 0; i < TRANSACTION_COMMIT_SERVICE_BUFFER_SIZE; i++) {
        const txHash = buildTxHashFromIndex(10_000 + i);
        const promise = service.registerPendingCommit(txHash);
        promise.catch(() => {});
        promises.push(promise);
    }

    t.exception(
        () => service.registerPendingCommit(buildTxHashFromIndex(10_000 + TRANSACTION_COMMIT_SERVICE_BUFFER_SIZE)),
        errorMessageIncludes(`limit=${TRANSACTION_COMMIT_SERVICE_BUFFER_SIZE}`)
    );
    t.exception(
        () => service.registerPendingCommit(buildTxHashFromIndex(20_000)),
        PendingCommitBufferFullError
    );

    service.close();
    const results = await Promise.allSettled(promises);
    t.is(results.length, TRANSACTION_COMMIT_SERVICE_BUFFER_SIZE);
});

test('TransactionCommitService.close rejects all pending commits', async t => {
    const service = new TransactionCommitService(config);
    const txHashA = buildTxHashFromIndex(9);
    const txHashB = buildTxHashFromIndex(10);

    const promiseA = service.registerPendingCommit(txHashA);
    const promiseB = service.registerPendingCommit(txHashB);

    service.close();
    t.is(service.has(txHashA), false);
    t.is(service.has(txHashB), false);

    const results = await Promise.allSettled([promiseA, promiseB]);
    t.is(results[0].status, 'rejected');
    t.ok(results[0].reason instanceof PendingCommitCancelledError);
    t.ok(results[0].reason.message.includes(txHashA));
    t.is(results[1].status, 'rejected');
    t.ok(results[1].reason instanceof PendingCommitCancelledError);
    t.ok(results[1].reason.message.includes(txHashB));
});


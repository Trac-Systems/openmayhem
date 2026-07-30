import { test } from 'brittle';
import b4a from 'b4a';
import sinon from 'sinon';

import TransactionPoolService, {
    TransactionPoolAlreadyQueuedError,
    TransactionPoolFullError,
    TransactionPoolInvalidIncomingDataError,
    TransactionPoolMissingCommitReceiptError,
    TransactionPoolProofUnavailableError
} from '../../../../src/core/network/services/TransactionPoolService.js';
import TransactionCommitService from '../../../../src/core/network/services/TransactionCommitService.js';
import { safeDecodeApplyOperation } from '../../../../src/utils/protobuf/operationHelpers.js';
import { bigIntTo16ByteBuffer, decimalStringToBigInt } from '../../../../src/utils/amountSerialization.js';
import { BATCH_SIZE } from '../../../../src/utils/constants.js';
import { overrideConfig } from '../../../helpers/config.js';
import {
    buildTransferPayload,
    setupTransferScenario
} from '../../state/apply/transfer/transferScenarioHelpers.js';

if (typeof setTimeout !== "undefined" && typeof setTimeout.restore === "function") setTimeout.restore();
if (typeof setInterval !== "undefined" && typeof setInterval.restore === "function") setInterval.restore();
sinon.restore(); 

const CONFIG_DEFAULT = { enableWallet: true, txPoolSize: 10, processIntervalMs: 50 };
const CONFIG_TX_POOL_INCREASE = { enableWallet: true, txPoolSize: 100, processIntervalMs: 50 };

const config = overrideConfig(CONFIG_DEFAULT);
const increasedPoolConfig = overrideConfig(CONFIG_TX_POOL_INCREASE);

function createStateFixture() {
    return {
        async isAdminAllowedToValidate() { return false; },
        async allowedToValidate(_address) { return true; },
        async appendWithProofOfPublication(batch, batchTxHashes) {
            const timestamp = Date.now();
            return batch.map((completeTx, i) => ({
                txHash: batchTxHashes[i],
                completeTx,
                proof: b4a.alloc(32, 1), // Fake proof
                proofError: null,
                timestamp,
                blockNumber: 1000 + i 
            }));
        }
    };
}

test('TransactionPoolService processes queued transaction and resolves commit with proof', async t => {
    const context = await setupTransferScenario(t, { nodes: 4 });
    const validatorPeer = context.transferScenario.validatorPeer;
    const encodedTx = await buildTransferPayload(context);
    const decoded = safeDecodeApplyOperation(encodedTx);
    const txHash = decoded?.tro?.tx?.toString('hex');

    t.ok(txHash, 'tx hash extracted from transfer payload');
    if (!txHash) return;

    const clock = sinon.useFakeTimers();
    
    const txCommitService = new TransactionCommitService(config);
    const stateAdapter = createStateFixture();
    const poolService = new TransactionPoolService(stateAdapter, validatorPeer.wallet.address, txCommitService, config);

    try {
        await poolService.start();

        let isResolved = false;
        const pendingCommit = txCommitService.registerPendingCommit(txHash);
        pendingCommit.then(() => isResolved = true).catch(() => isResolved = true);
        
        poolService.addTransaction(txHash, encodedTx);

        for (let i = 0; i < 50 && !isResolved; i++) {
            clock.tick(config.processIntervalMs);
            await Promise.resolve();
        }

        const receipt = await pendingCommit;

        t.ok(receipt, 'pending commit resolves');
        t.is(receipt.txHash, txHash, 'receipt txHash matches queued tx');
        t.ok(b4a.isBuffer(receipt.proof), 'receipt contains proof buffer');
        t.ok(receipt.proof.length > 0, 'proof is non-empty');
        t.ok(Number.isSafeInteger(receipt.blockNumber), 'blockNumber is safe integer');
        t.ok(receipt.blockNumber >= 0, 'blockNumber is non-negative');
    } finally {
        await poolService.stopPool();
        txCommitService.close();
        clock.restore();
    }
});

test('TransactionPoolService processes batch of 10 queued transactions and resolves 10 receipts', async t => {
    const context = await setupTransferScenario(t, {
        nodes: 4, senderInitialBalance: bigIntTo16ByteBuffer(decimalStringToBigInt('100'))
    });
    const validatorPeer = context.transferScenario.validatorPeer;
    
    const clock = sinon.useFakeTimers();
    const txCommitService = new TransactionCommitService(config);
    const stateAdapter = createStateFixture();
    const poolService = new TransactionPoolService(stateAdapter, validatorPeer.wallet.address, txCommitService, config);

    try {
        await poolService.start();

        const txHashes = [];
        const pendingCommits = [];
        let resolvedCount = 0;

        for (let i = 0; i < 10; i++) {
            const encodedTx = await buildTransferPayload(context);
            const decoded = safeDecodeApplyOperation(encodedTx);
            const txHash = decoded?.tro?.tx?.toString('hex');
            txHashes.push(txHash);
            const pc = txCommitService.registerPendingCommit(txHash);
            pc.then(() => resolvedCount++).catch(() => resolvedCount++);
            pendingCommits.push(pc);
            poolService.addTransaction(txHash, encodedTx);
        }

        for (let i = 0; i < 50 && resolvedCount < 10; i++) {
            clock.tick(config.processIntervalMs);
            await Promise.resolve();
        }

        const receipts = await Promise.all(pendingCommits);
        t.is(receipts.length, 10, '10 pending commits resolved');

        const receiptsByHash = new Map(receipts.map(receipt => [receipt.txHash, receipt]));
        for (const txHash of txHashes) {
            const receipt = receiptsByHash.get(txHash);
            t.ok(receipt, `receipt exists for tx ${txHash}`);
            t.ok(b4a.isBuffer(receipt.proof), `proof is buffer for tx ${txHash}`);
            t.ok(receipt.proof.length > 0, `proof is non-empty for tx ${txHash}`);
        }
    } finally {
        await poolService.stopPool();
        txCommitService.close();
        clock.restore();
    }
});

test('TransactionPoolService.addTransaction enforces pool size limit via validateEnqueue', t => {
    const service = new TransactionPoolService({}, 'test', {}, { txPoolSize: 1, enableWallet: true });
    service.addTransaction('tx-1', b4a.from('aa', 'hex'));
    t.exception(() => service.addTransaction('tx-2', b4a.from('bb', 'hex')), TransactionPoolFullError);
    t.is(service.txPool.size(), 1);
});

test('TransactionPoolService.addTransaction rejects invalid incoming payload', t => {
    const service = new TransactionPoolService({}, 'test', {}, { txPoolSize: 10, enableWallet: true });
    t.exception(() => service.addTransaction('', b4a.from('aa', 'hex')), TransactionPoolInvalidIncomingDataError);
    t.exception(() => service.addTransaction('tx-1', 'not-a-buffer'), TransactionPoolInvalidIncomingDataError);
});

test('TransactionPoolService.addTransaction rejects duplicate txHash', t => {
    const service = new TransactionPoolService({}, 'v-addr', {}, { txPoolSize: 10, enableWallet: true });
    service.addTransaction('tx-dup', b4a.from('aa', 'hex'));
    t.exception(() => service.addTransaction('tx-dup', b4a.from('bb', 'hex')), TransactionPoolAlreadyQueuedError);
});

test('TransactionPoolService rejects pending commit when proof is unavailable', async t => {
    const clock = sinon.useFakeTimers();
    const txHash = 'a'.repeat(64);
    const txCommitService = new TransactionCommitService({ txCommitTimeout: 1_000 });
    const service = new TransactionPoolService(
        {
            async isAdminAllowedToValidate() { return false; },
            async allowedToValidate() { return true; },
            async appendWithProofOfPublication(_batch, hashes) {
                return [{ txHash: hashes[0], proof: null, blockNumber: 123, proofError: 'proof-missing', timestamp: new Date(1234) }];
            }
        }, 'v-addr', txCommitService, config
    );

    try {
        await service.start();
        
        let isResolved = false;
        const pendingCommit = txCommitService.registerPendingCommit(txHash);
        pendingCommit.then(() => isResolved = true).catch(() => isResolved = true);
        
        service.addTransaction(txHash, b4a.from('aa', 'hex'));

        for (let i = 0; i < 20 && !isResolved; i++) {
            clock.tick(config.processIntervalMs);
            await Promise.resolve();
        }

        try {
            await pendingCommit;
            t.fail('expected reject');
        } catch (error) {
            t.ok(error instanceof TransactionPoolProofUnavailableError);
            t.is(error.reason, 'proof-missing');
        }
    } finally {
        await service.stopPool();
        txCommitService.close();
        clock.restore();
    }
});

test('TransactionPoolService rejects pending commit when commit receipt is missing', async t => {
    const clock = sinon.useFakeTimers();
    const txHash = 'b'.repeat(64);
    const txCommitService = new TransactionCommitService({ txCommitTimeout: 1_000 });
    const service = new TransactionPoolService(
        {
            async isAdminAllowedToValidate() { return true; },
            async allowedToValidate() { return false; },
            async appendWithProofOfPublication() {
                return [{ txHash: 'diff', proof: b4a.from('aa', 'hex') }];
            }
        }, 'v-addr', txCommitService, config
    );

    try {
        await service.start();
        
        let isResolved = false;
        const pendingCommit = txCommitService.registerPendingCommit(txHash);
        pendingCommit.then(() => isResolved = true).catch(() => isResolved = true);
        
        service.addTransaction(txHash, b4a.from('aa', 'hex'));

        for (let i = 0; i < 20 && !isResolved; i++) {
            clock.tick(config.processIntervalMs);
            await Promise.resolve();
        }

        try {
            await pendingCommit;
            t.fail('expected reject');
        } catch (error) {
            t.ok(error instanceof TransactionPoolMissingCommitReceiptError);
        }
    } finally {
        await service.stopPool();
        txCommitService.close();
        clock.restore();
    }
});

test('TransactionPoolService rejects all pending commits when appendWithProofOfPublication throws', async t => {
    const clock = sinon.useFakeTimers();
    const txHashA = 'd'.repeat(64);
    const txHashB = 'e'.repeat(64);
    const appendError = new Error('append failed');
    const txCommitService = new TransactionCommitService({ txCommitTimeout: 1_000 });
    const service = new TransactionPoolService(
        {
            async isAdminAllowedToValidate() { return false; },
            async allowedToValidate() { return true; },
            async appendWithProofOfPublication() { throw appendError; }
        }, 'v-addr', txCommitService, config
    );

    try {
        await service.start();
        
        let resolvedCount = 0;
        const pendingA = txCommitService.registerPendingCommit(txHashA);
        const pendingB = txCommitService.registerPendingCommit(txHashB);
        pendingA.then(() => resolvedCount++).catch(() => resolvedCount++);
        pendingB.then(() => resolvedCount++).catch(() => resolvedCount++);

        service.addTransaction(txHashA, b4a.from('aa', 'hex'));
        service.addTransaction(txHashB, b4a.from('bb', 'hex'));

        for (let i = 0; i < 20 && resolvedCount < 2; i++) {
            clock.tick(config.processIntervalMs);
            await Promise.resolve();
        }

        const results = await Promise.allSettled([pendingA, pendingB]);
        t.is(results[0].status, 'rejected');
        t.is(results[1].reason, appendError);
    } finally {
        await service.stopPool();
        txCommitService.close();
        clock.restore();
    }
});

test('TransactionPoolProofUnavailableError normalizes timestamp values', t => {
    const txHash = 'f'.repeat(64);
    const fromDate = new TransactionPoolProofUnavailableError(txHash, 1, 'no-proof', new Date(5000));
    const fromInvalid = new TransactionPoolProofUnavailableError(txHash, 2, 'no-proof', 'invalid');
    t.is(fromDate.timestamp, 5000);
    t.is(fromInvalid.timestamp, 0);
});

test('TransactionPoolService.start is idempotent when scheduler is already running', async t => {
    const logs = [];
    const originalInfo = console.info;
    console.info = (...args) => logs.push(args.join(' '));
    const service = new TransactionPoolService(
        {
            async isAdminAllowedToValidate() { return false; },
            async allowedToValidate() { return false; },
            async appendWithProofOfPublication() { return []; }
        }, 'v-addr', { resolvePendingCommit() {}, rejectPendingCommit() {} }, config
    );

    try {
        await service.start();
        await service.start();
        t.ok(logs.some(m => m.includes('TransactionPoolService is already started')));
    } finally {
        console.info = originalInfo;
        await service.stopPool();
    }
});

test('TransactionPoolService schedules immediate follow-up run when queue remains after batch', async t => {
    const clock = sinon.useFakeTimers();
    let appendCalls = 0;
    const service = new TransactionPoolService(
        {
            async isAdminAllowedToValidate() { return true; },
            async allowedToValidate() { return true; },
            async appendWithProofOfPublication(_encodedBatch, txHashes) {
                appendCalls++;
                return txHashes.map(h => ({ txHash: h, proof: b4a.from('aa'), blockNumber: 1, timestamp: Date.now() }));
            }
        }, 'v-addr', { resolvePendingCommit() { return true; }, rejectPendingCommit() { return true; } }, increasedPoolConfig
    );

    try {
        for (let i = 0; i < BATCH_SIZE + 1; i++) service.addTransaction(`tx-${i}`, b4a.from('aa', 'hex'));
        await service.start();
        
        for (let i = 0; i < 20 && appendCalls < 2; i++) {
            clock.tick(10);
            await Promise.resolve();
        }

        t.ok(appendCalls >= 2, 'queue processed in at least two batches before interval');
    } finally {
        await service.stopPool();
        clock.restore();
    }
});

test('TransactionPoolService wraps worker errors from validation permission checks', async t => {
    const clock = sinon.useFakeTimers();
    const errors = [];
    const originalError = console.error;
    console.error = (...args) => errors.push(args);

    const service = new TransactionPoolService(
        {
            async isAdminAllowedToValidate() { throw new Error('permission boom'); },
            async allowedToValidate() { return false; },
            async appendWithProofOfPublication() { return []; }
        }, 'v-addr', { resolvePendingCommit() {}, rejectPendingCommit() {} }, { enableWallet: true, txPoolSize: 10, processIntervalMs: 10 }
    );

    try {
        await service.start();
        
        for (let i = 0; i < 20; i++) {
            clock.tick(10);
            await Promise.resolve();
        }

        const workerError = errors.map(e => e[1]).find(v => v instanceof Error && v.message.includes('permission boom'));
        t.ok(workerError, 'worker error is wrapped');
    } finally {
        console.error = originalError;
        await service.stopPool();
        clock.restore();
    }
});

test('TransactionPoolService.start does nothing when wallet is disabled', async t => {
    const logs = [];
    const originalInfo = console.info;
    console.info = (...args) => logs.push(args.join(' '));
    const service = new TransactionPoolService({}, 'v-addr', {}, { enableWallet: false });
    try {
        await service.start();
        t.ok(logs.some(m => m.includes('Wallet is not enabled')));
    } finally {
        console.info = originalInfo;
        await service.stopPool();
    }
});

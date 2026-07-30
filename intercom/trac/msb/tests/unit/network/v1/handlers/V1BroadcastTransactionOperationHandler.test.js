import test from 'brittle';
import b4a from 'b4a';

import V1BroadcastTransactionOperationHandler from '../../../../../src/core/network/protocols/v1/handlers/V1BroadcastTransactionOperationHandler.js';
import V1BroadcastTransactionRequest from '../../../../../src/core/network/protocols/v1/validators/V1BroadcastTransactionRequest.js';
import {V1ProtocolError} from '../../../../../src/core/network/protocols/v1/V1ProtocolError.js';

import {
    ResultCode,
    OperationType
} from '../../../../../src/utils/constants.js';

import * as PoolErrors from '../../../../../src/core/network/services/TransactionPoolService.js';
import * as CommitErrors from '../../../../../src/core/network/services/TransactionCommitService.js';


import PartialRoleAccessValidator from '../../../../../src/core/network/protocols/shared/validators/PartialRoleAccessValidator.js';
import PartialBootstrapDeploymentValidator from '../../../../../src/core/network/protocols/shared/validators/PartialBootstrapDeploymentValidator.js';
import PartialTransactionValidator from '../../../../../src/core/network/protocols/shared/validators/PartialTransactionValidator.js';
import PartialTransferValidator from '../../../../../src/core/network/protocols/shared/validators/PartialTransferValidator.js';
import { config as testConfig } from '../../../../helpers/config.js';
import { errorMessageIncludes } from '../../../../helpers/regexHelper.js';

const originalValidatorMethods = new Map([
    [V1BroadcastTransactionRequest, V1BroadcastTransactionRequest.prototype.validate],
    [PartialRoleAccessValidator, PartialRoleAccessValidator.prototype.validate],
    [PartialBootstrapDeploymentValidator, PartialBootstrapDeploymentValidator.prototype.validate],
    [PartialTransactionValidator, PartialTransactionValidator.prototype.validate],
    [PartialTransferValidator, PartialTransferValidator.prototype.validate]
]);

const VALID_ADDR = 'trac123z3gfpr2epjwww7ntm3m6ud2fhmq0tvts27p2f5mx3qkecsutlqfys769';
const VALID_TO_ADDR = 'trac1mqktwme8fvklrds4hlhfy6lhmsu9qgfn3c3kuhz7c5zwjt8rc3dqj9tx7h';
const VALID_PUB = b4a.alloc(33, 2);

const basePayload = () => ({
    tx: b4a.alloc(32),
    txv: b4a.alloc(32),
    in: b4a.alloc(32),
    is: b4a.alloc(64)
});

const roleAccessPayload = () => ({
    ...basePayload(),
    iw: b4a.alloc(32)
});

const transferPayload = () => ({
    ...basePayload(),
    to: VALID_TO_ADDR,
    am: b4a.alloc(16)
});

const transactionPayload = () => ({
    ...basePayload(),
    iw: b4a.alloc(32),
    ch: b4a.alloc(32),
    bs: b4a.alloc(32),
    mbs: b4a.alloc(32)
});

const bootstrapDeploymentPayload = () => ({
    ...basePayload(),
    bs: b4a.alloc(32),
    ic: b4a.alloc(32)
});

function restoreValidatorMethods() {
    for (const [Validator, originalValidate] of originalValidatorMethods.entries()) {
        Validator.prototype.validate = originalValidate;
    }
}

function setupHandler(t, overrides = {}) {
    restoreValidatorMethods();
    t.teardown(() => {
        restoreValidatorMethods();
    });

    // Bypass all validation layers
    [
        V1BroadcastTransactionRequest,
        PartialRoleAccessValidator,
        PartialBootstrapDeploymentValidator,
        PartialTransactionValidator,
        PartialTransferValidator
    ].forEach(v => v.prototype.validate = async () => true);

    const state = overrides.state || {
        allowedToValidate: async () => true,
        isAdminAllowedToValidate: async () => true
    };

    const wallet = overrides.wallet || {
        address: VALID_ADDR,
        getPublicKey: () => VALID_PUB,
        sign: () => b4a.alloc(64)
    };

    const txPool = overrides.txPool || {
        validateEnqueue() {},
        addTransaction() {}
    };

    const commitService = overrides.commit || {
        registerPendingCommit: () =>
            Promise.resolve({ proof: b4a.alloc(32), timestamp: 5 }),
        rejectPendingCommit() {}
    };
    const config = overrides.config || testConfig;

    const handler = new V1BroadcastTransactionOperationHandler(
        state,
        wallet,
        { v1HandleRateLimit() {} },
        txPool,
        { resolvePendingRequest() {} },
        commitService,
        config
    );

    handler.displayError = () => {};
    return handler;
}

function mockConn(assertFn) {
    return {
        remotePublicKey: b4a.alloc(32),
        protocolSession: {
            sendAndForget: assertFn || (() => {})
        },
        async flush() {},
        end() {}
    };
}

test('handleRequest: dispatches all supported operation types -> sends response', async t => {

    const scenarios = [
        { type: OperationType.ADD_WRITER, key: 'rao', data: roleAccessPayload() },
        { type: OperationType.REMOVE_WRITER, key: 'rao', data: roleAccessPayload() },
        { type: OperationType.ADMIN_RECOVERY, key: 'rao', data: roleAccessPayload() },
        { type: OperationType.TRANSFER, key: 'tro', data: transferPayload() },
        { type: OperationType.TX, key: 'txo', data: transactionPayload() },
        { type: OperationType.BOOTSTRAP_DEPLOYMENT, key: 'bdo', data: bootstrapDeploymentPayload() }
    ];

    for (const s of scenarios) {
        const handler = setupHandler(t);
        handler.decodeApplyOperation = () => ({
            type: s.type,
            address: VALID_ADDR,
            [s.key]: s.data
        });

        await handler.handleRequest(
            { id: b4a.alloc(32), broadcast_transaction_request: { data: b4a.alloc(1) } },
            mockConn()
        );
    }

    t.pass();
});

test('dispatchTransaction: missing/invalid type -> throws invalid payload error', async t => {

    const handler = setupHandler(t);

    await t.exception(
        async () => handler.dispatchTransaction(null),
        errorMessageIncludes('Decoded transaction type is missing')
    );

    await t.exception(
        async () => handler.dispatchTransaction({ type: 0 }),
        errorMessageIncludes('Decoded transaction type is missing')
    );

    await t.exception(
        async () => handler.dispatchTransaction({ type: 999 }),
        errorMessageIncludes('Unsupported transaction type')
    );
});

test('Commit service not configured', async t => {

    const handler = setupHandler(t, { commit: null });

    handler.decodeApplyOperation = () => ({
        type: OperationType.TX,
        address: VALID_ADDR,
        txo: transactionPayload()
    });

    await handler.handleRequest(
        { id: b4a.alloc(32), broadcast_transaction_request: { data: b4a.alloc(1) } },
        mockConn(res => {
            t.is(
                res.broadcast_transaction_response.result,
                ResultCode.INTERNAL_ERROR
            );
        })
    );
});

test('TxPool validateEnqueue full', async t => {

    const handler = setupHandler(t, {
        txPool: {
            validateEnqueue() {
                throw new PoolErrors.TransactionPoolFullError();
            }
        }
    });

    handler.decodeApplyOperation = () => ({
        type: OperationType.TX,
        address: VALID_ADDR,
        txo: transactionPayload()
    });

    await handler.handleRequest(
        { id: b4a.alloc(32), broadcast_transaction_request: { data: b4a.alloc(1) } },
        mockConn(res => {
            t.is(res.broadcast_transaction_response.result, ResultCode.NODE_OVERLOADED);
        })
    );
});

test('Commit registration error mapping', async t => {

    const errors = [
        CommitErrors.PendingCommitInvalidTxHashError,
        CommitErrors.PendingCommitAlreadyExistsError,
        CommitErrors.PendingCommitBufferFullError
    ];

    for (const Err of errors) {

        const handler = setupHandler(t, {
            commit: {
                registerPendingCommit() { throw new Err('x'); },
                rejectPendingCommit() {}
            }
        });

        handler.decodeApplyOperation = () => ({
            type: OperationType.TX,
            address: VALID_ADDR,
            txo: transactionPayload()
        });

        await handler.handleRequest(
            { id: b4a.alloc(32), broadcast_transaction_request: { data: b4a.alloc(1) } },
            mockConn()
        );
    }

    t.pass();
});

test('TxPool addTransaction error mapping', async t => {

    const poolErrors = [
        PoolErrors.TransactionPoolFullError,
        PoolErrors.TransactionPoolAlreadyQueuedError,
        PoolErrors.TransactionPoolInvalidIncomingDataError
    ];

    for (const Err of poolErrors) {

        const handler = setupHandler(t, {
            txPool: {
                validateEnqueue() {},
                addTransaction() { throw new Err('x'); }
            }
        });

        handler.decodeApplyOperation = () => ({
            type: OperationType.TX,
            address: VALID_ADDR,
            txo: transactionPayload()
        });

        await handler.handleRequest(
            { id: b4a.alloc(32), broadcast_transaction_request: { data: b4a.alloc(1) } },
            mockConn()
        );
    }

    t.pass();
});

test('Pending commit rejection branches', async t => {

    const errors = [
        new PoolErrors.TransactionPoolProofUnavailableError('x', 7),
        new PoolErrors.TransactionPoolMissingCommitReceiptError('x'),
        new CommitErrors.PendingCommitTimeoutError('x')
    ];

    for (const err of errors) {

        const handler = setupHandler(t, {
            commit: {
                registerPendingCommit() {
                    return Promise.reject(err);
                },
                rejectPendingCommit() {}
            }
        });

        handler.decodeApplyOperation = () => ({
            type: OperationType.TX,
            address: VALID_ADDR,
            txo: transactionPayload()
        });

        await handler.handleRequest(
            { id: b4a.alloc(32), broadcast_transaction_request: { data: b4a.alloc(1) } },
            mockConn()
        );
    }

    t.pass();
});

test('Capability validation failure', async t => {

    const handler = setupHandler(t, {
        state: {
            allowedToValidate: async () => false,
            isAdminAllowedToValidate: async () => false
        }
    });

    handler.decodeApplyOperation = () => ({
        type: OperationType.TX,
        address: VALID_ADDR,
        txo: transactionPayload()
    });

    await handler.handleRequest(
        { id: b4a.alloc(32), broadcast_transaction_request: { data: b4a.alloc(1) } },
        mockConn(res => {
            t.is(res.broadcast_transaction_response.result, ResultCode.NODE_HAS_NO_WRITE_ACCESS);
        })
    );
});

test('Response build failure branch', async t => {

    const handler = setupHandler(t);
    handler.decodeApplyOperation = () => ({
        type: OperationType.TX,
        address: VALID_ADDR,
        txo: transactionPayload()
    });

    const conn = {
        remotePublicKey: b4a.alloc(32),
        protocolSession: {
            sendAndForget() { throw new Error('fail'); }
        },
        end() {}
    };

    await handler.handleRequest(
        { id: b4a.alloc(32), broadcast_transaction_request: { data: b4a.alloc(1) } },
        conn
    );

    t.pass();
});

test('handleResponse: resolvePendingResponse throws -> delegates to handlePendingResponseError', async t => {

    const handler = setupHandler(t);

    handler.resolvePendingResponse = async () => {
        throw new Error('boom');
    };

    handler.handlePendingResponseError = () => {};

    await handler.handleResponse(
        { id: b4a.alloc(32) },
        { remotePublicKey: b4a.alloc(32), end() {} }
    );

    t.pass();
});

test('Sanitize removes null completion fields', async t => {

    const handler = setupHandler(t);

    const tx = {
        type: OperationType.TX,
        address: VALID_ADDR,
        txo: {
            ...transactionPayload(),
            va: null,
            vn: null,
            vs: null
        }
    };

    handler.decodeApplyOperation = () => tx;

    await handler.handleRequest(
        { id: b4a.alloc(32), broadcast_transaction_request: { data: b4a.alloc(1) } },
        mockConn()
    );

    t.absent(tx.txo.va);
    t.absent(tx.txo.vn);
    t.absent(tx.txo.vs);
});

test('Proof unavailable without timestamp branch', async t => {

    const handler = setupHandler(t, {
        commit: {
            registerPendingCommit() {
                return Promise.reject(
                    new PoolErrors.TransactionPoolProofUnavailableError('x')
                );
            },
            rejectPendingCommit() {}
        }
    });

    handler.decodeApplyOperation = () => ({
        type: OperationType.TX,
        address: VALID_ADDR,
        txo: transactionPayload()
    });

    await handler.handleRequest(
        { id: b4a.alloc(32), broadcast_transaction_request: { data: b4a.alloc(1) } },
        mockConn()
    );

    t.pass();
});

test('Proof unavailable timestamp <= 0 branch', async t => {

    const handler = setupHandler(t, {
        commit: {
            registerPendingCommit() {
                return Promise.reject(
                    new PoolErrors.TransactionPoolProofUnavailableError('x', 0)
                );
            },
            rejectPendingCommit() {}
        }
    });

    handler.decodeApplyOperation = () => ({
        type: OperationType.TX,
        address: VALID_ADDR,
        txo: transactionPayload()
    });

    await handler.handleRequest(
        { id: b4a.alloc(32), broadcast_transaction_request: { data: b4a.alloc(1) } },
        mockConn()
    );

    t.pass();
});

test('Request validator failure mapping branch', async t => {

    const handler = setupHandler(t);

    V1BroadcastTransactionRequest.prototype.validate = async () => {
        throw new Error('validation boom');
    };

    await handler.handleRequest(
        { id: b4a.alloc(32), broadcast_transaction_request: { data: b4a.alloc(1) } },
        mockConn()
    );

    t.pass();
});

test('handleRequest: flushes response before closing when protocol error requests disconnect', async t => {
    const handler = setupHandler(t);
    const events = [];

    V1BroadcastTransactionRequest.prototype.validate = async () => {
        throw new V1ProtocolError(ResultCode.INVALID_PAYLOAD, 'validation boom');
    };

    const connection = {
        remotePublicKey: b4a.alloc(32),
        protocolSession: {
            sendAndForget() {
                events.push('send');
            }
        },
        async flush() {
            events.push('flush');
            return true;
        },
        end() {
            events.push('end');
        }
    };

    await handler.handleRequest(
        { id: 'msg-flush-close', broadcast_transaction_request: { data: b4a.alloc(1) } },
        connection
    );

    t.alike(events, ['send', 'flush', 'end']);
});

test('decodeApplyOperation failure branch', async t => {

    const handler = setupHandler(t);

    handler.decodeApplyOperation = () => {
        throw new Error('decode fail');
    };

    await handler.handleRequest(
        { id: b4a.alloc(32), broadcast_transaction_request: { data: b4a.alloc(1) } },
        mockConn()
    );

    t.pass();
});

test('Response build internal failure branch', async t => {

    const badWallet = {
        address: VALID_ADDR,
        getPublicKey: () => VALID_PUB,
        sign: () => { throw new Error('sign fail'); }
    };

    const handler = new V1BroadcastTransactionOperationHandler(
        {
            allowedToValidate: async () => true,
            isAdminAllowedToValidate: async () => true
        },
        badWallet,
        { v1HandleRateLimit() {} },
        { validateEnqueue() {}, addTransaction() {} },
        { resolvePendingRequest() {} },
        {
            registerPendingCommit: () =>
                Promise.resolve({ proof: b4a.alloc(32), timestamp: 1 }),
            rejectPendingCommit() {}
        },
        testConfig
    );

    handler.displayError = () => {};

    handler.decodeApplyOperation = () => ({
        type: OperationType.TX,
        address: VALID_ADDR,
        txo: transactionPayload()
    });

    await handler.handleRequest(
        { id: b4a.alloc(32), broadcast_transaction_request: { data: b4a.alloc(1) } },
        mockConn()
    );

    t.pass();
});


test('Unsupported role access subtype', async t => {

    const handler = setupHandler(t);

    await t.exception(
        async () => handler.dispatchTransaction({
            type: 1234,
            address: VALID_ADDR,
            rao: roleAccessPayload(),
            // unsupported operation type
        }),
        errorMessageIncludes('Unsupported transaction type')
    );
});

test('Role access switch default branch', async t => {

    const handler = setupHandler(t);

    await t.exception(
        async () => handler.dispatchTransaction({
            type: OperationType.ADD_WRITER + 999, // still integer but not matched
            address: VALID_ADDR,
            rao: roleAccessPayload()
        }),
        errorMessageIncludes('Unsupported transaction type')
    );
});

test('TransactionPoolMissingCommitReceiptError via receipt branch', async t => {

    const handler = setupHandler(t, {
        commit: {
            registerPendingCommit() {
                return Promise.reject(
                    new PoolErrors.TransactionPoolMissingCommitReceiptError('x')
                );
            },
            rejectPendingCommit() {}
        }
    });

    handler.decodeApplyOperation = () => ({
        type: OperationType.TX,
        address: VALID_ADDR,
        txo: transactionPayload()
    });

    let capturedResultCode = null;
    await handler.handleRequest(
        { id: 'receipt-missing-id', broadcast_transaction_request: { data: b4a.alloc(1) } },
        mockConn(res => {
            capturedResultCode = res.broadcast_transaction_response.result;
        })
    );

    t.is(capturedResultCode, ResultCode.TX_COMMITTED_RECEIPT_MISSING);
});

test('PendingCommitBufferFullError mapping branch', async t => {

    const handler = setupHandler(t, {
        commit: {
            registerPendingCommit() {
                throw new CommitErrors.PendingCommitBufferFullError('x');
            },
            rejectPendingCommit() {}
        }
    });

    handler.decodeApplyOperation = () => ({
        type: OperationType.TX,
        address: VALID_ADDR,
        txo: transactionPayload()
    });

    await handler.handleRequest(
        { id: b4a.alloc(32), broadcast_transaction_request: { data: b4a.alloc(1) } },
        mockConn()
    );

    t.pass();
});

test('handleResponse extractor real path', async t => {

    const handler = setupHandler(t);

    handler.resolvePendingResponse = async (msg, conn, validator, extractor) => {
        const result = extractor({
            broadcast_transaction_response: { result: 123 }
        });
        t.is(result, 123);
    };

    await handler.handleResponse(
        { id: b4a.alloc(32) },
        { remotePublicKey: b4a.alloc(32), end() {} }
    );

    t.pass();
});

test('getOperationPayloadKey null branch', async t => {

    const handler = setupHandler(t);

    handler.decodeApplyOperation = () => ({
        type: 9999, // not recognized by isRoleAccess/isTransaction/etc
        address: VALID_ADDR
    });

    await handler.handleRequest(
        { id: b4a.alloc(32), broadcast_transaction_request: { data: b4a.alloc(1) } },
        mockConn()
    );

    t.pass();
});

test('TransactionPoolInvalidIncomingDataError mapping', async t => {

    const handler = setupHandler(t, {
        txPool: {
            validateEnqueue() {},
            addTransaction() {
                throw new PoolErrors.TransactionPoolInvalidIncomingDataError('x');
            }
        }
    });

    handler.decodeApplyOperation = () => ({
        type: OperationType.TX,
        address: VALID_ADDR,
        txo: transactionPayload()
    });

    await handler.handleRequest(
        { id: b4a.alloc(32), broadcast_transaction_request: { data: b4a.alloc(1) } },
        mockConn(res => {
            t.is(res.broadcast_transaction_response.result, ResultCode.INTERNAL_ERROR);
        })
    );
});

test('Unknown receipt error rethrow branch', async t => {

    const handler = setupHandler(t, {
        commit: {
            registerPendingCommit() {
                return Promise.reject(new Error('weird'));
            },
            rejectPendingCommit() {}
        }
    });

    handler.decodeApplyOperation = () => ({
        type: OperationType.TX,
        address: VALID_ADDR,
        txo: transactionPayload()
    });

    await handler.handleRequest(
        { id: b4a.alloc(32), broadcast_transaction_request: { data: b4a.alloc(1) } },
        mockConn()
    );

    t.pass();
});

test('validateEnqueue rethrow unknown error branch', async t => {

    const handler = setupHandler(t, {
        txPool: {
            validateEnqueue() { throw new Error('boom'); }
        }
    });

    handler.decodeApplyOperation = () => ({
        type: OperationType.TX,
        address: VALID_ADDR,
        txo: transactionPayload()
    });

    await handler.handleRequest(
        { id: b4a.alloc(32), broadcast_transaction_request: { data: b4a.alloc(1) } },
        mockConn()
    );

    t.pass();
});

test('Capability OR branch admin true', async t => {

    const handler = setupHandler(t, {
        state: {
            allowedToValidate: async () => false,
            isAdminAllowedToValidate: async () => true
        }
    });

    handler.decodeApplyOperation = () => ({
        type: OperationType.TX,
        address: VALID_ADDR,
        txo: transactionPayload()
    });

    await handler.handleRequest(
        { id: b4a.alloc(32), broadcast_transaction_request: { data: b4a.alloc(1) } },
        mockConn()
    );

    t.pass();
});

test('dispatchTransaction maps proof unavailable to V1ProtocolError and preserves positive timestamp', async t => {

    const proofUnavailableTimestamp = Date.now();
    const handler = setupHandler(t, {
        commit: {
            registerPendingCommit() {
                return Promise.reject(
                    new PoolErrors.TransactionPoolProofUnavailableError('x', 1, 'proof-missing', proofUnavailableTimestamp)
                );
            },
            rejectPendingCommit() {}
        }
    });

    try {
        await handler.dispatchTransaction({
            type: OperationType.TX,
            address: VALID_ADDR,
            txo: transactionPayload()
        });
        t.fail('expected dispatchTransaction to throw');
    } catch (error) {
        t.ok(error instanceof V1ProtocolError);
        t.is(error.resultCode, ResultCode.TX_ACCEPTED_PROOF_UNAVAILABLE);
        t.is(error.timestamp, proofUnavailableTimestamp);
    }
});

test('handleRequest builds proof unavailable response with propagated positive timestamp', async t => {

    const proofUnavailableTimestamp = Date.now();
    const handler = setupHandler(t, {
        commit: {
            registerPendingCommit() {
                return Promise.reject(
                    new PoolErrors.TransactionPoolProofUnavailableError('x', 1, 'proof-missing', proofUnavailableTimestamp)
                );
            },
            rejectPendingCommit() {}
        }
    });

    handler.decodeApplyOperation = () => ({
        type: OperationType.TX,
        address: VALID_ADDR,
        txo: transactionPayload()
    });

    await handler.handleRequest(
        { id: b4a.alloc(32), broadcast_transaction_request: { data: b4a.alloc(1) } },
        mockConn(res => {
            t.is(res.broadcast_transaction_response.result, ResultCode.TX_ACCEPTED_PROOF_UNAVAILABLE);
            t.is(res.broadcast_transaction_response.timestamp, proofUnavailableTimestamp);
        })
    );
});

test('handleRequest keeps connection open for TIMEOUT response', async t => {
    const handler = setupHandler(t, {
        commit: {
            registerPendingCommit() {
                return Promise.reject(new CommitErrors.PendingCommitTimeoutError('commit timeout'));
            },
            rejectPendingCommit() {}
        }
    });
    const events = [];

    handler.decodeApplyOperation = () => ({
        type: OperationType.TX,
        address: VALID_ADDR,
        txo: transactionPayload()
    });

    const connection = {
        remotePublicKey: b4a.alloc(32),
        protocolSession: {
            sendAndForget(message) {
                events.push('send');
                t.is(
                    message.broadcast_transaction_response.result,
                    ResultCode.TIMEOUT,
                    'validator should answer with TIMEOUT on pending commit timeout'
                );
            }
        },
        async flush() {
            events.push('flush');
            return true;
        },
        end() {
            events.push('end');
        }
    };

    await handler.handleRequest(
        { id: 'msg-timeout-keep-open', broadcast_transaction_request: { data: b4a.alloc(1) } },
        connection
    );

    t.alike(events, ['send']);
});

import test from 'brittle';
import V1BaseOperationHandler from '../../../../../src/core/network/protocols/v1/handlers/V1BaseOperationHandler.js';
import {ResultCode} from '../../../../../src/utils/constants.js';
import {V1ProtocolError} from '../../../../../src/core/network/protocols/v1/V1ProtocolError.js';

class MockRateLimiter {
    constructor() { this.called = false; }
    v1HandleRateLimit(conn) {
        this.called = true;
        this.conn = conn;
    }
}

class MockPendingReqService {
    constructor() {
        this.entries = {};
        this.stopped = [];
        this.resolved = [];
        this.rejected = [];
        this.shouldReject = true;
    }
    getPendingRequest(id) { return this.entries[id]; }
    stopPendingRequestTimeout(id) { this.stopped.push(id); }
    resolvePendingRequest(id, code) { this.resolved.push({ id, code }); }
    rejectPendingRequest(id, err) {
        if (this.shouldReject) this.rejected.push({ id, err });
        return this.shouldReject;
    }
}

const mockConfig = { disableRateLimit: false };

test('constructor: stores provided config -> config getter returns same reference', async (t) => {
    const handler = new V1BaseOperationHandler(null, null, mockConfig);
    t.is(handler.config, mockConfig, 'Should return the config passed in the constructor');
});

test('applyRateLimit: rate limit enabled -> calls rate limiter with connection', async (t) => {
    const rateLimiter = new MockRateLimiter();
    const handler = new V1BaseOperationHandler(rateLimiter, null, mockConfig);
    const conn = { id: 1 };

    handler.applyRateLimit(conn);

    t.ok(rateLimiter.called, 'Should call the rate limiter when enabled');
    t.is(rateLimiter.conn, conn, 'Should pass the connection to the rate limiter');
});

test('applyRateLimit: disableRateLimit=true -> skips rate limiter call', async (t) => {
    const rateLimiter = new MockRateLimiter();
    const handler = new V1BaseOperationHandler(rateLimiter, null, { disableRateLimit: true });

    handler.applyRateLimit({});

    t.absent(rateLimiter.called, 'Should NOT call the rate limiter when disableRateLimit is true');
});

test('resolvePendingResponse: pending request missing -> returns false', async (t) => {
    const pendingReq = new MockPendingReqService();
    const handler = new V1BaseOperationHandler(null, pendingReq, mockConfig);

    const result = await handler.resolvePendingResponse({ id: 'msg-123' }, {}, {}, () => {}, {});

    t.is(result, false, 'Should return false if the pending request does not exist');
});

test('resolvePendingResponse: valid pending response -> stops timeout and resolves request', async (t) => {
    const pendingReq = new MockPendingReqService();
    pendingReq.entries['msg-123'] = { id: 'msg-123' };

    const handler = new V1BaseOperationHandler(null, pendingReq, mockConfig);

    let validated = false;
    const validatorMock = { async validate() { validated = true; } };
    const extractCodeMock = () => 'SUCCESS';

    const result = await handler.resolvePendingResponse(
        { id: 'msg-123' },
        {},
        validatorMock,
        extractCodeMock,
        {}
    );

    t.is(result, true, 'Should return true after resolving the request');
    t.is(pendingReq.stopped[0], 'msg-123', 'Should stop the timeout');
    t.ok(validated, 'Should call validation');
    t.is(pendingReq.resolved[0].code, 'SUCCESS', 'Should extract resultCode and resolve');
});

test('resolvePendingResponse: validator throws -> propagates validation error', async (t) => {
    const pendingReq = new MockPendingReqService();
    pendingReq.entries['msg-123'] = { id: 'msg-123' };

    const handler = new V1BaseOperationHandler(null, pendingReq, mockConfig);

    const validatorMock = {
        async validate() { throw new Error('Validation Failed'); }
    };

    await t.exception(async () => {
        await handler.resolvePendingResponse(
            { id: 'msg-123' },
            {},
            validatorMock,
            () => {},
            {}
        );
    }, /Validation Failed/, 'Should propagate the validation error');
});

test('handlePendingResponseError: request already rejected -> does not close connection', async (t) => {
    const pendingReq = new MockPendingReqService();
    pendingReq.shouldReject = false;

    const handler = new V1BaseOperationHandler(null, pendingReq, mockConfig);

    let ended = false;
    const connMock = { end: () => { ended = true; } };

    handler.handlePendingResponseError('msg-123', connMock, new Error('test'), 'step');

    t.absent(ended, 'Should NOT end the connection if the request was already rejected');
});

test('handlePendingResponseError: unknown native error -> maps to V1ProtocolError', async (t) => {
    const pendingReq = new MockPendingReqService();
    const handler = new V1BaseOperationHandler(null, pendingReq, mockConfig);

    // Bypass display logic for isolation
    handler.displayError = () => {};

    let ended = false;
    const connMock = {
        end: () => { ended = true; },
        remotePublicKey: Buffer.alloc(32)
    };

    handler.handlePendingResponseError(
        'msg-123',
        connMock,
        new Error('Random native error'),
        'test-step'
    );

    t.is(pendingReq.rejected.length, 1, 'Should reject the pending request');

    const capturedError = pendingReq.rejected[0].err;

    t.ok(capturedError instanceof V1ProtocolError, 'Should map to V1ProtocolError');
    t.is(capturedError.resultCode, ResultCode.UNEXPECTED_ERROR, 'Should map to UNEXPECTED_ERROR');
    t.is(capturedError.message, 'Random native error', 'Should preserve original message');
    t.absent(ended, 'Unexpected errors should not close the connection directly in this handler');
});

test('handlePendingResponseError: protocol error -> does not close connection directly', async (t) => {
    const pendingReq = new MockPendingReqService();
    const handler = new V1BaseOperationHandler(null, pendingReq, mockConfig);

    handler.displayError = () => {};

    let ended = false;
    const connMock = {
        end: () => { ended = true; },
        remotePublicKey: Buffer.alloc(32)
    };

    const protocolError = new V1ProtocolError(999, 'FATAL_ERROR');

    handler.handlePendingResponseError(
        'msg-123',
        connMock,
        protocolError,
        'test-step'
    );

    t.is(
        pendingReq.rejected[0].err,
        protocolError,
        'Should pass protocol error without wrapping'
    );

    t.absent(ended, 'Connection closing is delegated to ConnectionManager');
});

test('handlePendingResponseError: delegates logging -> calls displayError', async (t) => {
    const pendingReq = new MockPendingReqService();
    const handler = new V1BaseOperationHandler(null, pendingReq, { hrp: 'trac' });

    let called = false;
    handler.displayError = () => { called = true; };

    handler.handlePendingResponseError(
        'msg-1',
        { end() {}, remotePublicKey: Buffer.alloc(32) },
        new Error('boom'),
        'step'
    );

    t.ok(called);
});

test('handlePendingResponseError: protocol-shaped error -> keeps original error instance', async (t) => {
    const pendingReq = new MockPendingReqService();
    const handler = new V1BaseOperationHandler(null, pendingReq, mockConfig);

    const protocolError = new V1ProtocolError(123, 'protocol-shaped');

    handler.displayError = () => {};

    handler.handlePendingResponseError(
        'msg-1',
        { end() {}, remotePublicKey: Buffer.alloc(32) },
        protocolError,
        'step'
    );

    t.is(
        pendingReq.rejected[0].err,
        protocolError,
        'Should not wrap protocol error'
    );
});

test('handlePendingResponseError: undefined error -> uses Unexpected error fallback', async (t) => {
    const pendingReq = new MockPendingReqService();
    const handler = new V1BaseOperationHandler(null, pendingReq, mockConfig);

    handler.displayError = () => {};

    handler.handlePendingResponseError(
        'msg-123',
        { end() {}, remotePublicKey: Buffer.alloc(32) },
        undefined,
        'step'
    );

    const captured = pendingReq.rejected[0].err;

    t.ok(captured instanceof V1ProtocolError);
    t.is(captured.resultCode, ResultCode.UNEXPECTED_ERROR);
    t.is(captured.message, 'Unexpected error');
});

test('displayError: real implementation with invalid config -> throws', async (t) => {
    const pendingReq = new MockPendingReqService();

    const handler = new V1BaseOperationHandler(
        null,
        pendingReq,
        {} // intentionally invalid config
    );

    const originalConsoleError = console.error;
    console.error = () => {};

    await t.exception(() => {
        handler.displayError(
            'step',
            Buffer.alloc(33, 1),
            new Error('boom')
        );
    });

    console.error = originalConsoleError;

    t.pass();
});

test('handlePendingResponseError: primitive error value -> uses Unexpected error fallback', async (t) => {
    const pendingReq = new MockPendingReqService();
    const handler = new V1BaseOperationHandler(null, pendingReq, mockConfig);

    handler.displayError = () => {};

    handler.handlePendingResponseError(
        'msg-123',
        { end() {}, remotePublicKey: Buffer.alloc(32) },
        'string error',
        'step'
    );

    const captured = pendingReq.rejected[0].err;

    t.ok(captured instanceof V1ProtocolError);
    t.is(captured.resultCode, ResultCode.UNEXPECTED_ERROR);
    t.is(captured.message, 'Unexpected error');
});

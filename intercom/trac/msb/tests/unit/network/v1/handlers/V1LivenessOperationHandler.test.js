import test from 'brittle';
import b4a from 'b4a';
import V1LivenessOperationHandler from '../../../../../src/core/network/protocols/v1/handlers/V1LivenessOperationHandler.js';
import V1LivenessRequest from '../../../../../src/core/network/protocols/v1/validators/V1LivenessRequest.js';
import V1LivenessResponse from '../../../../../src/core/network/protocols/v1/validators/V1LivenessResponse.js';
import { ResultCode } from '../../../../../src/utils/constants.js';
import {V1ProtocolError} from '../../../../../src/core/network/protocols/v1/V1ProtocolError.js';

// Backup original validators
const originalReqValidate = V1LivenessRequest.prototype.validate;
const originalResValidate = V1LivenessResponse.prototype.validate;

const mockConfig = {
    hrp: 'trac',
    addressPrefix: 'trac',
    networkId: 1,
    disableRateLimit: true,
    network: { hrp: 'trac' }
};

// Minimal wallet required so constructor does not break
const mockWallet = {
    getPublicKey: () => b4a.alloc(33, 2),
    sign: () => b4a.alloc(64),
    address: 'trac123z3gfpr2epjwww7ntm3m6ud2fhmq0tvts27p2f5mx3qkecsutlqfys769',
    networkId: 1
};

class MockConnection {
    constructor() {
        this.remotePublicKey = b4a.alloc(32);
        this.ended = false;
        this.sentPayload = null;
        this.flushCalled = false;
        this.protocolSession = {
            sendAndForget: () => {}
        };
    }

    async flush() {
        this.flushCalled = true;
        return true;
    }

    end() {
        this.ended = true;
    }
}

const mockRateLimiter = {
    v1HandleRateLimit: () => {}
};

test('handleRequest: request validation and response send -> covers success and error branches', async (t) => {

    // -------- SUCCESS FLOW (does not validate real payload) --------
    {
        V1LivenessRequest.prototype.validate = async () => {};

        const handler = new V1LivenessOperationHandler(
            mockWallet,
            mockRateLimiter,
            {},
            mockConfig
        );

        handler.displayError = () => {};

        const conn = new MockConnection();

        await handler.handleRequest({ id: 'msg-success' }, conn);

        // Real factory is not being asserted
        t.pass('Success path executed');
    }

    // -------- VALIDATION ERROR with close-on-policy result code --------
    {
        V1LivenessRequest.prototype.validate = async () => {
            throw new V1ProtocolError(ResultCode.INVALID_PAYLOAD, 'Validation Fail');
        };

        const handler = new V1LivenessOperationHandler(
            mockWallet,
            mockRateLimiter,
            {},
            mockConfig
        );

        handler.displayError = () => {};

        const conn = new MockConnection();

        await handler.handleRequest({ id: 'msg-end' }, conn);

        t.ok(conn.ended);
        t.ok(conn.flushCalled);
    }

    // -------- VALIDATION ERROR with keep-open result code --------
    {
        V1LivenessRequest.prototype.validate = async () => {
            throw new V1ProtocolError(ResultCode.TX_ALREADY_PENDING, 'Validation Fail No End');
        };

        const handler = new V1LivenessOperationHandler(
            mockWallet,
            mockRateLimiter,
            {},
            mockConfig
        );

        handler.displayError = () => {};

        const conn = new MockConnection();

        await handler.handleRequest({ id: 'msg-no-end' }, conn);

        t.absent(conn.ended);
        t.absent(conn.flushCalled);
    }

    // -------- SEND FAILURE (second catch block) --------
    {
        V1LivenessRequest.prototype.validate = async () => {};

        const handler = new V1LivenessOperationHandler(
            mockWallet,
            mockRateLimiter,
            {},
            mockConfig
        );

        handler.displayError = () => {};

        const conn = new MockConnection();

        conn.protocolSession.sendAndForget = () => {
            throw new Error('send fail');
        };

        await handler.handleRequest({ id: 'msg-send-fail' }, conn);

        t.ok(conn.ended);
    }
});

test('handleResponse: resolvePendingResponse result handling -> covers success and delegated error branches', async (t) => {

    const handler = new V1LivenessOperationHandler(
        mockWallet,
        mockRateLimiter,
        {},
        mockConfig
    );

    // resolvePendingResponse success path
    {
        let extractorCalled = false;

        handler.resolvePendingResponse = async (msg, conn, val, extractor) => {
            extractor({ liveness_response: { result: 1 } });
            extractorCalled = true;
        };

        await handler.handleResponse({ id: 'msg-1' }, new MockConnection());

        t.ok(extractorCalled);
    }

    // resolvePendingResponse error path
    {
        handler.resolvePendingResponse = async () => {
            throw new Error('Fail');
        };

        let handled = false;

        handler.handlePendingResponseError = () => {
            handled = true;
        };

        await handler.handleResponse({ id: 'msg-2' }, new MockConnection());

        t.ok(handled);
    }
});

test('validators cleanup: restore original validator methods', async (t) => {
    V1LivenessRequest.prototype.validate = originalReqValidate;
    V1LivenessResponse.prototype.validate = originalResValidate;
    t.pass();
});

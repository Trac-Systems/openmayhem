import b4a from 'b4a';
import sinon from 'sinon';
import { config } from '../../../helpers/config.js';
import { NetworkOperationType, V1_PROTOCOL_PAYLOAD_MAX_SIZE } from '../../../../src/utils/constants.js';
import { encodeV1networkOperation } from '../../../../src/utils/protobuf/operationHelpers.js';
import NetworkMessageRouterV1 from '../../../../src/core/network/protocols/v1/NetworkMessageRouter.js';
import V1LivenessOperationHandler from '../../../../src/core/network/protocols/v1/handlers/V1LivenessOperationHandler.js';
import V1BroadcastTransactionOperationHandler from '../../../../src/core/network/protocols/v1/handlers/V1BroadcastTransactionOperationHandler.js';

const resolveStub = (stubSource) => {
    if (typeof stubSource === 'function') {
        return stubSource;
    }
    if (stubSource && typeof stubSource.stub === 'function') {
        return stubSource.stub.bind(stubSource);
    }
    throw new Error('Stub source must provide a stub function');
};

export const makeConnection = (stubSource) => {
    const stub = resolveStub(stubSource);
    return {
        remotePublicKey: b4a.alloc(32, 0x01),
        protocolSession: {
            setV1AsPreferredProtocol: stub()
        },
        end: stub()
    };
};

export const makeHandlers = (sandbox) => ({
    livenessRequest: sandbox.stub(V1LivenessOperationHandler.prototype, 'handleRequest').resolves(),
    livenessResponse: sandbox.stub(V1LivenessOperationHandler.prototype, 'handleResponse').resolves(),
    broadcastRequest: sandbox.stub(V1BroadcastTransactionOperationHandler.prototype, 'handleRequest').resolves(),
    broadcastResponse: sandbox.stub(V1BroadcastTransactionOperationHandler.prototype, 'handleResponse').resolves()
});

export const makeRouter = (t, { walletAddress = 'test-wallet' } = {}) => {
    const sandbox = sinon.createSandbox();
    if (t && typeof t.teardown === 'function') {
        t.teardown(() => sandbox.restore());
    }
    sandbox.stub(console, 'error');

    const handlerStubs = makeHandlers(sandbox);
    const router = new NetworkMessageRouterV1(
        {},
        { address: walletAddress },
        {},
        {},
        {},
        {},
        config
    );

    return { router, handlerStubs, sandbox };
};

export const makeRouterContext = (t, options) => {
    const { router, handlerStubs, sandbox } = makeRouter(t, options);
    const connection = makeConnection(sandbox);
    return { router, handlerStubs, sandbox, connection };
};

export const buildMessage = (type, extra = {}) => encodeV1networkOperation({
    type,
    id: '1',
    ...extra
});

export const buildEmptyMessage = () => b4a.alloc(0);

export const buildOversizedMessage = () => b4a.alloc(V1_PROTOCOL_PAYLOAD_MAX_SIZE + 1, 0x01);

export const buildMalformedMessage = () => b4a.from([0x07]);

export const buildMaxSizeMessage = () => {
    const basePayload = {
        type: NetworkOperationType.LIVENESS_REQUEST,
        id: '1',
        capabilities: ['']
    };
    const baseLength = encodeV1networkOperation(basePayload).length;
    let fillerLength = Math.max(0, V1_PROTOCOL_PAYLOAD_MAX_SIZE - baseLength - 1);
    let buffer = encodeV1networkOperation({
        ...basePayload,
        capabilities: ['a'.repeat(fillerLength)]
    });

    if (buffer.length !== V1_PROTOCOL_PAYLOAD_MAX_SIZE) {
        const delta = V1_PROTOCOL_PAYLOAD_MAX_SIZE - buffer.length;
        fillerLength = Math.max(0, fillerLength + delta);
        buffer = encodeV1networkOperation({
            ...basePayload,
            capabilities: ['a'.repeat(fillerLength)]
        });
    }

    if (buffer.length !== V1_PROTOCOL_PAYLOAD_MAX_SIZE) {
        throw new Error(
            `Failed to build payload of size ${V1_PROTOCOL_PAYLOAD_MAX_SIZE}, got ${buffer.length}`
        );
    }

    return buffer;
};

export const getUnsupportedType = () => {
    const values = Object.values(NetworkOperationType).map(Number);
    return Math.max(...values) + 1;
};

export const assertDisconnected = (t, connection, message = 'should end connection') => {
    t.ok(connection.end.calledOnce, message);
};

export const assertPreferredSet = (t, connection, message = 'should set preferred protocol') => {
    t.ok(connection.protocolSession.setV1AsPreferredProtocol.calledOnce, message);
};

export const assertPreferredNotSet = (t, connection, message = 'should not set preferred protocol') => {
    t.ok(connection.protocolSession.setV1AsPreferredProtocol.notCalled, message);
};

export const assertNoHandlersCalled = (t, handlerStubs) => {
    for (const [key, stub] of Object.entries(handlerStubs)) {
        t.ok(stub.notCalled, `${key} not called`);
    }
};

export const assertOnlyHandlerCalled = (t, handlerStubs, expectedKey) => {
    for (const [key, stub] of Object.entries(handlerStubs)) {
        if (key === expectedKey) {
            t.ok(stub.calledOnce, `${key} called`);
        } else {
            t.ok(stub.notCalled, `${key} not called`);
        }
    }
};

export const assertRejected = (
    t,
    { connection, handlerStubs, preferred = 'not', disconnectMessage } = {}
) => {
    if (preferred === 'set') {
        assertPreferredSet(t, connection);
    } else {
        assertPreferredNotSet(t, connection);
    }

    assertDisconnected(t, connection, disconnectMessage);
    assertNoHandlersCalled(t, handlerStubs);
};

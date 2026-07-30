import { test } from 'brittle';
import { NetworkOperationType } from '../../../../src/utils/constants.js';
import {
    assertDisconnected,
    assertOnlyHandlerCalled,
    assertPreferredSet,
    assertRejected,
    buildEmptyMessage,
    buildMalformedMessage,
    buildMaxSizeMessage,
    buildMessage,
    buildOversizedMessage,
    getUnsupportedType,
    makeRouterContext
} from '../utils/v1TestUtils.js';

const SUPPORTED_OPERATION_CASES = [
    {
        name: 'LIVENESS_REQUEST -> liveness.handleRequest',
        type: NetworkOperationType.LIVENESS_REQUEST,
        handlerKey: 'livenessRequest'
    },
    {
        name: 'LIVENESS_RESPONSE -> liveness.handleResponse',
        type: NetworkOperationType.LIVENESS_RESPONSE,
        handlerKey: 'livenessResponse'
    },
    {
        name: 'BROADCAST_TRANSACTION_REQUEST -> broadcast.handleRequest',
        type: NetworkOperationType.BROADCAST_TRANSACTION_REQUEST,
        handlerKey: 'broadcastRequest'
    },
    {
        name: 'BROADCAST_TRANSACTION_RESPONSE -> broadcast.handleResponse',
        type: NetworkOperationType.BROADCAST_TRANSACTION_RESPONSE,
        handlerKey: 'broadcastResponse'
    }
];

const runRejectionCase = async (t, message, { preferred = 'not' } = {}) => {
    const { router, handlerStubs, connection } = makeRouterContext(t);
    await router.route(message, connection);
    assertRejected(t, { connection, handlerStubs, preferred });
};

test('NetworkMessageRouterV1', async (t) => {
    await t.test('pre-validation', async (t) => {
        const cases = [
            { name: 'rejects null message', message: null },
            { name: 'rejects non-buffer message', message: 'not-a-buffer' },
            { name: 'rejects empty buffer', message: buildEmptyMessage() },
            { name: 'rejects oversized buffer', message: buildOversizedMessage() }
        ];

        for (const { name, message } of cases) {
            await t.test(name, async (t) => {
                await runRejectionCase(t, message);
            });
        }

        await t.test('accepts max size payload', async (t) => {
            const { router, handlerStubs, connection } = makeRouterContext(t);
            const maxSizePayload = buildMaxSizeMessage();

            await router.route(maxSizePayload, connection);

            assertPreferredSet(t, connection);
            t.ok(connection.end.notCalled, 'should not end connection');
            t.ok(handlerStubs.livenessRequest.calledOnce, 'liveness handler called');
        });
    });

    await t.test('decode failures', async (t) => {
        await t.test('disconnects on decode error', async (t) => {
            await runRejectionCase(t, buildMalformedMessage());
        });
    });

    await t.test('type validation', async (t) => {
        const invalidTypeCases = [
            { name: 'rejects type 0', message: buildMessage(0) },
            {
                name: 'rejects missing type (defaults to 0)',
                message: buildMessage(NetworkOperationType.LIVENESS_REQUEST, { type: undefined })
            }
        ];

        for (const { name, message } of invalidTypeCases) {
            await t.test(name, async (t) => {
                await runRejectionCase(t, message);
            });
        }

        await t.test('unsupported type disconnects after setting preferred protocol', async (t) => {
            await runRejectionCase(t, buildMessage(getUnsupportedType()), { preferred: 'set' });
        });
    });

    await t.test('routing', async (t) => {
        for (const { name, type, handlerKey } of SUPPORTED_OPERATION_CASES) {
            await t.test(name, async (t) => {
                const { router, handlerStubs, connection } = makeRouterContext(t);

                await router.route(buildMessage(type), connection);

                assertPreferredSet(t, connection);
                t.ok(connection.end.notCalled, 'should not end connection');
                assertOnlyHandlerCalled(t, handlerStubs, handlerKey);

                const handler = handlerStubs[handlerKey];
                t.is(handler.firstCall.args[0].type, type, 'passes decoded message');
                t.is(handler.firstCall.args[1], connection, 'passes connection');
            });
        }

        const handlerErrorCases = [
            {
                name: 'liveness handler throws',
                type: NetworkOperationType.LIVENESS_REQUEST,
                handlerKey: 'livenessRequest',
                error: new Error('handler-fail')
            },
            {
                name: 'broadcast request handler throws',
                type: NetworkOperationType.BROADCAST_TRANSACTION_REQUEST,
                handlerKey: 'broadcastRequest',
                error: new Error('broadcast-request-fail')
            },
            {
                name: 'broadcast response handler throws',
                type: NetworkOperationType.BROADCAST_TRANSACTION_RESPONSE,
                handlerKey: 'broadcastResponse',
                error: new Error('broadcast-response-fail')
            }
        ];

        for (const { name, type, handlerKey, error } of handlerErrorCases) {
            await t.test(name, async (t) => {
                const { router, handlerStubs, connection } = makeRouterContext(t);

                handlerStubs[handlerKey].rejects(error);

                await router.route(buildMessage(type), connection);

                assertPreferredSet(t, connection);
                t.ok(handlerStubs[handlerKey].calledOnce, 'handler called');
                assertDisconnected(t, connection);
            });
        }
    });
});

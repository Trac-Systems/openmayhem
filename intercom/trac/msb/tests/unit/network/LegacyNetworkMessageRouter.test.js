import { test } from 'brittle';
import sinon from 'sinon';
import b4a from 'b4a';
import { config } from '../../helpers/config.js';
import NetworkMessageRouter from '../../../src/core/network/protocols/legacy/NetworkMessageRouter.js';
import LegacyGetRequestHandler from '../../../src/core/network/protocols/legacy/handlers/LegacyGetRequestHandler.js';
import LegacyResponseHandler from '../../../src/core/network/protocols/legacy/handlers/LegacyResponseHandler.js';
import { NETWORK_MESSAGE_TYPES } from '../../../src/utils/constants.js';

const makeConnection = (sandbox) => ({
    remotePublicKey: b4a.alloc(32, 0x01),
    protocolSession: {
        setLegacyAsPreferredProtocol: sandbox.stub()
    },
    end: sandbox.stub()
});

const makeRouterContext = (t) => {
    const sandbox = sinon.createSandbox();
    t.teardown(() => sandbox.restore());

    const getHandler = sandbox.stub(LegacyGetRequestHandler.prototype, 'handle').resolves();
    const responseHandler = sandbox.stub(LegacyResponseHandler.prototype, 'handle').resolves();
    const router = new NetworkMessageRouter({}, { address: 'test-wallet' }, {}, {}, config);
    const connection = makeConnection(sandbox);

    return { connection, getHandler, responseHandler, router };
};

test('LegacyNetworkMessageRouter', async (t) => {
    await t.test('routes legacy string GET messages', async (t) => {
        const { connection, getHandler, responseHandler, router } = makeRouterContext(t);
        const message = NETWORK_MESSAGE_TYPES.GET.VALIDATOR;

        await router.route(message, connection);

        t.ok(connection.protocolSession.setLegacyAsPreferredProtocol.calledOnce, 'should prefer legacy protocol');
        t.ok(getHandler.calledOnce, 'should route GET message');
        t.ok(responseHandler.notCalled, 'should not route response handler');
        t.is(getHandler.firstCall.args[0], message, 'passes GET message through to handler');
    });

    await t.test('routes legacy object response messages', async (t) => {
        const { connection, getHandler, responseHandler, router } = makeRouterContext(t);
        const message = { op: NETWORK_MESSAGE_TYPES.RESPONSE.VALIDATOR };

        await router.route(message, connection);

        t.ok(connection.protocolSession.setLegacyAsPreferredProtocol.calledOnce, 'should prefer legacy protocol');
        t.ok(responseHandler.calledOnce, 'should route response message');
        t.ok(getHandler.notCalled, 'should not route GET handler');
        t.is(responseHandler.firstCall.args[0], message, 'passes response message through to handler');
    });
});

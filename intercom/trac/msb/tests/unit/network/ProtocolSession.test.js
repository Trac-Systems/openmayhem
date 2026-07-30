import { test } from 'brittle';
import sinon from 'sinon';
import ProtocolSession from '../../../src/core/network/protocols/ProtocolSession.js';
import { ResultCode } from '../../../src/utils/constants.js';
import { config } from '../../helpers/config.js';
import { testKeyPair1 } from '../../fixtures/apply.fixtures.js';
import { WalletProvider } from 'trac-wallet';

async function createWallet() {
    return await new WalletProvider(config).fromSecretKey(testKeyPair1.secretKey)
}

function makeProtocol(sendStub) {
    return {
        send: sendStub ?? sinon.stub().resolves(ResultCode.OK),
        sendAndForget: sinon.stub(),
        decode: sinon.stub(),
        close: sinon.stub()
    };
}

test('ProtocolSession', (t) => {
    t.teardown(() => sinon.restore());

    test('probe sets preferred protocol to v1 on OK', async (t) => {
        const v1Send = sinon.stub().resolves(ResultCode.OK);
        const session = new ProtocolSession(
            makeProtocol(),
            makeProtocol(v1Send),
            await createWallet(),
            config
        );

        await session.probe();
        t.is(session.preferredProtocol, session.supportedProtocols.V1);
        t.ok(v1Send.calledOnce);
    });

    test('probe sets preferred protocol to legacy on non-OK', async (t) => {
        const v1Send = sinon.stub().resolves(ResultCode.TIMEOUT);
        const session = new ProtocolSession(
            makeProtocol(),
            makeProtocol(v1Send),
            await createWallet(),
            config
        );

        await session.probe();
        t.is(session.preferredProtocol, session.supportedProtocols.LEGACY);
        t.ok(v1Send.calledOnce);
    });

    test('probe sets preferred protocol to legacy on rejection', async (t) => {
        const v1Send = sinon.stub().rejects(new Error('boom'));
        const session = new ProtocolSession(
            makeProtocol(),
            makeProtocol(v1Send),
            await createWallet(),
            config
        );

        await session.probe();
        t.is(session.preferredProtocol, session.supportedProtocols.LEGACY);
        t.ok(v1Send.calledOnce);
    });

    test('sendHealthCheck returns OK when preferred is v1', async (t) => {
        const v1Send = sinon.stub().resolves(ResultCode.OK);
        const session = new ProtocolSession(
            makeProtocol(),
            makeProtocol(v1Send),
            await createWallet(),
            config
        );

        session.setV1AsPreferredProtocol();
        const result = await session.sendHealthCheck();
        t.is(result, ResultCode.OK);
        t.ok(v1Send.calledOnce);
    });

    test('sendHealthCheck returns OK when preferred is legacy', async (t) => {
        const session = new ProtocolSession(
            makeProtocol(),
            makeProtocol(),
            await createWallet(),
            config
        );

        session.setLegacyAsPreferredProtocol();
        const result = await session.sendHealthCheck();
        t.is(result, ResultCode.OK);
    });

    test('sendHealthCheck returns UNSPECIFIED when not probed', async (t) => {
        const session = new ProtocolSession(
            makeProtocol(),
            makeProtocol(),
            await createWallet(),
            config
        );

        const result = await session.sendHealthCheck();
        t.is(result, ResultCode.UNSPECIFIED);
    });

    test('isHealthCheckSupported throws when not probed', async (t) => {
        const session = new ProtocolSession(
            makeProtocol(),
            makeProtocol(),
            await createWallet(),
            config
        );

        await t.exception.all(() => session.isHealthCheckSupported());
    });
});

import { test } from 'brittle';
import sinon from 'sinon';
import b4a from 'b4a';

import PeerWallet from 'trac-wallet';
import NetworkWalletFactory, { EphemeralWallet } from '../../../src/core/network/identity/NetworkWalletFactory.js';
import { errorMessageIncludes } from '../../helpers/regexHelper.js';
import { testKeyPair1, testKeyPair2 } from '../../fixtures/apply.fixtures.js';
import { config } from '../../helpers/config.js';

test('NetworkWalletFactory.provide returns wallet when enabled', async t => {
    const publicKey = b4a.from(testKeyPair2.publicKey, 'hex');
    const address = PeerWallet.encodeBech32m(config.addressPrefix, publicKey);
    const signResult = b4a.from('abcd', 'hex');
    const wallet = {
        publicKey,
        address,
        sign: sinon.stub().returns(signResult),
        verify: sinon.stub().returns(true)
    };

    const provider = NetworkWalletFactory.provide({ wallet, enableWallet: true, networkPrefix: config.addressPrefix });
    const message = b4a.from('00112233', 'hex');
    const signature = provider.sign(message);

    t.alike(provider.publicKey, publicKey);
    t.is(provider.address, address);
    t.alike(signature, signResult);
    t.ok(wallet.sign.calledOnceWithExactly(message));

    const verifyResult = provider.verify(signature, message);
    t.ok(verifyResult);
    t.ok(wallet.verify.calledOnceWithExactly(signature, message));

    sinon.restore();
});

test('NetworkWalletFactory.provide requires both public and secret keys when wallet disabled', async t => {
    const publicKey = b4a.from(testKeyPair1.publicKey, 'hex');
    await t.exception(
        () => NetworkWalletFactory.provide({ enableWallet: false, keyPair: { publicKey }, networkPrefix: config.addressPrefix }),
        errorMessageIncludes('keyPair with publicKey and secretKey is required')
    );
});

test('NetworkWalletFactory.provide rejects non-buffer inputs', async t => {
    const secretKey = b4a.from(testKeyPair1.secretKey, 'hex');
    await t.exception(
        () =>
            NetworkWalletFactory.provide({
                enableWallet: false,
                keyPair: { publicKey: 'not-a-buffer', secretKey },
                networkPrefix: config.addressPrefix
            }),
        errorMessageIncludes('must be a Buffer')
    );
});

test('NetworkWalletFactory.provide propagates invalid public key length errors', async t => {
    const secretKey = b4a.from(testKeyPair1.secretKey, 'hex');
    const invalidPublicKey = b4a.alloc(10);

    await t.exception(
        () =>
            NetworkWalletFactory.provide({
                enableWallet: false,
                keyPair: { publicKey: invalidPublicKey, secretKey },
                networkPrefix: config.addressPrefix
            }),
        errorMessageIncludes('Invalid public key')
    );
});

test('NetworkWalletFactory.provide derives address and signs payloads from keyPair', async t => {
    const keyPair = {
        publicKey: b4a.from(testKeyPair1.publicKey, 'hex'),
        secretKey: b4a.from(testKeyPair1.secretKey, 'hex')
    };
    const provider = NetworkWalletFactory.provide({
        enableWallet: false,
        keyPair,
        networkPrefix: config.addressPrefix
    });
    const message = b4a.from('123455555', 'hex');
    const signature = provider.sign(message);

    t.is(
        provider.address,
        PeerWallet.encodeBech32m(config.addressPrefix, provider.publicKey)
    );
    t.ok(PeerWallet.verify(signature, message, provider.publicKey));
    t.ok(provider.verify(signature, message));
});

test('NetworkWalletFactory handles falsy address derivation results', async t => {
    const keyPair = {
        publicKey: b4a.from(testKeyPair1.publicKey, 'hex'),
        secretKey: b4a.from(testKeyPair1.secretKey, 'hex')
    };

    const stub = sinon.stub(PeerWallet, 'encodeBech32m').returns(null);
    await t.exception(
        () =>
            NetworkWalletFactory.provide({
                enableWallet: false,
                keyPair,
                networkPrefix: config.addressPrefix
            }),
        errorMessageIncludes('failed to derive address')
    );
    stub.restore();
    sinon.restore();
});

test('NetworkWalletFactory propagates encoder exceptions', async t => {
    const keyPair = {
        publicKey: b4a.from(testKeyPair1.publicKey, 'hex'),
        secretKey: b4a.from(testKeyPair1.secretKey, 'hex')
    };

    const stub = sinon.stub(PeerWallet, 'encodeBech32m').throws(new Error('test exception'));
    await t.exception(
        () =>
            NetworkWalletFactory.provide({
                enableWallet: false,
                keyPair,
                networkPrefix: config.addressPrefix
            }),
        errorMessageIncludes('test exception')
    );
    stub.restore();
    sinon.restore();
});

test('EphemeralWallet exposes wallet like interface', async t => {
    const keyPair = {
        publicKey: b4a.from(testKeyPair1.publicKey, 'hex'),
        secretKey: b4a.from(testKeyPair1.secretKey, 'hex')
    };
    const wallet = new EphemeralWallet(keyPair, config.addressPrefix);
    const message = b4a.from('feedface', 'hex');
    const signature = wallet.sign(message);

    t.alike(wallet.publicKey, keyPair.publicKey);
    t.is(wallet.address, PeerWallet.encodeBech32m(config.addressPrefix, keyPair.publicKey));
    t.ok(PeerWallet.verify(signature, message, wallet.publicKey));
    t.ok(wallet.verify(signature, message));
});

test('EphemeralWallet requires both public and secret keys', async t => {
    const publicKey = b4a.from(testKeyPair1.publicKey, 'hex');
    await t.exception(
        () => new EphemeralWallet({ publicKey }, config.addressPrefix),
        errorMessageIncludes('keyPair with publicKey and secretKey is required')
    );
});

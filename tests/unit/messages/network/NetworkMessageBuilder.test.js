import { test } from 'brittle';
import b4a from 'b4a';
import PeerWallet from 'trac-wallet';
import { TRAC_NETWORK_MSB_MAINNET_PREFIX } from 'trac-wallet/constants.js';
import { v7 as uuidv7 } from 'uuid';
import NetworkWalletFactory from '../../../../src/core/network/identity/NetworkWalletFactory.js';
import NetworkMessageBuilder from '../../../../src/messages/network/v1/NetworkMessageBuilder.js';
import {
    NetworkOperationType,
    ResultCode as NetworkResultCode
} from '../../../../src/utils/constants.js';
import { decodeV1networkOperation, encodeV1networkOperation } from '../../../../src/utils/protobuf/operationHelpers.js';
import { errorMessageIncludes } from '../../../helpers/regexHelper.js';
import {
    createMessage,
    encodeCapabilities,
    safeWriteUInt32BE,
    idToBuffer,
    timestampToBuffer
} from '../../../../src/utils/buffer.js';
import { addressToBuffer } from '../../../../src/core/state/utils/address.js';
import { config } from '../../../helpers/config.js';
import { testKeyPair1 } from '../../../fixtures/apply.fixtures.js';

function createWallet() {
    const keyPair = {
        publicKey: b4a.from(testKeyPair1.publicKey, 'hex'),
        secretKey: b4a.from(testKeyPair1.secretKey, 'hex')
    };
    return NetworkWalletFactory.provide({
        enableWallet: false,
        keyPair,
        networkPrefix: TRAC_NETWORK_MSB_MAINNET_PREFIX
    });
}

function uniqueResultCodes() {
    return [...new Set(Object.values(NetworkResultCode))].sort((a, b) => a - b);
}

test('NetworkMessageBuilder builds validator connection request and verifies signature', async t => {
    const wallet = createWallet();
    const builder = new NetworkMessageBuilder(wallet, config);

    const id = uuidv7();
    const caps = ['cap:b', 'cap:a'];

    await builder
        .setType(NetworkOperationType.VALIDATOR_CONNECTION_REQUEST)
        .setId(id)
        .setTimestamp()
        .setIssuerAddress(wallet.address)
        .setCapabilities(caps)
        .buildPayload();

    const payload = builder.getResult();
    t.is(payload.type, NetworkOperationType.VALIDATOR_CONNECTION_REQUEST);
    t.is(payload.id, id);
    t.alike(payload.capabilities, caps);
    t.ok(b4a.isBuffer(payload.validator_connection_request.nonce));
    t.ok(b4a.isBuffer(payload.validator_connection_request.signature));

    const message = createMessage(
        payload.type,
        idToBuffer(id),
        timestampToBuffer(payload.timestamp),
        addressToBuffer(wallet.address, config.addressPrefix),
        payload.validator_connection_request.nonce,
        encodeCapabilities(caps)
    );
    const hash = await PeerWallet.blake3(message);
    t.ok(wallet.verify(payload.validator_connection_request.signature, hash, wallet.publicKey));

    const roundTrip = decodeV1networkOperation(encodeV1networkOperation(payload));
    t.is(roundTrip.type, NetworkOperationType.VALIDATOR_CONNECTION_REQUEST);
});

test('NetworkMessageBuilder iterates validator connection response ResultCode values', async t => {
    const wallet = createWallet();
    const builder = new NetworkMessageBuilder(wallet, config);
    const id = uuidv7();
    const otherAddress = 'trac1xm76l9qaujh7vqktk8302mw9sfrxau3l45w62hqfl4kasswt6yts0autkh';
    const caps = ['cap:b', 'cap:a'];

    for (const code of uniqueResultCodes()) {
        await builder
            .setType(NetworkOperationType.VALIDATOR_CONNECTION_RESPONSE)
            .setId(id)
            .setTimestamp()
            .setIssuerAddress(otherAddress)
            .setCapabilities(caps)
            .setResultCode(code)
            .buildPayload();

        const payload = builder.getResult();
        t.is(payload.type, NetworkOperationType.VALIDATOR_CONNECTION_RESPONSE);
        t.is(payload.validator_connection_response.result, code);

        const msg = createMessage(
            payload.type,
            idToBuffer(payload.id),
            timestampToBuffer(payload.timestamp),
            addressToBuffer(otherAddress, config.addressPrefix),
            payload.validator_connection_response.nonce,
            safeWriteUInt32BE(code, 0),
            encodeCapabilities(caps)
        );
        const hash = await PeerWallet.blake3(msg);
        t.ok(wallet.verify(payload.validator_connection_response.signature, hash, wallet.publicKey));

        const decoded = decodeV1networkOperation(encodeV1networkOperation(payload));
        t.is(decoded.validator_connection_response.result, code);
    }
});

test('NetworkMessageBuilder iterates liveness response ResultCode values', async t => {
    const wallet = createWallet();
    const builder = new NetworkMessageBuilder(wallet, config);
    const id = uuidv7();
    const caps = ['cap:b', 'cap:a'];
    const data = b4a.from('ping', 'utf8');

    for (const code of uniqueResultCodes()) {
        await builder
            .setType(NetworkOperationType.LIVENESS_RESPONSE)
            .setId(id)
            .setTimestamp()
            .setData(data)
            .setCapabilities(caps)
            .setResultCode(code)
            .buildPayload();

        const payload = builder.getResult();
        t.is(payload.type, NetworkOperationType.LIVENESS_RESPONSE);
        t.is(payload.liveness_response.result, code);

        const msg = createMessage(
            payload.type,
            idToBuffer(payload.id),
            timestampToBuffer(payload.timestamp),
            payload.liveness_response.nonce,
            safeWriteUInt32BE(code, 0),
            encodeCapabilities(caps)
        );
        const hash = await PeerWallet.blake3(msg);
        t.ok(wallet.verify(payload.liveness_response.signature, hash, wallet.publicKey));

        const decoded = decodeV1networkOperation(encodeV1networkOperation(payload));
        t.is(decoded.liveness_response.result, code);
    }
});

test('NetworkMessageBuilder builds liveness request and verifies signature (data not signed)', async t => {
    const wallet = createWallet();
    const builder = new NetworkMessageBuilder(wallet, config);

    const id = uuidv7();
    const caps = ['cap:b', 'cap:a'];
    const data = b4a.from('ping', 'utf8');

    await builder
        .setType(NetworkOperationType.LIVENESS_REQUEST)
        .setId(id)
        .setTimestamp()
        .setData(data)
        .setCapabilities(caps)
        .buildPayload();

    const payload = builder.getResult();
    t.is(payload.type, NetworkOperationType.LIVENESS_REQUEST);
    t.ok(b4a.isBuffer(payload.liveness_request.nonce));
    t.ok(b4a.isBuffer(payload.liveness_request.signature));

    const msg = createMessage(
        payload.type,
        idToBuffer(payload.id),
        timestampToBuffer(payload.timestamp),
        payload.liveness_request.nonce,
        encodeCapabilities(caps)
    );
    const hash = await PeerWallet.blake3(msg);
    t.ok(wallet.verify(payload.liveness_request.signature, hash, wallet.publicKey));
});

test('NetworkMessageBuilder iterates broadcast transaction response ResultCode values', async t => {
    const wallet = createWallet();
    const builder = new NetworkMessageBuilder(wallet, config);
    const id = uuidv7();
    const caps = ['cap:b', 'cap:a'];

    for (const code of uniqueResultCodes()) {
        await builder
            .setType(NetworkOperationType.BROADCAST_TRANSACTION_RESPONSE)
            .setId(id)
            .setTimestamp()
            .setCapabilities(caps)
            .setResultCode(code)
            .buildPayload();

        const payload = builder.getResult();
        t.is(payload.type, NetworkOperationType.BROADCAST_TRANSACTION_RESPONSE);
        t.is(payload.broadcast_transaction_response.result, code);

        const msg = createMessage(
            payload.type,
            idToBuffer(payload.id),
            timestampToBuffer(payload.timestamp),
            payload.broadcast_transaction_response.nonce,
            safeWriteUInt32BE(code, 0),
            encodeCapabilities(caps)
        );
        const hash = await PeerWallet.blake3(msg);
        t.ok(wallet.verify(payload.broadcast_transaction_response.signature, hash, wallet.publicKey));

        const decoded = decodeV1networkOperation(encodeV1networkOperation(payload));
        t.is(decoded.broadcast_transaction_response.result, code);
    }
});

test('NetworkMessageBuilder builds broadcast transaction request and verifies signature', async t => {
    const wallet = createWallet();
    const builder = new NetworkMessageBuilder(wallet, config);

    const id = uuidv7();
    const caps = ['cap:b', 'cap:a'];
    const data = b4a.from('deadbeef', 'hex');

    await builder
        .setType(NetworkOperationType.BROADCAST_TRANSACTION_REQUEST)
        .setId(id)
        .setTimestamp()
        .setData(data)
        .setCapabilities(caps)
        .buildPayload();

    const payload = builder.getResult();
    t.is(payload.type, NetworkOperationType.BROADCAST_TRANSACTION_REQUEST);
    t.alike(payload.broadcast_transaction_request.data, data);

    const msg = createMessage(
        payload.type,
        idToBuffer(payload.id),
        timestampToBuffer(payload.timestamp),
        data,
        payload.broadcast_transaction_request.nonce,
        encodeCapabilities(caps)
    );
    const hash = await PeerWallet.blake3(msg);
    t.ok(wallet.verify(payload.broadcast_transaction_request.signature, hash, wallet.publicKey));
});

test('NetworkMessageBuilder validates required inputs', async t => {
    const wallet = createWallet();
    const builder = new NetworkMessageBuilder(wallet, config);
    const id = uuidv7();
    await t.exception(
        () => builder.setType(undefined),
        errorMessageIncludes('Invalid operation type')
    );

    await t.exception(
        () => builder.setCapabilities('not-an-array'),
        errorMessageIncludes('Capabilities must be a string array.')
    );

    await t.exception(
        () =>
            builder
                .setType(NetworkOperationType.BROADCAST_TRANSACTION_REQUEST)
                .setId(id)
                .setTimestamp()
                .setCapabilities([])
                .buildPayload(),
        errorMessageIncludes('Data must be set before building broadcast transaction request')
    );
});

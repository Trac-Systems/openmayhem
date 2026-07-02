import { test } from 'brittle';
import b4a from 'b4a';
import PeerWallet from 'trac-wallet';
import { TRAC_NETWORK_MSB_MAINNET_PREFIX } from 'trac-wallet/constants.js';

import NetworkWalletFactory from '../../../../src/core/network/identity/NetworkWalletFactory.js';
import NetworkMessageDirector from '../../../../src/messages/network/v1/NetworkMessageDirector.js';
import NetworkMessageBuilder from '../../../../src/messages/network/v1/NetworkMessageBuilder.js';
import { NetworkOperationType, ResultCode as NetworkResultCode } from '../../../../src/utils/constants.js';
import { decodeV1networkOperation, encodeV1networkOperation } from '../../../../src/utils/protobuf/operationHelpers.js';
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

test('NetworkMessageDirector builds validator connection request and verifies signature', async t => {
    const wallet = createWallet();
    const director = new NetworkMessageDirector(new NetworkMessageBuilder(wallet, config));

    const id = '1';
    const caps = ['cap:b', 'cap:a'];

    const payload = await director.buildValidatorConnectionRequest(id, wallet.address, caps);
    t.is(payload.type, NetworkOperationType.VALIDATOR_CONNECTION_REQUEST);
    t.is(payload.id, id);
    t.alike(payload.capabilities, caps);

    const msg = createMessage(
        payload.type,
        idToBuffer(payload.id),
        timestampToBuffer(payload.timestamp),
        addressToBuffer(wallet.address, config.addressPrefix),
        payload.validator_connection_request.nonce,
        encodeCapabilities(caps)
    );
    const hash = await PeerWallet.blake3(msg);
    t.ok(wallet.verify(payload.validator_connection_request.signature, hash, wallet.publicKey));
});

test('NetworkMessageDirector builds liveness request and verifies signature', async t => {
    const wallet = createWallet();
    const director = new NetworkMessageDirector(new NetworkMessageBuilder(wallet, config));

    const id = '1';
    const caps = ['cap:b', 'cap:a'];
    const data = b4a.from('ping', 'utf8');

    const payload = await director.buildLivenessRequest(id, data, caps);
    t.is(payload.type, NetworkOperationType.LIVENESS_REQUEST);
    t.is(payload.id, id);
    t.alike(payload.capabilities, caps);

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

test('NetworkMessageDirector iterates liveness response ResultCode values', async t => {
    const wallet = createWallet();
    const director = new NetworkMessageDirector(new NetworkMessageBuilder(wallet, config));

    const id = '1';
    const caps = ['cap:b', 'cap:a'];
    const data = b4a.from('ping', 'utf8');

    for (const code of uniqueResultCodes()) {
        const payload = await director.buildLivenessResponse(id, data, caps, code);
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

test('NetworkMessageDirector builds broadcast transaction request and verifies signature', async t => {
    const wallet = createWallet();
    const director = new NetworkMessageDirector(new NetworkMessageBuilder(wallet, config));

    const id = '1';
    const data = b4a.from('deadbeef', 'hex');
    const caps = ['cap:b', 'cap:a'];

    const payload = await director.buildBroadcastTransactionRequest(id, data, caps);
    t.is(payload.type, NetworkOperationType.BROADCAST_TRANSACTION_REQUEST);
    t.is(payload.id, id);
    t.alike(payload.capabilities, caps);
    t.alike(payload.broadcast_transaction_request.data, data);

    const message = createMessage(
        payload.type,
        idToBuffer(id),
        timestampToBuffer(payload.timestamp),
        data,
        payload.broadcast_transaction_request.nonce,
        encodeCapabilities(caps)
    );
    const hash = await PeerWallet.blake3(message);
    t.ok(wallet.verify(payload.broadcast_transaction_request.signature, hash, wallet.publicKey));

    const decoded = decodeV1networkOperation(encodeV1networkOperation(payload));
    t.is(decoded.type, NetworkOperationType.BROADCAST_TRANSACTION_REQUEST);
});

test('NetworkMessageDirector iterates broadcast transaction response ResultCode values', async t => {
    const wallet = createWallet();
    const director = new NetworkMessageDirector(new NetworkMessageBuilder(wallet, config));

    const id = '1';
    const caps = ['cap:b', 'cap:a'];

    for (const code of uniqueResultCodes()) {
        const payload = await director.buildBroadcastTransactionResponse(id, caps, code);
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

test('NetworkMessageDirector iterates validator connection response ResultCode values', async t => {
    const wallet = createWallet();
    const director = new NetworkMessageDirector(new NetworkMessageBuilder(wallet, config));

    const id = '1';
    const caps = ['cap:b', 'cap:a'];
    const otherAddress =
        'trac1xm76l9qaujh7vqktk8302mw9sfrxau3l45w62hqfl4kasswt6yts0autkh';

    for (const code of uniqueResultCodes()) {
        const payload = await director.buildValidatorConnectionResponse(
            id,
            otherAddress,
            caps,
            code
        );
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
    }
});

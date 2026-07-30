import { test } from 'brittle';
import b4a from 'b4a';
import tracCryptoApi from 'trac-crypto-api';
import { v7 as uuidv7 } from 'uuid';
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
import { config } from '../../../helpers/config.js';
import { testKeyPair1 } from '../../../fixtures/apply.fixtures.js';
import { WalletProvider } from 'trac-wallet';

async function createWallet() {
    return await new WalletProvider(config).fromSecretKey(testKeyPair1.secretKey)
}

function uniqueResultCodes() {
    return [...new Set(Object.values(NetworkResultCode))].sort((a, b) => a - b);
}

test('NetworkMessageBuilder iterates liveness response ResultCode values', async t => {
    const wallet = await createWallet();
    const builder = new NetworkMessageBuilder(wallet, config);
    const id = uuidv7();
    const caps = ['cap:b', 'cap:a'];
    for (const code of uniqueResultCodes()) {
        await builder
            .setType(NetworkOperationType.LIVENESS_RESPONSE)
            .setId(id)
            .setTimestamp()
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
        const hash = await tracCryptoApi.hash.blake3(msg);
        t.ok(wallet.verify(payload.liveness_response.signature, hash, wallet.publicKey));

        const decoded = decodeV1networkOperation(encodeV1networkOperation(payload));
        t.is(decoded.liveness_response.result, code);
    }
});

test('NetworkMessageBuilder builds liveness request and verifies signature (data not signed)', async t => {
    const wallet = await createWallet();
    const builder = new NetworkMessageBuilder(wallet, config);

    const id = uuidv7();
    const caps = ['cap:b', 'cap:a'];

    await builder
        .setType(NetworkOperationType.LIVENESS_REQUEST)
        .setId(id)
        .setTimestamp()
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
    const hash = await tracCryptoApi.hash.blake3(msg);
    t.ok(wallet.verify(payload.liveness_request.signature, hash, wallet.publicKey));
});

test('NetworkMessageBuilder iterates broadcast transaction response ResultCode values', async t => {
    const wallet = await createWallet();
    const builder = new NetworkMessageBuilder(wallet, config);
    const id = uuidv7();
    const caps = ['cap:b', 'cap:a'];
    const proof = b4a.from('deadbeef', 'hex');
    const timestamp = Date.now();
    const emptyProof = b4a.alloc(0);

    for (const code of uniqueResultCodes()) {
        const includeProof = code === NetworkResultCode.OK;
        const proofUnavailable = code === NetworkResultCode.TX_ACCEPTED_PROOF_UNAVAILABLE;
        const responseProof = includeProof ? proof : emptyProof;
        const responseTimestampLedger = includeProof || proofUnavailable ? timestamp : 0;
        await builder
            .setType(NetworkOperationType.BROADCAST_TRANSACTION_RESPONSE)
            .setId(id)
            .setTimestamp()
            .setCapabilities(caps)
            .setProof(responseProof)
            .setTimestampLedger(responseTimestampLedger)
            .setResultCode(code)
            .buildPayload();

        const payload = builder.getResult();
        t.is(payload.type, NetworkOperationType.BROADCAST_TRANSACTION_RESPONSE);
        t.is(payload.broadcast_transaction_response.result, code);
        t.alike(payload.broadcast_transaction_response.proof, responseProof);
        t.is(payload.broadcast_transaction_response.timestamp, responseTimestampLedger);

        const msg = createMessage(
            payload.type,
            idToBuffer(payload.id),
            timestampToBuffer(payload.timestamp),
            payload.broadcast_transaction_response.nonce,
            responseProof,
            timestampToBuffer(responseTimestampLedger),
            safeWriteUInt32BE(code, 0),
            encodeCapabilities(caps)
        );

        const hash = await tracCryptoApi.hash.blake3(msg);
        t.ok(wallet.verify(payload.broadcast_transaction_response.signature, hash, wallet.publicKey));

        const decoded = decodeV1networkOperation(encodeV1networkOperation(payload));
        t.is(decoded.broadcast_transaction_response.result, code);
        t.alike(decoded.broadcast_transaction_response.proof, responseProof);
        t.is(decoded.broadcast_transaction_response.timestamp, responseTimestampLedger);
    }
});

test('NetworkMessageBuilder builds broadcast transaction response with proof and timestamp', async t => {
    const wallet = await createWallet();
    const builder = new NetworkMessageBuilder(wallet, config);
    const id = uuidv7();
    const caps = ['cap:b', 'cap:a'];
    const proof = b4a.from('deadbeef', 'hex');
    const timestamp = Date.now();

    await builder
        .setType(NetworkOperationType.BROADCAST_TRANSACTION_RESPONSE)
        .setId(id)
        .setTimestamp()
        .setCapabilities(caps)
        .setProof(proof)
        .setTimestampLedger(timestamp)
        .setResultCode(NetworkResultCode.OK)
        .buildPayload();

    const payload = builder.getResult();
    t.alike(payload.broadcast_transaction_response.proof, proof);
    t.is(payload.broadcast_transaction_response.timestamp, timestamp);

    const msg = createMessage(
        payload.type,
        idToBuffer(payload.id),
        timestampToBuffer(payload.timestamp),
        payload.broadcast_transaction_response.nonce,
        proof,
        timestampToBuffer(timestamp),
        safeWriteUInt32BE(NetworkResultCode.OK, 0),
        encodeCapabilities(caps)
    );
    const hash = await tracCryptoApi.hash.blake3(msg);
    t.ok(wallet.verify(payload.broadcast_transaction_response.signature, hash, wallet.publicKey));

    const decoded = decodeV1networkOperation(encodeV1networkOperation(payload));
    t.alike(decoded.broadcast_transaction_response.proof, proof);
    t.is(decoded.broadcast_transaction_response.timestamp, timestamp);
});

test('NetworkMessageBuilder rejects OK response when proof is provided without timestamp', async t => {
    const wallet = await createWallet();
    const builder = new NetworkMessageBuilder(wallet, config);
    const id = uuidv7();
    const caps = ['cap:b', 'cap:a'];
    const proof = b4a.from('deadbeef', 'hex');

    await t.exception(
        () =>
            builder
                .setType(NetworkOperationType.BROADCAST_TRANSACTION_RESPONSE)
                .setId(id)
                .setTimestamp()
                .setCapabilities(caps)
                .setProof(proof)
                .setResultCode(NetworkResultCode.OK)
                .buildPayload(),
        errorMessageIncludes('Result code OK requires non-empty proof and timestamp > 0.')
    );
});

test('NetworkMessageBuilder rejects OK response when timestamp is provided without proof', async t => {
    const wallet = await createWallet();
    const builder = new NetworkMessageBuilder(wallet, config);
    const id = uuidv7();
    const caps = ['cap:b', 'cap:a'];
    const timestamp = Date.now();

    await t.exception(
        () =>
            builder
                .setType(NetworkOperationType.BROADCAST_TRANSACTION_RESPONSE)
                .setId(id)
                .setTimestamp()
                .setCapabilities(caps)
                .setTimestampLedger(timestamp)
                .setResultCode(NetworkResultCode.OK)
                .buildPayload(),
        errorMessageIncludes('Result code OK requires non-empty proof and timestamp > 0.')
    );
});

test('NetworkMessageBuilder allows TX_ACCEPTED_PROOF_UNAVAILABLE response with timestamp and empty proof', async t => {
    const wallet = await createWallet();
    const builder = new NetworkMessageBuilder(wallet, config);
    const id = uuidv7();
    const caps = ['cap:b', 'cap:a'];
    const timestamp = Date.now();
    const emptyProof = b4a.alloc(0);

    await builder
        .setType(NetworkOperationType.BROADCAST_TRANSACTION_RESPONSE)
        .setId(id)
        .setTimestamp()
        .setCapabilities(caps)
        .setProof(emptyProof)
        .setTimestampLedger(timestamp)
        .setResultCode(NetworkResultCode.TX_ACCEPTED_PROOF_UNAVAILABLE)
        .buildPayload();

    const payload = builder.getResult();
    t.alike(payload.broadcast_transaction_response.proof, emptyProof);
    t.is(payload.broadcast_transaction_response.timestamp, timestamp);
    t.is(payload.broadcast_transaction_response.result, NetworkResultCode.TX_ACCEPTED_PROOF_UNAVAILABLE);

    const msg = createMessage(
        payload.type,
        idToBuffer(payload.id),
        timestampToBuffer(payload.timestamp),
        payload.broadcast_transaction_response.nonce,
        emptyProof,
        timestampToBuffer(timestamp),
        safeWriteUInt32BE(NetworkResultCode.TX_ACCEPTED_PROOF_UNAVAILABLE, 0),
        encodeCapabilities(caps)
    );
    const hash = await tracCryptoApi.hash.blake3(msg);
    t.ok(wallet.verify(payload.broadcast_transaction_response.signature, hash, wallet.publicKey));

    const decoded = decodeV1networkOperation(encodeV1networkOperation(payload));
    t.is(decoded.broadcast_transaction_response.timestamp, timestamp);
    t.is(decoded.broadcast_transaction_response.result, NetworkResultCode.TX_ACCEPTED_PROOF_UNAVAILABLE);
});

test('NetworkMessageBuilder rejects OK response when proof and timestamp are both missing', async t => {
    const wallet = await createWallet();
    const builder = new NetworkMessageBuilder(wallet, config);
    const id = uuidv7();
    const caps = ['cap:b', 'cap:a'];

    await t.exception(
        () =>
            builder
                .setType(NetworkOperationType.BROADCAST_TRANSACTION_RESPONSE)
                .setId(id)
                .setTimestamp()
                .setCapabilities(caps)
                .setResultCode(NetworkResultCode.OK)
                .buildPayload(),
        errorMessageIncludes('Result code OK requires non-empty proof and timestamp > 0.')
    );
});

test('NetworkMessageBuilder rejects TX_ACCEPTED_PROOF_UNAVAILABLE response when timestamp is missing', async t => {
    const wallet = await createWallet();
    const builder = new NetworkMessageBuilder(wallet, config);
    const id = uuidv7();
    const caps = ['cap:b', 'cap:a'];

    await t.exception(
        () =>
            builder
                .setType(NetworkOperationType.BROADCAST_TRANSACTION_RESPONSE)
                .setId(id)
                .setTimestamp()
                .setCapabilities(caps)
                .setResultCode(NetworkResultCode.TX_ACCEPTED_PROOF_UNAVAILABLE)
                .buildPayload(),
        errorMessageIncludes('Result code TX_ACCEPTED_PROOF_UNAVAILABLE requires timestamp > 0.')
    );
});

test('NetworkMessageBuilder rejects TX_ACCEPTED_PROOF_UNAVAILABLE response when proof is non-empty', async t => {
    const wallet = await createWallet();
    const builder = new NetworkMessageBuilder(wallet, config);
    const id = uuidv7();
    const caps = ['cap:b', 'cap:a'];

    await t.exception(
        () =>
            builder
                .setType(NetworkOperationType.BROADCAST_TRANSACTION_RESPONSE)
                .setId(id)
                .setTimestamp()
                .setCapabilities(caps)
                .setProof(b4a.from('deadbeef', 'hex'))
                .setTimestampLedger(Date.now())
                .setResultCode(NetworkResultCode.TX_ACCEPTED_PROOF_UNAVAILABLE)
                .buildPayload(),
        errorMessageIncludes('Result code TX_ACCEPTED_PROOF_UNAVAILABLE requires empty proof.')
    );
});

test('NetworkMessageBuilder rejects non-OK response when proof is non-empty', async t => {
    const wallet = await createWallet();
    const builder = new NetworkMessageBuilder(wallet, config);
    const id = uuidv7();
    const caps = ['cap:b', 'cap:a'];

    await t.exception(
        () =>
            builder
                .setType(NetworkOperationType.BROADCAST_TRANSACTION_RESPONSE)
                .setId(id)
                .setTimestamp()
                .setCapabilities(caps)
                .setProof(b4a.from('deadbeef', 'hex'))
                .setTimestampLedger(0)
                .setResultCode(NetworkResultCode.INVALID_PAYLOAD)
                .buildPayload(),
        errorMessageIncludes('Non-OK result code requires empty proof.')
    );
});

test('NetworkMessageBuilder rejects non-OK response with timestamp > 0 unless proof is unavailable', async t => {
    const wallet = await createWallet();
    const builder = new NetworkMessageBuilder(wallet, config);
    const id = uuidv7();
    const caps = ['cap:b', 'cap:a'];

    await t.exception(
        () =>
            builder
                .setType(NetworkOperationType.BROADCAST_TRANSACTION_RESPONSE)
                .setId(id)
                .setTimestamp()
                .setCapabilities(caps)
                .setTimestampLedger(Date.now())
                .setResultCode(NetworkResultCode.INVALID_PAYLOAD)
                .buildPayload(),
        errorMessageIncludes('Non-OK result code requires timestamp to be 0, except TX_ACCEPTED_PROOF_UNAVAILABLE.')
    );
});

test('NetworkMessageBuilder builds broadcast transaction request and verifies signature', async t => {
    const wallet = await createWallet();
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
    const hash = await tracCryptoApi.hash.blake3(msg);
    t.ok(wallet.verify(payload.broadcast_transaction_request.signature, hash, wallet.publicKey));
});

test('NetworkMessageBuilder validates required inputs', async t => {
    const wallet = await createWallet();
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

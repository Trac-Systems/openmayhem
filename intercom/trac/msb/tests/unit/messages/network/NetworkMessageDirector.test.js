import { test } from 'brittle';
import b4a from 'b4a';
import tracCryptoApi from 'trac-crypto-api';
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
import { config } from '../../../helpers/config.js';
import { testKeyPair1 } from '../../../fixtures/apply.fixtures.js';
import { v7 as uuidv7 } from 'uuid';
import { errorMessageIncludes } from '../../../helpers/regexHelper.js';
import { WalletProvider } from 'trac-wallet';

async function createWallet() {
    return await new WalletProvider(config).fromSecretKey(testKeyPair1.secretKey)
}

function uniqueResultCodes() {
    return [...new Set(Object.values(NetworkResultCode))].sort((a, b) => a - b);
}

test('NetworkMessageDirector iterates liveness response ResultCode values', async t => {
    const wallet = await createWallet();
    const director = new NetworkMessageDirector(new NetworkMessageBuilder(wallet, config));

    const id = uuidv7();
    const caps = ['cap:b', 'cap:a'];

    for (const code of uniqueResultCodes()) {
        const payload = await director.buildLivenessResponse(id, caps, code);
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

test('NetworkMessageDirector builds broadcast transaction request and verifies signature', async t => {
    const wallet = await createWallet();
    const director = new NetworkMessageDirector(new NetworkMessageBuilder(wallet, config));

    const id = uuidv7();
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
    const hash = await tracCryptoApi.hash.blake3(message);
    t.ok(wallet.verify(payload.broadcast_transaction_request.signature, hash, wallet.publicKey));

    const decoded = decodeV1networkOperation(encodeV1networkOperation(payload));
    t.is(decoded.type, NetworkOperationType.BROADCAST_TRANSACTION_REQUEST);
});

test('NetworkMessageDirector iterates broadcast transaction response ResultCode values', async t => {
    const wallet = await createWallet();
    const director = new NetworkMessageDirector(new NetworkMessageBuilder(wallet, config));

    const id = uuidv7();
    const caps = ['cap:b', 'cap:a'];
    const proof = b4a.from('deadbeef', 'hex');
    const timestamp = Date.now();
    const emptyProof = b4a.alloc(0);

    for (const code of uniqueResultCodes()) {
        const includeProof = code === NetworkResultCode.OK;
        const proofUnavailable = code === NetworkResultCode.TX_ACCEPTED_PROOF_UNAVAILABLE;
        const responseProof = includeProof ? proof : emptyProof;
        const responseTimestamp = includeProof || proofUnavailable ? timestamp : 0;
        const payload = await director.buildBroadcastTransactionResponse(
            id,
            caps,
            code,
            responseProof,
            responseTimestamp
        );
        t.is(payload.type, NetworkOperationType.BROADCAST_TRANSACTION_RESPONSE);
        t.is(payload.broadcast_transaction_response.result, code);
        t.alike(payload.broadcast_transaction_response.proof, responseProof);
        t.is(payload.broadcast_transaction_response.timestamp, responseTimestamp);

        const msg = createMessage(
            payload.type,
            idToBuffer(payload.id),
            timestampToBuffer(payload.timestamp),
            payload.broadcast_transaction_response.nonce,
            responseProof,
            timestampToBuffer(responseTimestamp),
            safeWriteUInt32BE(code, 0),
            encodeCapabilities(caps)
        );
        const hash = await tracCryptoApi.hash.blake3(msg);
        t.ok(wallet.verify(payload.broadcast_transaction_response.signature, hash, wallet.publicKey));

        const decoded = decodeV1networkOperation(encodeV1networkOperation(payload));
        t.is(decoded.broadcast_transaction_response.result, code);
        t.alike(decoded.broadcast_transaction_response.proof, responseProof);
        t.is(decoded.broadcast_transaction_response.timestamp, responseTimestamp);
    }
});

test('NetworkMessageDirector builds broadcast transaction response with proof and timestamp', async t => {
    const wallet = await createWallet();
    const director = new NetworkMessageDirector(new NetworkMessageBuilder(wallet, config));

    const id = uuidv7();
    const caps = ['cap:b', 'cap:a'];
    const proof = b4a.from('deadbeef', 'hex');
    const timestamp = Date.now();

    const payload = await director.buildBroadcastTransactionResponse(
        id,
        caps,
        NetworkResultCode.OK,
        proof,
        timestamp
    );

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

test('NetworkMessageDirector rejects OK response when proof is provided without timestamp', async t => {
    const wallet = await createWallet();
    const director = new NetworkMessageDirector(new NetworkMessageBuilder(wallet, config));

    const id = uuidv7();
    const caps = ['cap:b', 'cap:a'];
    const proof = b4a.from('deadbeef', 'hex');

    await t.exception(
        () => director.buildBroadcastTransactionResponse(
            id,
            caps,
            NetworkResultCode.OK,
            proof
        ),
        errorMessageIncludes('Result code OK requires non-empty proof and timestamp > 0.')
    );
});

test('NetworkMessageDirector rejects OK response when timestamp is provided without proof', async t => {
    const wallet = await createWallet();
    const director = new NetworkMessageDirector(new NetworkMessageBuilder(wallet, config));

    const id = uuidv7();
    const caps = ['cap:b', 'cap:a'];
    const timestamp = Date.now();

    await t.exception(
        () => director.buildBroadcastTransactionResponse(
            id,
            caps,
            NetworkResultCode.OK,
            null,
            timestamp
        ),
        errorMessageIncludes('Result code OK requires non-empty proof and timestamp > 0.')
    );
});

test('NetworkMessageDirector allows TX_ACCEPTED_PROOF_UNAVAILABLE response with timestamp and empty proof', async t => {
    const wallet = await createWallet();
    const director = new NetworkMessageDirector(new NetworkMessageBuilder(wallet, config));

    const id = uuidv7();
    const caps = ['cap:b', 'cap:a'];
    const timestamp = Date.now();
    const emptyProof = b4a.alloc(0);

    const payload = await director.buildBroadcastTransactionResponse(
        id,
        caps,
        NetworkResultCode.TX_ACCEPTED_PROOF_UNAVAILABLE,
        emptyProof,
        timestamp
    );

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

test('NetworkMessageDirector rejects OK response when proof and timestamp are both missing', async t => {
    const wallet = await createWallet();
    const director = new NetworkMessageDirector(new NetworkMessageBuilder(wallet, config));

    const id = uuidv7();
    const caps = ['cap:b', 'cap:a'];

    await t.exception(
        () => director.buildBroadcastTransactionResponse(
            id,
            caps,
            NetworkResultCode.OK
        ),
        errorMessageIncludes('Result code OK requires non-empty proof and timestamp > 0.')
    );
});

test('NetworkMessageDirector rejects TX_ACCEPTED_PROOF_UNAVAILABLE response when timestamp is missing', async t => {
    const wallet = await createWallet();
    const director = new NetworkMessageDirector(new NetworkMessageBuilder(wallet, config));

    const id = uuidv7();
    const caps = ['cap:b', 'cap:a'];

    await t.exception(
        () => director.buildBroadcastTransactionResponse(
            id,
            caps,
            NetworkResultCode.TX_ACCEPTED_PROOF_UNAVAILABLE
        ),
        errorMessageIncludes('Result code TX_ACCEPTED_PROOF_UNAVAILABLE requires timestamp > 0.')
    );
});

test('NetworkMessageDirector rejects TX_ACCEPTED_PROOF_UNAVAILABLE response when proof is non-empty', async t => {
    const wallet = await createWallet();
    const director = new NetworkMessageDirector(new NetworkMessageBuilder(wallet, config));

    const id = uuidv7();
    const caps = ['cap:b', 'cap:a'];

    await t.exception(
        () => director.buildBroadcastTransactionResponse(
            id,
            caps,
            NetworkResultCode.TX_ACCEPTED_PROOF_UNAVAILABLE,
            b4a.from('deadbeef', 'hex'),
            Date.now()
        ),
        errorMessageIncludes('Result code TX_ACCEPTED_PROOF_UNAVAILABLE requires empty proof.')
    );
});

test('NetworkMessageDirector rejects non-OK response when proof is non-empty', async t => {
    const wallet = await createWallet();
    const director = new NetworkMessageDirector(new NetworkMessageBuilder(wallet, config));

    const id = uuidv7();
    const caps = ['cap:b', 'cap:a'];

    await t.exception(
        () => director.buildBroadcastTransactionResponse(
            id,
            caps,
            NetworkResultCode.INVALID_PAYLOAD,
            b4a.from('deadbeef', 'hex'),
            0
        ),
        errorMessageIncludes('Non-OK result code requires empty proof.')
    );
});

test('NetworkMessageDirector rejects non-OK response with timestamp > 0 unless proof is unavailable', async t => {
    const wallet = await createWallet();
    const director = new NetworkMessageDirector(new NetworkMessageBuilder(wallet, config));

    const id = uuidv7();
    const caps = ['cap:b', 'cap:a'];

    await t.exception(
        () => director.buildBroadcastTransactionResponse(
            id,
            caps,
            NetworkResultCode.INVALID_PAYLOAD,
            null,
            Date.now()
        ),
        errorMessageIncludes('Non-OK result code requires timestamp to be 0, except TX_ACCEPTED_PROOF_UNAVAILABLE.')
    );
});

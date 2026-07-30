import test from 'brittle';
import b4a from 'b4a';
import tracCryptoApi from 'trac-crypto-api';
import V1BaseOperation from '../../../../src/core/network/protocols/v1/validators/V1BaseOperation.js';
import NetworkMessageBuilder from '../../../../src/messages/network/v1/NetworkMessageBuilder.js';
import {
    V1ProtocolError
} from '../../../../src/core/network/protocols/v1/V1ProtocolError.js';
import {
    NetworkOperationType,
    ResultCode
} from '../../../../src/utils/constants.js';
import { testKeyPair1, testKeyPair2 } from '../../../fixtures/apply.fixtures.js';
import { config } from '../../../helpers/config.js';
import { errorMessageIncludes } from '../../../helpers/regexHelper.js';
import { WalletProvider } from 'trac-wallet';

async function createWallet(testKeypair = testKeyPair1) {
    return await new WalletProvider(config).fromSecretKey(testKeypair.secretKey)
}

const buildSignedPayload = async (wallet, type, options = {}) => {
    const builder = new NetworkMessageBuilder(wallet, config)
        .setType(type)
        .setId(`id-${type}-${Date.now()}-${Math.random()}`)
        .setTimestamp()
        .setCapabilities(['cap:a']);

    switch (type) {
        case NetworkOperationType.LIVENESS_REQUEST:
            break;
        case NetworkOperationType.LIVENESS_RESPONSE:
            builder.setResultCode(options.resultCode ?? ResultCode.OK);
            break;
        case NetworkOperationType.BROADCAST_TRANSACTION_REQUEST:
            builder.setData(options.data ?? b4a.from('abcd', 'hex'));
            break;
        case NetworkOperationType.BROADCAST_TRANSACTION_RESPONSE:
            builder
                .setResultCode(options.resultCode ?? ResultCode.OK)
                .setProof(options.proof ?? b4a.from('deadbeef', 'hex'))
                .setTimestampLedger(options.timestamp ?? Date.now());
            break;
        default:
            throw new Error(`Unsupported type in test helper: ${type}`);
    }

    await builder.buildPayload();
    return builder.getResult();
};

test('V1BaseOperation.validate throws "must be implemented" by default', async t => {
    const operation = new V1BaseOperation(config);

    await t.exception(
        async () => operation.validate({}, {}, {}),
        errorMessageIncludes('must be implemented')
    );
});

test('V1BaseOperation.isPayloadSchemaValid handles missing/invalid type cases', async t => {
    const operation = new V1BaseOperation(config);

    t.exception(
        () => operation.isPayloadSchemaValid(null),
        errorMessageIncludes('Payload or payload type is missing')
    );

    t.exception(
        () => operation.isPayloadSchemaValid({ type: null }),
        errorMessageIncludes('Payload or payload type is missing')
    );

    t.exception(
        () => operation.isPayloadSchemaValid({ type: 1.5 }),
        errorMessageIncludes('Operation type must be an integer')
    );

    t.exception(
        () => operation.isPayloadSchemaValid({ type: 0 }),
        errorMessageIncludes('Operation type is unspecified')
    );

    t.exception(
        () => operation.isPayloadSchemaValid({ type: 9999 }),
        errorMessageIncludes('Unknown operation type')
    );
});

test('V1BaseOperation.isPayloadSchemaValid accepts all supported message schemas', async t => {
    const operation = new V1BaseOperation(config);
    const wallet = await createWallet();

    const payloads = [
        await buildSignedPayload(wallet, NetworkOperationType.LIVENESS_REQUEST),
        await buildSignedPayload(wallet, NetworkOperationType.LIVENESS_RESPONSE),
        await buildSignedPayload(wallet, NetworkOperationType.BROADCAST_TRANSACTION_REQUEST),
        await buildSignedPayload(wallet, NetworkOperationType.BROADCAST_TRANSACTION_RESPONSE),
    ];

    for (const payload of payloads) {
        operation.isPayloadSchemaValid(payload);
    }

    t.pass();
});

test('V1BaseOperation.validateSignature verifies valid signatures for all supported message types', async t => {
    const operation = new V1BaseOperation(config);
    const wallet = await createWallet();

    const payloads = [
        await buildSignedPayload(wallet, NetworkOperationType.LIVENESS_REQUEST),
        await buildSignedPayload(wallet, NetworkOperationType.LIVENESS_RESPONSE),
        await buildSignedPayload(wallet, NetworkOperationType.BROADCAST_TRANSACTION_REQUEST),
        await buildSignedPayload(wallet, NetworkOperationType.BROADCAST_TRANSACTION_RESPONSE),
    ];

    for (const payload of payloads) {
        await operation.validateSignature(payload, wallet.publicKey);
    }

    t.pass();
});

test('V1BaseOperation.validateSignature throws V1ProtocolError on wrong public key', async t => {
    const operation = new V1BaseOperation(config);
    const wallet = await createWallet(testKeyPair1);
    const otherWallet = await createWallet(testKeyPair2);

    const payload = await buildSignedPayload(wallet, NetworkOperationType.LIVENESS_REQUEST);

    await t.exception(
        async () => operation.validateSignature(payload, otherWallet.publicKey),
        errorMessageIncludes('signature verification failed')
    );
});

test('V1BaseOperation.validateSignature rethrows protocol-shaped build errors', async t => {
    const operation = new V1BaseOperation(config);

    const payload = {
        type: 0,
        id: 'id',
        timestamp: Date.now(),
        capabilities: [],
    };

    try {
        await operation.validateSignature(payload, b4a.alloc(32, 1));
        t.fail('expected validateSignature to throw');
    } catch (error) {
        t.ok(error instanceof V1ProtocolError);
        t.is(error.resultCode, ResultCode.INVALID_PAYLOAD);
        t.ok(error.message.includes('Operation type is unspecified'));
    }
});

test('V1BaseOperation.validateSignature wraps non-protocol build errors as V1ProtocolError', async t => {
    const operation = new V1BaseOperation(config);

    const payload = {
        type: NetworkOperationType.LIVENESS_REQUEST,
        id: 'id',
        timestamp: Date.now(),
        capabilities: [],
    };

    try {
        await operation.validateSignature(payload, b4a.alloc(32, 1));
        t.fail('expected validateSignature to throw');
    } catch (error) {
        t.ok(error instanceof V1ProtocolError);
        t.is(error.resultCode, ResultCode.INVALID_PAYLOAD);
        t.ok(error.message.includes('Failed to build signature message'));
    }
});

test('V1BaseOperation.validateSignature throws V1ProtocolError when hashing fails', async t => {
    const operation = new V1BaseOperation(config);
    const wallet = await createWallet();
    const payload = await buildSignedPayload(wallet, NetworkOperationType.LIVENESS_REQUEST);

    const originalBlake3 = tracCryptoApi.hash.blake3;
    tracCryptoApi.hash.blake3 = async () => {
        throw new Error('hash fail');
    };

    t.teardown(() => {
        tracCryptoApi.hash.blake3 = originalBlake3;
    });

    try {
        await operation.validateSignature(payload, wallet.publicKey);
        t.fail('expected validateSignature to throw');
    } catch (error) {
        t.ok(error instanceof V1ProtocolError);
        t.ok(error.message.includes('Failed to hash signature message'));
    }
});

test('V1BaseOperation.validateSignature handles verify() throw as invalid signature', async t => {
    const operation = new V1BaseOperation(config);
    const wallet = await createWallet();
    const payload = await buildSignedPayload(wallet, NetworkOperationType.LIVENESS_REQUEST);

    const originalVerify = tracCryptoApi.signature.verify;
    tracCryptoApi.signature.verify = () => {
        throw new Error('verify fail');
    };

    t.teardown(() => {
        tracCryptoApi.signature.verify = originalVerify;
    });

    try {
        await operation.validateSignature(payload, wallet.publicKey);
        t.fail('expected validateSignature to throw');
    } catch (error) {
        t.ok(error instanceof V1ProtocolError);
        t.is(error.resultCode, ResultCode.SIGNATURE_INVALID);
    }
});

test('V1BaseOperation.validateSignature enforces BROADCAST_TRANSACTION_RESPONSE proof/timestamp invariants', async t => {
    const operation = new V1BaseOperation(config);
    const wallet = await createWallet();

    const validBase = await buildSignedPayload(wallet, NetworkOperationType.BROADCAST_TRANSACTION_RESPONSE);

    const cases = [
        {
            mutate: payload => {
                payload.broadcast_transaction_response.result = ResultCode.OK;
                payload.broadcast_transaction_response.proof = b4a.alloc(0);
            },
            match: 'Result code OK requires non-empty proof'
        },
        {
            mutate: payload => {
                payload.broadcast_transaction_response.result = ResultCode.TX_ACCEPTED_PROOF_UNAVAILABLE;
                payload.broadcast_transaction_response.proof = b4a.from('aa', 'hex');
            },
            match: 'TX_ACCEPTED_PROOF_UNAVAILABLE requires empty proof'
        },
        {
            mutate: payload => {
                payload.broadcast_transaction_response.result = ResultCode.TX_ACCEPTED_PROOF_UNAVAILABLE;
                payload.broadcast_transaction_response.proof = b4a.alloc(0);
                payload.broadcast_transaction_response.timestamp = 0;
            },
            match: 'TX_ACCEPTED_PROOF_UNAVAILABLE requires timestamp > 0'
        },
        {
            mutate: payload => {
                payload.broadcast_transaction_response.result = ResultCode.TIMEOUT;
                payload.broadcast_transaction_response.proof = b4a.from('aa', 'hex');
                payload.broadcast_transaction_response.timestamp = 0;
            },
            match: 'Non-OK result code requires empty proof'
        },
        {
            mutate: payload => {
                payload.broadcast_transaction_response.result = ResultCode.TIMEOUT;
                payload.broadcast_transaction_response.proof = b4a.alloc(0);
                payload.broadcast_transaction_response.timestamp = Date.now();
            },
            match: 'Non-OK result code requires timestamp to be 0'
        },
    ];

    for (const scenario of cases) {
        const payload = structuredClone(validBase);
        scenario.mutate(payload);

        await t.exception(
            async () => operation.validateSignature(payload, wallet.publicKey),
            errorMessageIncludes(scenario.match)
        );
    }
});

test('V1BaseOperation.validateSignature throws V1ProtocolError for unknown operation type', async t => {
    const operation = new V1BaseOperation(config);

    const payload = {
        type: 9999,
        id: 'id',
        timestamp: Date.now(),
        capabilities: [],
    };

    await t.exception(
        async () => operation.validateSignature(payload, b4a.alloc(32, 1)),
        errorMessageIncludes('Unknown operation type')
    );
});

test('V1BaseOperation.validatePeerCorrectness validates response sender identity', t => {
    const operation = new V1BaseOperation(config);
    const remotePublicKey = b4a.alloc(32, 9);

    operation.validatePeerCorrectness(remotePublicKey, {
        requestedTo: b4a.toString(remotePublicKey, 'hex')
    });

    t.exception(
        () => operation.validatePeerCorrectness(remotePublicKey, { requestedTo: 'ff' }),
        errorMessageIncludes('Response sender mismatch')
    );
});

test('V1BaseOperation.validateResponseType supports expected mappings and rejects mismatches', t => {
    const operation = new V1BaseOperation(config);

    operation.validateResponseType(
        { type: NetworkOperationType.LIVENESS_RESPONSE },
        { id: '1', requestType: NetworkOperationType.LIVENESS_REQUEST }
    );

    operation.validateResponseType(
        { type: NetworkOperationType.BROADCAST_TRANSACTION_RESPONSE },
        { id: '2', requestType: NetworkOperationType.BROADCAST_TRANSACTION_REQUEST }
    );

    t.exception(
        () => operation.validateResponseType(
            { type: NetworkOperationType.BROADCAST_TRANSACTION_RESPONSE },
            { id: '3', requestType: NetworkOperationType.LIVENESS_REQUEST }
        ),
        errorMessageIncludes('Response type mismatch')
    );

    try {
        operation.validateResponseType(
            { type: NetworkOperationType.LIVENESS_RESPONSE },
            { id: '4', requestType: 9999 }
        );
        t.fail('expected validateResponseType to throw');
    } catch (error) {
        t.ok(error instanceof V1ProtocolError);
        t.ok(error.message.includes('Unsupported pending request type'));
    }
});

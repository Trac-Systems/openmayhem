import {test} from 'brittle';
import b4a from 'b4a';
import Autobase from 'autobase';
import Hypercore from 'hypercore';

import V1BroadcastTransactionResponse, {extractRequiredVaFromDecodedTx} from '../../../../src/core/network/protocols/v1/validators/V1BroadcastTransactionResponse.js';
import {V1ProtocolError} from '../../../../src/core/network/protocols/v1/V1ProtocolError.js';
import Check from '../../../../src/utils/check.js';
import {
    unsafeEncodeApplyOperation,
    unsafeDecodeApplyOperation,
} from '../../../../src/utils/protobuf/operationHelpers.js';
import {addressToBuffer} from '../../../../src/core/state/utils/address.js';
import {publicKeyToAddress} from '../../../../src/utils/helpers.js';
import {OperationType, ResultCode} from '../../../../src/utils/constants.js';
import { config } from '../../../helpers/config.js';
import { createState } from '../utils/createState.js';
import protobufFixtures from '../../../fixtures/protobuf.fixtures.js';
import { testKeyPair1 } from '../../../fixtures/apply.fixtures.js';

const remotePublicKey = b4a.from(testKeyPair1.publicKey, 'hex');
const remoteAddressBuffer = addressToBuffer(publicKeyToAddress(remotePublicKey, config), config.addressPrefix);
const writerKey = b4a.alloc(32, 2);

const defaultStateOverrides = {
    registeredWriterKeys: new Map([[b4a.toString(writerKey, 'hex'), remoteAddressBuffer]]),
    signedEntries: new Map([[
        b4a.toString(remoteAddressBuffer),
        {
            isWriter: true,
            wk: writerKey,
        }
    ]]),
};

const createValidator = (stateOverrides = {}) =>
    new V1BroadcastTransactionResponse(createState({
        ...defaultStateOverrides,
        ...stateOverrides
    }), config);

const overrideCheckMethods = (t, overrides) => {
    const originals = {};
    for (const [name, impl] of Object.entries(overrides)) {
        originals[name] = Check.prototype[name];
        Check.prototype[name] = impl;
    }

    t.teardown(() => {
        for (const [name, original] of Object.entries(originals)) {
            Check.prototype[name] = original;
        }
    });
};

const overrideFunction = (t, target, key, replacement) => {
    const original = target[key];
    target[key] = replacement;
    t.teardown(() => {
        target[key] = original;
    });
};

test('extractRequiredVaFromDecodedTx throws VALIDATOR_TX_OBJECT_INVALID for non-object', t => {
    try {
        extractRequiredVaFromDecodedTx(null);
        t.fail('expected throw');
    } catch (err) {
        t.is(err.resultCode, ResultCode.VALIDATOR_TX_OBJECT_INVALID);
    }
});

test('extractRequiredVaFromDecodedTx throws VALIDATOR_VA_MISSING when va is missing', t => {
    try {
        extractRequiredVaFromDecodedTx({type: 1, txo: {tx: b4a.alloc(32)}});
        t.fail('expected throw');
    } catch (err) {
        t.is(err.resultCode, ResultCode.VALIDATOR_VA_MISSING);
    }
});

test('extractRequiredVaFromDecodedTx returns va buffer when present', t => {
    const va = b4a.alloc(39, 1);
    const extracted = extractRequiredVaFromDecodedTx({
        type: OperationType.TX,
        txo: {va}
    });

    t.ok(b4a.equals(extracted, va));
});

test('validate skips proof validation when result code is non-OK', async t => {
    const validator = createValidator();
    let proofCalled = false;

    validator.isPayloadSchemaValid = () => true;
    validator.validateResponseType = () => true;
    validator.validatePeerCorrectness = () => true;
    validator.validateSignature = async () => true;
    validator.verifyProofOfPublication = async () => {
        proofCalled = true;
        return {};
    };

    const payload = {
        broadcast_transaction_response: {
            result: ResultCode.TIMEOUT,
        }
    };

    const result = await validator.validate(
        payload,
        { remotePublicKey: b4a.alloc(32, 1) },
        {},
        null
    );

    t.is(result, true);
    t.absent(proofCalled);
});

test('validate runs proof validation pipeline when result code is OK', async t => {
    const validator = createValidator();
    const calls = [];

    validator.isPayloadSchemaValid = () => calls.push('schema');
    validator.validateResponseType = () => calls.push('type');
    validator.validatePeerCorrectness = () => calls.push('peer');
    validator.validateSignature = async () => calls.push('signature');
    validator.verifyProofOfPublication = async () => {
        calls.push('proof');
        return { proof: {}, manifest: {} };
    };
    validator.assertProofPayloadMatchesRequestPayload = async () => {
        calls.push('assert-proof-payload');
        return {
            validatorDecodedTx: { type: OperationType.TX, txo: { va: remoteAddressBuffer } },
            manifest: {}
        };
    };
    validator.validateDecodedCompletePayloadSchema = () => calls.push('schema-complete');
    validator.validateWritingKey = async () => {
        calls.push('writer-key');
        return {
            writerKeyFromManifest: writerKey,
            validatorAddressCorrelatedWithManifest: remoteAddressBuffer
        };
    };
    validator.validateValidatorCorrectness = async () => calls.push('validator-correctness');

    const result = await validator.validate(
        { broadcast_transaction_response: { result: ResultCode.OK } },
        { remotePublicKey },
        { requestTxData: b4a.alloc(1), requestedTo: b4a.toString(remotePublicKey, 'hex') },
        {}
    );

    t.is(result, true);
    t.alike(calls, [
        'schema',
        'type',
        'peer',
        'signature',
        'proof',
        'assert-proof-payload',
        'schema-complete',
        'writer-key',
        'validator-correctness'
    ]);
});

test('validate rejects TX_COMMITTED_RECEIPT_MISSING as validator internal error', async t => {
    const validator = createValidator();

    validator.isPayloadSchemaValid = () => true;
    validator.validateResponseType = () => true;
    validator.validatePeerCorrectness = () => true;
    validator.validateSignature = async () => true;

    const payload = {
        broadcast_transaction_response: {
            result: ResultCode.TX_COMMITTED_RECEIPT_MISSING,
        }
    };

    try {
        await validator.validate(
            payload,
            { remotePublicKey: b4a.alloc(32, 1) },
            {},
            null
        );
        t.fail('expected validate to throw');
    } catch (error) {
        t.ok(error instanceof V1ProtocolError);
        t.is(error.resultCode, ResultCode.TX_COMMITTED_RECEIPT_MISSING);
    }
});

test('validateDecodedCompletePayloadSchema throws for missing, unknown and unsupported types', t => {
    const validator = createValidator();

    try {
        validator.validateDecodedCompletePayloadSchema({});
        t.fail('expected missing type to throw');
    } catch (error) {
        t.is(error.resultCode, ResultCode.VALIDATOR_RESPONSE_TX_TYPE_INVALID);
    }

    try {
        validator.validateDecodedCompletePayloadSchema({ type: 9999 });
        t.fail('expected unknown type to throw');
    } catch (error) {
        t.is(error.resultCode, ResultCode.VALIDATOR_RESPONSE_TX_TYPE_UNKNOWN);
    }

    try {
        validator.validateDecodedCompletePayloadSchema({ type: OperationType.ADD_ADMIN });
        t.fail('expected unsupported type to throw');
    } catch (error) {
        t.is(error.resultCode, ResultCode.VALIDATOR_RESPONSE_TX_TYPE_UNSUPPORTED);
    }
});

test('validateDecodedCompletePayloadSchema selects proper validators for supported operation types', t => {
    const validator = createValidator();
    const called = [];

    overrideCheckMethods(t, {
        validateRoleAccessOperation: () => {
            called.push('role');
            return true;
        },
        validateBootstrapDeploymentOperation: () => {
            called.push('bootstrap');
            return true;
        },
        validateTransactionOperation: () => {
            called.push('tx');
            return true;
        },
        validateTransferOperation: () => {
            called.push('transfer');
            return true;
        },
    });

    validator.validateDecodedCompletePayloadSchema({ type: OperationType.ADD_WRITER });
    validator.validateDecodedCompletePayloadSchema({ type: OperationType.BOOTSTRAP_DEPLOYMENT });
    validator.validateDecodedCompletePayloadSchema({ type: OperationType.TX });
    validator.validateDecodedCompletePayloadSchema({ type: OperationType.TRANSFER });

    t.alike(called, ['role', 'bootstrap', 'tx', 'transfer']);
});

test('validateDecodedCompletePayloadSchema throws VALIDATOR_RESPONSE_SCHEMA_INVALID when selected validator fails', t => {
    const validator = createValidator();

    overrideCheckMethods(t, {
        validateTransactionOperation: () => false,
    });

    try {
        validator.validateDecodedCompletePayloadSchema({ type: OperationType.TX });
        t.fail('expected schema invalid');
    } catch (error) {
        t.is(error.resultCode, ResultCode.VALIDATOR_RESPONSE_SCHEMA_INVALID);
    }
});

test('verifyProofOfPublication delegates verification to state instance', async t => {
    const proof = b4a.from('deadbeef', 'hex');
    const validator = createValidator({
        verifyProofOfPublication: receivedProof => {
            t.ok(b4a.equals(receivedProof, proof));
            return { ok: true };
        }
    });

    const result = await validator.verifyProofOfPublication({
        broadcast_transaction_response: {
            proof,
        }
    });

    t.alike(result, { ok: true });
});

test('assertProofPayloadMatchesRequestPayload throws when pending request tx data is missing', async t => {
    const validator = createValidator();

    try {
        await validator.assertProofPayloadMatchesRequestPayload(
            { proof: { block: { value: b4a.alloc(1) }, manifest: {} } },
            { requestTxData: null }
        );
        t.fail('expected missing tx data to throw');
    } catch (error) {
        t.is(error.resultCode, ResultCode.PENDING_REQUEST_MISSING_TX_DATA);
    }
});

test('assertProofPayloadMatchesRequestPayload throws PROOF_PAYLOAD_MISMATCH when decoded payloads differ', async t => {
    const validator = createValidator();

    const requestTxData = unsafeEncodeApplyOperation(protobufFixtures.validPartialTransactionOperation);
    const responseTxData = unsafeEncodeApplyOperation(protobufFixtures.validPartialTransferOperation);

    overrideFunction(t, Autobase, 'decodeValue', async () => responseTxData);

    try {
        await validator.assertProofPayloadMatchesRequestPayload(
            {
                proof: {
                    block: { value: b4a.alloc(1) },
                    manifest: {}
                }
            },
            { requestTxData }
        );
        t.fail('expected payload mismatch');
    } catch (error) {
        t.is(error.resultCode, ResultCode.PROOF_PAYLOAD_MISMATCH);
    }
});

test('assertProofPayloadMatchesRequestPayload strips validator metadata before comparison', async t => {
    const validator = createValidator();

    const requestPayload = structuredClone(protobufFixtures.validTransactionOperation);
    requestPayload.txo.va = null;
    requestPayload.txo.vn = null;
    requestPayload.txo.vs = null;
    const requestTxData = unsafeEncodeApplyOperation(requestPayload);
    const responseTxData = unsafeEncodeApplyOperation(protobufFixtures.validTransactionOperation);
    const manifest = { signers: [] };

    overrideFunction(t, Autobase, 'decodeValue', async () => responseTxData);

    const result = await validator.assertProofPayloadMatchesRequestPayload(
        {
            proof: {
                block: { value: b4a.alloc(1) },
                manifest,
            }
        },
        { requestTxData }
    );

    t.is(result.validatorDecodedTx.type, OperationType.TX);
    t.alike(result.manifest, manifest);

    const decodedResponse = unsafeDecodeApplyOperation(responseTxData);
    t.alike(result.validatorDecodedTx, decodedResponse);
});

test('validateWritingKey throws when writer key is not registered', async t => {
    const validator = createValidator({
        getRegisteredWriterKey: async () => null
    });

    overrideFunction(t, Hypercore, 'key', () => writerKey);

    try {
        await validator.validateWritingKey({}, {});
        t.fail('expected validateWritingKey to throw');
    } catch (error) {
        t.is(error.resultCode, ResultCode.VALIDATOR_WRITER_KEY_NOT_REGISTERED);
    }
});

test('validateWritingKey returns writer key and correlated validator address', async t => {
    const registeredAddress = b4a.alloc(39, 7);
    const validator = createValidator({
        getRegisteredWriterKey: async writerKeyHex => {
            t.is(writerKeyHex, b4a.toString(writerKey, 'hex'));
            return registeredAddress;
        }
    });

    overrideFunction(t, Hypercore, 'key', () => writerKey);

    const result = await validator.validateWritingKey({}, {});

    t.ok(b4a.equals(result.writerKeyFromManifest, writerKey));
    t.ok(b4a.equals(result.validatorAddressCorrelatedWithManifest, registeredAddress));
});

test('validateValidatorCorrectness throws VALIDATOR_ADDRESS_MISMATCH when tx va differs from connection-derived address', async t => {
    const validator = createValidator({
        getNodeEntry: async () => null
    });

    try {
        await validator.validateValidatorCorrectness(
            { txo: { va: b4a.alloc(remoteAddressBuffer.length, 9) } },
            remotePublicKey,
            writerKey,
            remoteAddressBuffer
        );
        t.fail('expected address mismatch');
    } catch (error) {
        t.is(error.resultCode, ResultCode.VALIDATOR_ADDRESS_MISMATCH);
    }
});

test('validateValidatorCorrectness throws VALIDATOR_ADDRESS_MISMATCH when tx va differs from manifest-correlated address', async t => {
    const validator = createValidator({
        getNodeEntry: async () => null
    });

    try {
        await validator.validateValidatorCorrectness(
            { txo: { va: remoteAddressBuffer } },
            remotePublicKey,
            writerKey,
            b4a.alloc(remoteAddressBuffer.length, 8)
        );
        t.fail('expected address mismatch');
    } catch (error) {
        t.is(error.resultCode, ResultCode.VALIDATOR_ADDRESS_MISMATCH);
    }
});

test('validateValidatorCorrectness throws VALIDATOR_NODE_ENTRY_NOT_FOUND when state has no node entry', async t => {
    const validator = createValidator({
        getNodeEntry: async () => null
    });

    try {
        await validator.validateValidatorCorrectness(
            { txo: { va: remoteAddressBuffer } },
            remotePublicKey,
            writerKey,
            remoteAddressBuffer
        );
        t.fail('expected missing node entry');
    } catch (error) {
        t.is(error.resultCode, ResultCode.VALIDATOR_NODE_ENTRY_NOT_FOUND);
    }
});

test('validateValidatorCorrectness throws VALIDATOR_NODE_NOT_WRITER when node is not a writer', async t => {
    const validator = createValidator({
        getNodeEntry: async () => ({
            isWriter: false,
            wk: writerKey,
        })
    });

    try {
        await validator.validateValidatorCorrectness(
            { txo: { va: remoteAddressBuffer } },
            remotePublicKey,
            writerKey,
            remoteAddressBuffer
        );
        t.fail('expected node-not-writer');
    } catch (error) {
        t.is(error.resultCode, ResultCode.VALIDATOR_NODE_NOT_WRITER);
    }
});

test('validateValidatorCorrectness throws VALIDATOR_WRITER_KEY_MISMATCH when state writer key differs', async t => {
    const validator = createValidator({
        getNodeEntry: async () => ({
            isWriter: true,
            wk: b4a.alloc(32, 99),
        })
    });

    try {
        await validator.validateValidatorCorrectness(
            { txo: { va: remoteAddressBuffer } },
            remotePublicKey,
            writerKey,
            remoteAddressBuffer
        );
        t.fail('expected writer-key mismatch');
    } catch (error) {
        t.is(error.resultCode, ResultCode.VALIDATOR_WRITER_KEY_MISMATCH);
    }
});

test('validateValidatorCorrectness passes when validator address and writer key are consistent', async t => {
    const validator = createValidator({
        getNodeEntry: async () => ({
            isWriter: true,
            wk: writerKey,
        })
    });

    await validator.validateValidatorCorrectness(
        { txo: { va: remoteAddressBuffer } },
        remotePublicKey,
        writerKey,
        remoteAddressBuffer
    );

    t.pass();
});

test('validateIfResultCodeIsValidatorInternalError throws only for TX_COMMITTED_RECEIPT_MISSING', t => {
    const validator = createValidator();

    try {
        validator.validateIfResultCodeIsValidatorInternalError(ResultCode.TX_COMMITTED_RECEIPT_MISSING);
        t.fail('expected internal error result code to throw');
    } catch (error) {
        t.is(error.resultCode, ResultCode.TX_COMMITTED_RECEIPT_MISSING);
    }

    validator.validateIfResultCodeIsValidatorInternalError(ResultCode.OK);
    t.pass();
});

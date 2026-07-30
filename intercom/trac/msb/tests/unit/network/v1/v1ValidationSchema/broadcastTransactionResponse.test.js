import test from 'brittle';
import b4a from 'b4a';

import V1ValidationSchema from '../../../../../src/core/network/protocols/v1/validators/V1ValidationSchema.js';
import {
    NetworkOperationType,
    NONCE_BYTE_LENGTH,
    ResultCode,
    SIGNATURE_BYTE_LENGTH,
} from '../../../../../src/utils/constants.js';
import {not_allowed_data_types} from '../../../../fixtures/check.fixtures.js';

import {
    assertNoThrowAndAbsent,
    fieldsNonZeroBufferTest,
    headerFieldValueValidationTests,
    topLevelValidationTests,
    valueLevelValidationTests,
} from './common.test.js';

const v = new V1ValidationSchema();

const getValidFixture = () => ({
    type: NetworkOperationType.BROADCAST_TRANSACTION_RESPONSE,
    id: 'test-id',
    timestamp: Date.now(),
    broadcast_transaction_response: {
        nonce: b4a.alloc(NONCE_BYTE_LENGTH, 1),
        signature: b4a.alloc(SIGNATURE_BYTE_LENGTH, 2),
        proof: b4a.from('deadbeef', 'hex'),
        timestamp: Date.now(),
        result: ResultCode.OK,
    },
    capabilities: ['cap:a'],
});

const topFields = ['type', 'id', 'timestamp', 'broadcast_transaction_response', 'capabilities'];
const valueFields = ['nonce', 'signature', 'proof', 'result'];

test('V1ValidationSchema.validateV1BroadcastTransactionResponse - happy path', t => {
    t.ok(v.validateV1BroadcastTransactionResponse(getValidFixture()), 'valid BROADCAST_TRANSACTION_RESPONSE should pass');
});

test('V1ValidationSchema.validateV1BroadcastTransactionResponse - payload/type mismatch (request payload + response type)', t => {
    const op = getValidFixture();
    delete op.broadcast_transaction_response;
    op.broadcast_transaction_request = {
        data: b4a.alloc(16, 1),
        nonce: b4a.alloc(NONCE_BYTE_LENGTH, 2),
        signature: b4a.alloc(SIGNATURE_BYTE_LENGTH, 3),
    };

    assertNoThrowAndAbsent(
        t,
        v.validateV1BroadcastTransactionResponse.bind(v),
        op,
        'broadcast_transaction_request payload with type=BROADCAST_TRANSACTION_RESPONSE'
    );
});

test('V1ValidationSchema.validateV1BroadcastTransactionResponse - top level validation', t => {
    topLevelValidationTests(
        t,
        v.validateV1BroadcastTransactionResponse.bind(v),
        getValidFixture(),
        'broadcast_transaction_response',
        not_allowed_data_types,
        topFields,
        NetworkOperationType.BROADCAST_TRANSACTION_RESPONSE
    );
});

test('V1ValidationSchema.validateV1BroadcastTransactionResponse - header field values', t => {
    headerFieldValueValidationTests(t, v.validateV1BroadcastTransactionResponse.bind(v), getValidFixture());
});

test('V1ValidationSchema.validateV1BroadcastTransactionResponse - payload validation', t => {
    valueLevelValidationTests(
        t,
        v.validateV1BroadcastTransactionResponse.bind(v),
        getValidFixture(),
        'broadcast_transaction_response',
        valueFields,
        not_allowed_data_types,
        {
            skipInvalidType: (field, invalidType) => {
                if (field === 'result' && typeof invalidType === 'number') return true;
                if (field === 'proof' && b4a.isBuffer(invalidType)) return true;
                return false;
            }
        }
    );
});

test('V1ValidationSchema.validateV1BroadcastTransactionResponse - result value validation', t => {
    const buildWithResult = (res) => {
        const fix = getValidFixture();
        fix.broadcast_transaction_response.result = res;
        return fix;
    };

    t.absent(v.validateV1BroadcastTransactionResponse(buildWithResult(-1)), 'negative result should fail');
    t.absent(v.validateV1BroadcastTransactionResponse(buildWithResult(1.1)), 'non-integer result should fail');
    t.absent(v.validateV1BroadcastTransactionResponse(buildWithResult(NaN)), 'result NaN should fail');
    t.absent(v.validateV1BroadcastTransactionResponse(buildWithResult(Infinity)), 'result Infinity should fail');

    const unknownCode = Math.max(...Object.values(ResultCode)) + 1;
    t.absent(v.validateV1BroadcastTransactionResponse(buildWithResult(unknownCode)), 'unknown result code should fail');
});

test('V1ValidationSchema.validateV1BroadcastTransactionResponse - buffer lengths', t => {
    const buildWithNonce = (nonceBuf) => {
        const fix = getValidFixture();
        fix.broadcast_transaction_response.nonce = nonceBuf;
        return fix;
    };
    const buildWithSig = (sigBuf) => {
        const fix = getValidFixture();
        fix.broadcast_transaction_response.signature = sigBuf;
        return fix;
    };

    t.ok(v.validateV1BroadcastTransactionResponse(getValidFixture()), 'nonce exact length (length 32) should pass');

    t.absent(v.validateV1BroadcastTransactionResponse(buildWithNonce(b4a.alloc(31, 0x01))), 'nonce too short should fail');
    t.absent(v.validateV1BroadcastTransactionResponse(buildWithNonce(b4a.alloc(33, 0x01))), 'nonce too long should fail');

    t.ok(v.validateV1BroadcastTransactionResponse(getValidFixture()), 'signature exact length (length 64) should pass');

    t.absent(v.validateV1BroadcastTransactionResponse(buildWithSig(b4a.alloc(63, 0x01))), 'signature too short should fail');
    t.absent(v.validateV1BroadcastTransactionResponse(buildWithSig(b4a.alloc(65, 0x01))), 'signature too long should fail');
});

test('V1ValidationSchema.validateV1BroadcastTransactionResponse - non-zero buffers', t => {
    fieldsNonZeroBufferTest(
        t,
        v.validateV1BroadcastTransactionResponse.bind(v),
        getValidFixture(),
        'broadcast_transaction_response',
        ['nonce', 'signature']
    );
});
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
    fieldsBufferLengthTest,
    fieldsNonZeroBufferTest,
    headerFieldValueValidationTests,
    topLevelValidationTests,
    valueLevelValidationTests,
} from './common.test.js';

const v = new V1ValidationSchema();

const validFixture = {
    type: NetworkOperationType.LIVENESS_RESPONSE,
    id: 'test-id',
    timestamp: Date.now(),
    liveness_response: {
        nonce: b4a.alloc(NONCE_BYTE_LENGTH, 1),
        signature: b4a.alloc(SIGNATURE_BYTE_LENGTH, 2),
        result: ResultCode.OK,
    },
    capabilities: ['cap:a'],
};

const topFields = ['type', 'id', 'timestamp', 'liveness_response', 'capabilities'];
const valueFields = ['nonce', 'signature', 'result'];
const requiredLengths = {
    nonce: NONCE_BYTE_LENGTH,
    signature: SIGNATURE_BYTE_LENGTH,
};

test('V1ValidationSchema.validateV1LivenessResponse - happy path', t => {
    t.ok(v.validateV1LivenessResponse(validFixture), 'valid LIVENESS_RESPONSE should pass');
});

test('V1ValidationSchema.validateV1LivenessResponse - payload/type mismatch (request payload + response type)', t => {
    const op = structuredClone(validFixture);
    delete op.liveness_response;
    op.liveness_request = {
        nonce: b4a.alloc(NONCE_BYTE_LENGTH, 1),
        signature: b4a.alloc(SIGNATURE_BYTE_LENGTH, 2),
    };

    assertNoThrowAndAbsent(
        t,
        v.validateV1LivenessResponse.bind(v),
        op,
        'liveness_request payload with type=LIVENESS_RESPONSE'
    );
});

test('V1ValidationSchema.validateV1LivenessResponse - top level validation', t => {
    topLevelValidationTests(
        t,
        v.validateV1LivenessResponse.bind(v),
        validFixture,
        'liveness_response',
        not_allowed_data_types,
        topFields,
        NetworkOperationType.LIVENESS_RESPONSE
    );
});

test('V1ValidationSchema.validateV1LivenessResponse - header field values', t => {
    headerFieldValueValidationTests(t, v.validateV1LivenessResponse.bind(v), validFixture);
});

test('V1ValidationSchema.validateV1LivenessResponse - payload validation', t => {
    valueLevelValidationTests(
        t,
        v.validateV1LivenessResponse.bind(v),
        validFixture,
        'liveness_response',
        valueFields,
        not_allowed_data_types,
        {
            skipInvalidType: (field, invalidType) => field === 'result' && typeof invalidType === 'number',
        }
    );
});

test('V1ValidationSchema.validateV1LivenessResponse - result value validation', t => {
    const negative = structuredClone(validFixture);
    negative.liveness_response.result = -1;
    t.absent(v.validateV1LivenessResponse(negative), 'negative result should fail');

    const nonInteger = structuredClone(validFixture);
    nonInteger.liveness_response.result = 1.1;
    t.absent(v.validateV1LivenessResponse(nonInteger), 'non-integer result should fail');

    const nan = structuredClone(validFixture);
    nan.liveness_response.result = NaN;
    t.absent(v.validateV1LivenessResponse(nan), 'result NaN should fail');

    const infinity = structuredClone(validFixture);
    infinity.liveness_response.result = Infinity;
    t.absent(v.validateV1LivenessResponse(infinity), 'result Infinity should fail');

    const unknownCode = structuredClone(validFixture);
    unknownCode.liveness_response.result = Math.max(...Object.values(ResultCode)) + 1;
    t.absent(v.validateV1LivenessResponse(unknownCode), 'unknown result code should fail');
});

test('V1ValidationSchema.validateV1LivenessResponse - buffer lengths', t => {
    fieldsBufferLengthTest(
        t,
        v.validateV1LivenessResponse.bind(v),
        validFixture,
        'liveness_response',
        requiredLengths
    );
});

test('V1ValidationSchema.validateV1LivenessResponse - non-zero buffers', t => {
    fieldsNonZeroBufferTest(
        t,
        v.validateV1LivenessResponse.bind(v),
        validFixture,
        'liveness_response',
        ['nonce', 'signature']
    );
});

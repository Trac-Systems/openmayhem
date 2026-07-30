import test from 'brittle';
import b4a from 'b4a';

import V1ValidationSchema from '../../../../../src/core/network/protocols/v1/validators/V1ValidationSchema.js';
import {
    NetworkOperationType,
    NONCE_BYTE_LENGTH,
    SIGNATURE_BYTE_LENGTH,
} from '../../../../../src/utils/constants.js';
import {not_allowed_data_types} from '../../../../fixtures/check.fixtures.js';

import {
    fieldsBufferLengthTest,
    fieldsNonZeroBufferTest,
    headerFieldValueValidationTests,
    topLevelValidationTests,
    valueLevelValidationTests,
} from './common.test.js';

const v = new V1ValidationSchema();

const validFixture = {
    type: NetworkOperationType.LIVENESS_REQUEST,
    id: 'test-id',
    timestamp: Date.now(),
    liveness_request: {
        nonce: b4a.alloc(NONCE_BYTE_LENGTH, 1),
        signature: b4a.alloc(SIGNATURE_BYTE_LENGTH, 2),
    },
    capabilities: ['cap:a', 'cap:b'],
};

const topFields = ['type', 'id', 'timestamp', 'liveness_request', 'capabilities'];
const valueFields = ['nonce', 'signature'];
const requiredLengths = {
    nonce: NONCE_BYTE_LENGTH,
    signature: SIGNATURE_BYTE_LENGTH,
};

test('V1ValidationSchema.validateV1LivenessRequest - happy path', t => {
    t.ok(v.validateV1LivenessRequest(validFixture), 'valid LIVENESS_REQUEST should pass');
});

test('V1ValidationSchema.validateV1LivenessRequest - top level validation', t => {
    topLevelValidationTests(
        t,
        v.validateV1LivenessRequest.bind(v),
        validFixture,
        'liveness_request',
        not_allowed_data_types,
        topFields,
        NetworkOperationType.LIVENESS_REQUEST
    );
});

test('V1ValidationSchema.validateV1LivenessRequest - header field values', t => {
    headerFieldValueValidationTests(t, v.validateV1LivenessRequest.bind(v), validFixture);
});

test('V1ValidationSchema.validateV1LivenessRequest - payload validation', t => {
    valueLevelValidationTests(
        t,
        v.validateV1LivenessRequest.bind(v),
        validFixture,
        'liveness_request',
        valueFields,
        not_allowed_data_types,
    );
});

test('V1ValidationSchema.validateV1LivenessRequest - buffer lengths', t => {
    fieldsBufferLengthTest(
        t,
        v.validateV1LivenessRequest.bind(v),
        validFixture,
        'liveness_request',
        requiredLengths
    );
});

test('V1ValidationSchema.validateV1LivenessRequest - non-zero buffers', t => {
    fieldsNonZeroBufferTest(
        t,
        v.validateV1LivenessRequest.bind(v),
        validFixture,
        'liveness_request',
        ['nonce', 'signature']
    );
});


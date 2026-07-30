import test from 'brittle';
import b4a from 'b4a';

import V1ValidationSchema from '../../../../../src/core/network/protocols/v1/validators/V1ValidationSchema.js';
import {
    NetworkOperationType,
    NONCE_BYTE_LENGTH,
    SIGNATURE_BYTE_LENGTH,
    MAX_PARTIAL_TX_PAYLOAD_BYTE_SIZE
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

const bytes = (len, fill = 1) => b4a.alloc(len, fill);

const validFixture = {
    type: NetworkOperationType.BROADCAST_TRANSACTION_REQUEST,
    id: 'test-id',
    timestamp: Date.now(),
    broadcast_transaction_request: {
        data: bytes(16, 1),
        nonce: bytes(NONCE_BYTE_LENGTH, 2),
        signature: bytes(SIGNATURE_BYTE_LENGTH, 3),
    },
    capabilities: ['cap:a'],
};

const topFields = ['type', 'id', 'timestamp', 'broadcast_transaction_request', 'capabilities'];
const valueFields = ['data', 'nonce', 'signature'];
const requiredLengths = {
    nonce: NONCE_BYTE_LENGTH,
    signature: SIGNATURE_BYTE_LENGTH,
};

test('V1ValidationSchema.validateV1BroadcastTransactionRequest - happy path', t => {
    t.ok(v.validateV1BroadcastTransactionRequest(validFixture), 'valid BROADCAST_TRANSACTION_REQUEST should pass');
});

test('V1ValidationSchema.validateV1BroadcastTransactionRequest - top level validation', t => {
    topLevelValidationTests(
        t,
        v.validateV1BroadcastTransactionRequest.bind(v),
        validFixture,
        'broadcast_transaction_request',
        not_allowed_data_types,
        topFields,
        NetworkOperationType.BROADCAST_TRANSACTION_REQUEST
    );
});

test('V1ValidationSchema.validateV1BroadcastTransactionRequest - header field values', t => {
    headerFieldValueValidationTests(t, v.validateV1BroadcastTransactionRequest.bind(v), validFixture);
});

test('V1ValidationSchema.validateV1BroadcastTransactionRequest - payload validation', t => {
    valueLevelValidationTests(
        t,
        v.validateV1BroadcastTransactionRequest.bind(v),
        validFixture,
        'broadcast_transaction_request',
        valueFields,
        not_allowed_data_types,
        {
            skipInvalidType: (field, invalidType) => field === 'data' && b4a.isBuffer(invalidType),
        }
    );
});

test('V1ValidationSchema.validateV1BroadcastTransactionRequest - data size limits', t => {
    const min = structuredClone(validFixture);
    min.broadcast_transaction_request.data = bytes(1, 1);
    t.ok(v.validateV1BroadcastTransactionRequest(min), 'data length 1 should pass');

    const max = structuredClone(validFixture);
    max.broadcast_transaction_request.data = bytes(MAX_PARTIAL_TX_PAYLOAD_BYTE_SIZE, 1);
    t.ok(v.validateV1BroadcastTransactionRequest(max), `data length ${MAX_PARTIAL_TX_PAYLOAD_BYTE_SIZE} should pass`);

    const tooLarge = structuredClone(validFixture);
    tooLarge.broadcast_transaction_request.data = bytes(MAX_PARTIAL_TX_PAYLOAD_BYTE_SIZE + 1, 1);
    t.absent(v.validateV1BroadcastTransactionRequest(tooLarge), `data length ${MAX_PARTIAL_TX_PAYLOAD_BYTE_SIZE + 1} should fail`);

    const empty = structuredClone(validFixture);
    empty.broadcast_transaction_request.data = bytes(0, 0);
    t.absent(v.validateV1BroadcastTransactionRequest(empty), 'data length 0 should fail');

    const zeroFilled = structuredClone(validFixture);
    zeroFilled.broadcast_transaction_request.data = bytes(16, 0);
    t.ok(v.validateV1BroadcastTransactionRequest(zeroFilled), 'zero-filled data should pass (allowZero)');
});

test('V1ValidationSchema.validateV1BroadcastTransactionRequest - buffer lengths', t => {
    fieldsBufferLengthTest(
        t,
        v.validateV1BroadcastTransactionRequest.bind(v),
        validFixture,
        'broadcast_transaction_request',
        requiredLengths
    );
});

test('V1ValidationSchema.validateV1BroadcastTransactionRequest - non-zero buffers', t => {
    fieldsNonZeroBufferTest(
        t,
        v.validateV1BroadcastTransactionRequest.bind(v),
        validFixture,
        'broadcast_transaction_request',
        ['nonce', 'signature']
    );
});


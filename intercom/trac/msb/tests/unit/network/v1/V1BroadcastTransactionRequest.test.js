import test from 'brittle';
import b4a from 'b4a';

import V1BroadcastTransactionRequest from '../../../../src/core/network/protocols/v1/validators/V1BroadcastTransactionRequest.js';
import { V1ProtocolError } from '../../../../src/core/network/protocols/v1/V1ProtocolError.js';
import { MAX_PARTIAL_TX_PAYLOAD_BYTE_SIZE } from '../../../../src/utils/constants.js';
import { config } from '../../../helpers/config.js';

test('V1BroadcastTransactionRequest.validate runs schema, size and signature validation', async t => {
    const validator = new V1BroadcastTransactionRequest(config);
    const calls = [];

    validator.isPayloadSchemaValid = () => calls.push('schema');
    validator.isDataPropertySizeValid = () => calls.push('size');
    validator.validateSignature = async () => calls.push('signature');

    const result = await validator.validate({}, b4a.alloc(32, 1));

    t.is(result, true);
    t.is(calls.length, 3);
    t.is(calls[0], 'schema');
    t.is(calls[1], 'size');
    t.is(calls[2], 'signature');
});

test('V1BroadcastTransactionRequest.isDataPropertySizeValid accepts max allowed payload size', t => {
    const validator = new V1BroadcastTransactionRequest(config);
    const payload = {
        broadcast_transaction_request: {
            data: b4a.alloc(MAX_PARTIAL_TX_PAYLOAD_BYTE_SIZE, 1)
        }
    };

    validator.isDataPropertySizeValid(payload);
    t.pass();
});

test('V1BroadcastTransactionRequest.isDataPropertySizeValid throws for oversized payload', t => {
    const validator = new V1BroadcastTransactionRequest(config);
    const payload = {
        broadcast_transaction_request: {
            data: b4a.alloc(MAX_PARTIAL_TX_PAYLOAD_BYTE_SIZE + 1, 1)
        }
    };

    try {
        validator.isDataPropertySizeValid(payload);
        t.fail('expected size validation to throw');
    } catch (error) {
        t.ok(error instanceof V1ProtocolError);
        t.ok(error.message.includes('exceeds the maximum allowed byte size'));
    }
});

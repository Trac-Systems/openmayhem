import test from 'brittle';
import b4a from 'b4a';

import { bufferToBigInt } from '../../../../src/utils/amountSerialization.js';
import { encodeTransferBatchOutputs, decodeTransferBatchOutputs } from '../../../../src/utils/transferBatch.js';
import { config } from '../../../helpers/config.js';

const RECIPIENT_A = 'trac1mqktwme8fvklrds4hlhfy6lhmsu9qgfn3c3kuhz7c5zwjt8rc3dqj9tx7h';
const RECIPIENT_B = 'trac18qq7h503y3326v6msgvq0jwc0e8jp4t4q53z9p9jvd98arj7mtpqfac04p';

test('transfer batch outputs encode/decode canonical recipients and total', t => {
    const encoded = encodeTransferBatchOutputs([
        { to: RECIPIENT_A, amount: '1.25' },
        { to: RECIPIENT_B, tnk: '0.75' },
    ], config);

    t.ok(b4a.isBuffer(encoded.buffer), 'encoded batch is a buffer');
    t.is(encoded.outputs.length, 2, 'two outputs encoded');
    t.is(bufferToBigInt(encoded.totalAmount), 2000000000000000000n, 'total amount is summed');

    const decoded = decodeTransferBatchOutputs(encoded.buffer, config);
    t.is(decoded.outputs.length, 2, 'two outputs decoded');
    t.is(decoded.outputs[0].to, RECIPIENT_A, 'first recipient preserved');
    t.is(decoded.outputs[1].to, RECIPIENT_B, 'second recipient preserved');
    t.is(decoded.total, 2000000000000000000n, 'decoded total is summed');
    t.ok(b4a.equals(decoded.totalAmount, encoded.totalAmount), 'total buffer roundtrips');
});

test('transfer batch outputs reject duplicates and invalid totals', t => {
    t.exception(
        () => encodeTransferBatchOutputs([
            { to: RECIPIENT_A, amount: '1' },
            { to: RECIPIENT_A, amount: '2' },
        ], config),
        /Duplicate batch transfer recipient/
    );

    t.exception(
        () => encodeTransferBatchOutputs([{ to: RECIPIENT_B, amount: '0' }], config),
        /Invalid batch transfer amount/
    );

    t.exception(
        () => decodeTransferBatchOutputs(b4a.from('00000000', 'hex'), config),
        /Invalid batch transfer output count/
    );
});

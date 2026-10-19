import assert from 'node:assert/strict';
import test from 'node:test';

import { tnkVerificationWindow } from '../scripts/retail-crypto-verification.mjs';

test('TNK verification bounds its scan from the caught-up frontier', () => {
  const staleStartupLength = 338_219;
  const caughtUpLength = 354_500;
  const window = tnkVerificationWindow(caughtUpLength, 5_000);

  assert.deepEqual(window, {
    fromSignedLength: 349_500,
    minimumSignedLength: 354_500,
  });
  assert.notEqual(window.fromSignedLength, staleStartupLength - 5_000);
});

import { createServer } from 'node:http';
import { once } from 'node:events';
import { waitForCoreRecord, assertRetailCreditCompleted } from '../scripts/retail-crypto-payment-worker.mjs';

test('confirmed TAP settlement below the frozen quote completes its wait with the actual value', async () => {
  const value = { rail: 'tap', who: 'buyer', au: '24994404837387782091' };
  let reads = 0;
  const server = createServer((_request, response) => {
    reads++;
    response.setHeader('Content-Type', 'application/json');
    response.end(JSON.stringify({ confirmed: true, signed_length: 400, value }));
  });
  server.listen(0, '127.0.0.1'); await once(server, 'listening');
  try {
    const result = await waitForCoreRecord({ coreRpc: `http://127.0.0.1:${server.address().port}`, api: {
      renew() { throw new Error('Immutable confirmed deposit must not be polled again'); },
    } }, {}, 'dep/tap/existing', 1);
    assert.equal(result.value.au, '24994404837387782091');
    assert.equal(result.signedLength, 400n);
    assert.equal(reads, 1);
  } finally { await new Promise(resolve => server.close(resolve)); }
});

test('TAP and TNK finalization never treats a waiting API result as credited', () => {
  assert.doesNotThrow(() => assertRetailCreditCompleted({ status: 'SUCCEEDED' }));
  for (const result of [null, {}, { status: 'CONFIRMING' }, { status: 'REVIEW' }]) {
    assert.throws(() => assertRetailCreditCompleted(result), error =>
      error.code === 'bridge_funding_shortfall' && error.transferObserved === true);
  }
});

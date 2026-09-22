import assert from 'node:assert/strict';
import test from 'node:test';

import { waitForMinimumSignedLength } from '../scripts/msb-reader-catchup.mjs';

test('waits for a stale nonzero reader to pass the durable cursor', async () => {
  const lengths = [5, 5, 11];
  let reads = 0;
  let sleeps = 0;
  const signedLength = await waitForMinimumSignedLength({
    getSignedLength: () => lengths[Math.min(reads++, lengths.length - 1)],
  }, {
    minimumSignedLength: 11,
    timeoutSec: 3,
    sleepImpl: async () => { sleeps += 1; },
  });

  assert.equal(signedLength, 11);
  assert.equal(sleeps, 2);
});

test('returns the last observed length when catch-up times out', async () => {
  let sleeps = 0;
  const signedLength = await waitForMinimumSignedLength({
    getSignedLength: () => 5,
  }, {
    minimumSignedLength: 11,
    timeoutSec: 2,
    sleepImpl: async () => { sleeps += 1; },
  });

  assert.equal(signedLength, 5);
  assert.equal(sleeps, 2);
});

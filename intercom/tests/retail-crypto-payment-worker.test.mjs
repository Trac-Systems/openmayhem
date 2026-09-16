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

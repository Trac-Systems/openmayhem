/**
 * Wait until a reused read-only MSB store reaches the caller's durable cursor.
 * Opening the store only proves that it is readable; its signed length may
 * still be nonzero and far behind the canonical writer after a long stop.
 */
export async function waitForMinimumSignedLength(state, {
  minimumSignedLength,
  timeoutSec,
  sleepImpl,
}) {
  let signedLength = state.getSignedLength();
  for (let waited = 0; signedLength < minimumSignedLength && waited < timeoutSec; waited += 1) {
    await sleepImpl(1000);
    signedLength = state.getSignedLength();
  }
  return signedLength;
}

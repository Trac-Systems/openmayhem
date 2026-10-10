// Exact, signed TNK admission evidence. The queued transaction hash is durable;
// its age must not determine whether an already-found payment can be verified.
import { unsafeDecodeApplyOperation } from 'trac-msb/src/utils/protobuf/operationHelpers.js';
import { normalizeDecodedPayloadForJson } from 'trac-msb/src/utils/normalizers.js';
import { OperationType } from 'trac-msb/src/utils/constants.js';
import { waitForMinimumSignedLength } from './msb-reader-catchup.mjs';
import { need, hex, uint } from './proxy-admission-wire.mjs';
import { RetryWork } from './retail-crypto-verification.mjs';

export async function verifyTnkObservedTransfer(msb, intent, { frontier, finality, timeoutSeconds, signal, addressPrefix }) {
  need(uint(frontier, 1) && uint(finality, 1) && uint(timeoutSeconds, 1) && timeoutSeconds <= 10
    && typeof addressPrefix === 'string' && /^[a-z0-9]{1,32}$/.test(addressPrefix)
    && hex(intent.transaction_hash) && typeof intent.destination === 'string', 'invalid TNK observation bounds');
  const deadline = AbortSignal.any([signal, AbortSignal.timeout(timeoutSeconds * 1000)]);
  const sleep = ms => new Promise((resolve, reject) => {
    const finish = () => { deadline.removeEventListener('abort', abort); resolve(); };
    const timer = setTimeout(finish, ms);
    const abort = () => { clearTimeout(timer); deadline.removeEventListener('abort', abort); reject(deadline.reason); };
    deadline.addEventListener('abort', abort, { once: true });
    if (deadline.aborted) abort();
  });
  deadline.throwIfAborted();
  const confirmed = await waitForMinimumSignedLength(msb.state, {
    minimumSignedLength: frontier, timeoutSec: timeoutSeconds, sleepImpl: sleep,
  });
  deadline.throwIfAborted();
  if (confirmed < frontier) throw new RetryWork('reader_unavailable', 30);
  need(uint(confirmed, 1) && typeof msb.state.base?.view?.checkout === 'function', 'signed TNK view unavailable');

  // getExtendedTxDetails/getTransactionConfirmedLength scan history. Instead
  // one exact-key lookup returns both sequence and payload from the SAME signed
  // checkout. Never consult the moving unsigned view, a caller-supplied position,
  // or the latest-tip lookback. Existing evidence commitments remain unchanged.
  const snapshot = msb.state.base.view.checkout(confirmed);
  const abort = () => { void snapshot.close().catch(() => {}); };
  deadline.addEventListener('abort', abort, { once: true });
  try {
    deadline.throwIfAborted();
    const entry = await snapshot.get(intent.transaction_hash);
    deadline.throwIfAborted();
    if (!entry) throw new RetryWork('transfer_pending', 20);
    need(entry.key === intent.transaction_hash && uint(entry.seq, 1) && entry.seq < confirmed
      && entry.value instanceof Uint8Array && entry.value.byteLength > 0 && entry.value.byteLength <= 16384,
    'TNK signed transaction differs');
    // Match the existing scanner's exclusive safeEnd: a position at that
    // boundary has not yet reached the configured finality distance.
    if (entry.seq >= Math.max(0, confirmed - finality)) throw new RetryWork('awaiting_finality', 20, true);
    let decoded;
    try { decoded = unsafeDecodeApplyOperation(entry.value); } catch { throw new Error('TNK signed transaction cannot be decoded'); }
    need(decoded?.type === OperationType.TRANSFER && decoded.tro
      && Buffer.from(decoded.tro.tx ?? []).toString('hex') === intent.transaction_hash, 'TNK transfer identity differs');
    const transfer = normalizeDecodedPayloadForJson(decoded, { addressPrefix });
    need(transfer.tro.to === intent.destination, 'TNK destination differs');
    need(typeof transfer.tro.am === 'string' && /^[1-9][0-9]{0,38}$/.test(transfer.tro.am)
      && BigInt(transfer.tro.am) < (1n << 128n), 'TNK amount invalid');
    deadline.throwIfAborted();
    return { finalized: true, transactionHash: intent.transaction_hash, tokenAmountBaseUnits: BigInt(transfer.tro.am),
      fromAddress: transfer.address, toAddress: transfer.tro.to, blockNumber: BigInt(entry.seq) };
  } finally {
    deadline.removeEventListener('abort', abort);
    await snapshot.close();
  }
}

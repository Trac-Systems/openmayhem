// Exact, signed TNK admission evidence. The queued transaction hash is durable;
// its age must not determine whether an already-found payment can be verified.
import { unsafeDecodeApplyOperation } from 'trac-msb/src/utils/protobuf/operationHelpers.js';
import { normalizeDecodedPayloadForJson } from 'trac-msb/src/utils/normalizers.js';
import { OperationType } from 'trac-msb/src/utils/constants.js';
import { isAddressValid } from 'trac-msb/src/core/state/utils/address.js';
import { waitForMinimumSignedLength } from './msb-reader-catchup.mjs';
import { need, hex, uint } from './proxy-admission-wire.mjs';
import { RetryWork } from './retail-crypto-verification.mjs';

/** Discover at most sixteen signed ledger positions, payloads included, from
 * one immutable checkout. The moving getTxHashes/getTxDetails combination cannot
 * establish a coherent scan boundary. This proof describes ledger sequence, not
 * a payment timestamp or proof that the remote network has caught up to now. */
export async function scanTnkSignedPage(msb, { from, frontier, signal, addressPrefix }) {
  need(uint(from) && uint(frontier, 1) && from <= frontier && signal
    && typeof addressPrefix === 'string' && /^[a-z0-9]{1,32}$/.test(addressPrefix), 'invalid TNK page bounds');
  const base = msb.state?.base?.view, core = base?.core;
  need(core && typeof base.checkout === 'function' && typeof core.treeHash === 'function'
    && msb.state.getSignedLength() >= frontier, 'TNK signed reader behind');
  const key = Buffer.from(core.key ?? []).toString('hex'), fork = core.fork;
  need(hex(key) && uint(fork), 'TNK signed view identity unavailable');
  const end = Math.min(frontier, from + 16), snapshot = base.checkout(frontier);
  let stream;
  const abort = () => { stream?.destroy(signal.reason); void snapshot.close().catch(() => {}); };
  signal.addEventListener('abort', abort, { once: true });
  const stable = () => {
    signal.throwIfAborted();
    need(msb.state.base.view === base && base.core === core && core.fork === fork
      && Buffer.from(core.key).toString('hex') === key && msb.state.getSignedLength() >= frontier,
    'TNK signed snapshot changed');
  };
  try {
    stable();
    // Some sparse readers need storage work for the Merkle hash. Bound that
    // await too, before a cancellable history stream even exists.
    let stop;
    const canceled = new Promise((_, reject) => {
      stop = () => reject(signal.reason); signal.addEventListener('abort', stop, { once: true });
      if (signal.aborted) stop();
    });
    let tree;
    try { tree = await Promise.race([core.treeHash(frontier), canceled]); }
    finally { signal.removeEventListener('abort', stop); }
    const treeHash = Buffer.from(tree).toString('hex');
    need(hex(treeHash), 'TNK signed tree hash unavailable'); stable();
    const transfers = []; let previous = from - 1, count = 0;
    if (end > from) {
      stream = snapshot.createHistoryStream({ gte: from, lt: end, limit: 16 });
      for await (const entry of stream) {
        stable();
        need(++count <= 16 && uint(entry.seq) && entry.seq >= from && entry.seq < end && entry.seq > previous,
          'TNK signed history order differs'); previous = entry.seq;
        if (entry.type !== 'put' || !hex(entry.key)) continue;
        need(entry.value instanceof Uint8Array && entry.value.byteLength > 0 && entry.value.byteLength <= 16384,
          'TNK signed payload exceeds bound');
        let decoded;
        try { decoded = unsafeDecodeApplyOperation(entry.value); } catch { throw new Error('TNK signed payload cannot be decoded'); }
        need(decoded && Object.values(OperationType).includes(decoded.type), 'TNK signed operation type invalid');
        if (decoded.type !== OperationType.TRANSFER) continue;
        need(decoded.tro && Buffer.from(decoded.tro.tx ?? []).toString('hex') === entry.key, 'TNK signed transfer identity differs');
        const transfer = normalizeDecodedPayloadForJson(decoded, { addressPrefix });
        need(typeof transfer.tro.to === 'string' && transfer.tro.to.length <= 128 && isAddressValid(transfer.tro.to, addressPrefix)
          && isAddressValid(transfer.address, addressPrefix)
          && typeof transfer.tro.am === 'string' && /^[1-9][0-9]{0,38}$/.test(transfer.tro.am)
          && BigInt(transfer.tro.am) < (1n << 128n), 'TNK signed transfer invalid');
        transfers.push({ destination: transfer.tro.to, position: String(entry.seq), transaction_hash: entry.key });
      }
    }
    stable();
    return { next_cursor: String(end), transfers,
      proof: { view_key: key, fork, signed_length: frontier, tree_hash: treeHash } };
  } finally {
    signal.removeEventListener('abort', abort);
    stream?.destroy();
    await snapshot.close();
  }
}

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

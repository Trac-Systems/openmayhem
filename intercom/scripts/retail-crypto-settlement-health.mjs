import { RetryWork, summarizePayoutLiabilities } from './retail-crypto-verification.mjs';

const HEX64 = /^[0-9a-f]{64}$/;
const UINT = /^(0|[1-9][0-9]*)$/;
const READ_CONCURRENCY = 4;

function invalid() { throw new RetryWork('settlement_state_invalid', 60); }
function integer(value) {
  if (typeof value !== 'string' || !UINT.test(value)) invalid();
  return BigInt(value);
}
function epoch(value) {
  if (!Number.isSafeInteger(value) || value < 0) invalid();
  return value;
}

function payoutMinimum(record, at) {
  // Same genesis default and activation rule as params/payout_min_au.
  if (record === null) return 1_000_000_000_000_000_000n;
  if (record?.key !== 'payout_min_au' || !record.current) invalid();
  integer(record.current.value);
  if (record.pending != null) {
    epoch(record.pending.effective_at);
    integer(record.pending.value);
  }
  return integer(record.pending && record.pending.effective_at <= at
    ? record.pending.value : record.current.value);
}

/** Current indexed liabilities, never transaction or epoch history. */
export async function readSettlementHealth({ read, rail, maxEpochLag, nowUnix = Math.floor(Date.now() / 1_000) }) {
  const name = String(rail).toLowerCase();
  if (!['tnk', 'tap'].includes(name)) invalid();
  epoch(maxEpochLag);
  epoch(nowUnix);
  const apply = await read('epoch/apply/state');
  if (!apply.value) invalid();
  const currentEpoch = epoch(apply.value.updated_epoch);
  const at = epoch(currentEpoch === 0 ? 0 : apply.value.last_settlement_unix);
  if (!Number.isSafeInteger(apply.signed_length) || apply.signed_length < 0) invalid();
  const options = { signedLength: apply.signed_length };
  const point = async (key) => {
    const result = await read(key, options);
    if (result.confirmed !== true || result.signed_length !== apply.signed_length ||
        result.key !== key || !Object.hasOwn(result, 'value')) invalid();
    return result.value;
  };
  if (apply.confirmed !== true || apply.key !== 'epoch/apply/state') invalid();
  const [index, minimumRecord, anchor] = await Promise.all([
    point(`payout/liability-index/${name}`),
    point('params/payout_min_au'),
    currentEpoch > 0 ? point(`epoch/apply-anchor/${currentEpoch}`) : null,
  ]);
  if (currentEpoch > 0 && (anchor?.type !== 'epoch_apply_anchor' ||
      anchor.epoch !== currentEpoch || !HEX64.test(anchor.apply_hash) ||
      anchor.apply_hash !== apply.value.last_apply_hash || anchor.settlement_unix !== at)) invalid();
  const minimumAu = payoutMinimum(minimumRecord, nowUnix);
  if (index !== null && (index?.type !== 'provider_payout_liability_index' ||
      index.rail !== name || !Array.isArray(index.entries) ||
      epoch(index.updated_epoch) > currentEpoch)) invalid();
  const entries = index?.entries ?? [];
  let previous = '';
  for (const entry of entries) {
    if (!HEX64.test(entry?.provider) || !HEX64.test(entry?.payout_revision)) invalid();
    const identity = `${entry.provider}/${entry.payout_revision}`;
    if (identity <= previous) invalid();
    previous = identity;
  }
  const records = new Array(entries.length);
  let next = 0;
  await Promise.all(Array.from({ length: Math.min(READ_CONCURRENCY, entries.length) }, async () => {
    while (next < entries.length) {
      const position = next++;
      const entry = entries[position];
      const key = `payout/liability/${name}/${entry.provider}/${entry.payout_revision}`;
      const value = await point(key);
      if (!value || value.provider !== entry.provider || value.revision !== entry.payout_revision ||
          epoch(value.updated_epoch) > currentEpoch) invalid();
      records[position] = { key, value };
    }
  }));
  const liabilities = summarizePayoutLiabilities(records, name.toUpperCase());
  // The payout minimum applies per provider/revision, not to the rail total.
  // Below-minimum and held amounts still count in unsettledAu/backing checks.
  const dueAu = records.reduce((sum, { value }) => {
    const payable = integer(value.total_au) - integer(value.held_au) - integer(value.paid_cum_au);
    return sum + (payable > 0n && payable >= minimumAu ? payable : 0n);
  }, 0n);

  // Eligibility only depends on whether settlement exists in this finite window.
  // Zero means no close observed in the window, not a scan of all old epochs.
  let lastSettledEpoch = 0;
  for (let candidate = currentEpoch; candidate > 0 && currentEpoch - candidate <= maxEpochLag; candidate--) {
    const close = await point(`settle/targeted/${name}/${candidate}`);
    if (close === null) continue;
    if (close.epoch !== candidate ||
        !['targeted_payout_epoch_close', `targeted_${name}_settlement`].includes(close.type) ||
        (close.type === 'targeted_payout_epoch_close' && close.rail !== name) ||
        (close.rail != null && close.rail !== name)) invalid();
    lastSettledEpoch = candidate;
    break;
  }
  const lag = Math.max(0, currentEpoch - lastSettledEpoch);
  const payoutStatus = dueAu === 0n ? 'current'
    : lastSettledEpoch > 0 ? 'pending' : 'lagging';
  return { ...liabilities, currentEpoch, lastSettledEpoch, payoutStatus, lag };
}

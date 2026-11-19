// Read-only spool gate. The page selector comes from the frozen bundle, never
// from the report whose completeness is being checked.
import fs from 'node:fs';
import { fileURLToPath } from 'node:url';
import { isDeepStrictEqual } from 'node:util';

const emptyReason = 'no payable TAP liabilities on this page';
const thresholdReason = 'provider earnings are below payout minimum';
const holdReason = 'provider earnings await challenge or holdback maturity';
const cursorPattern = /^payout\/liability\/tap\/[0-9a-f]{64}\/[0-9a-f]{64}$/;

function requireEvidence(condition, message) {
  if (!condition) throw new Error(`Invalid TAP sweep: ${message}`);
}

function amount(value) {
  requireEvidence(typeof value === 'string' && /^(0|[1-9]\d*)$/.test(value), 'invalid AU amount');
  return BigInt(value);
}

function verifyCarry(report, reason) {
  requireEvidence(report.posted === false && report.blocked !== true && report.reason === reason &&
    report.tap_settlement_checkpoint == null && report.spent_au === '0' &&
    isDeepStrictEqual(report.checkpoint_outputs, []), 'carry reason, spend or checkpoint');
  for (const field of ['root_proposed', 'root_pending', 'root_confirmed', 'root_already_posted',
    'awaiting_governance_delay', 'awaiting_finality', 'canonical_preparation_confirmed']) {
    requireEvidence(report[field] == null || report[field] === false, 'carry claims a new root or preparation');
  }
  requireEvidence(report.proposal_tx == null && report.execution_tx == null &&
    report.pending_root == null && report.root_confirmation == null &&
    report.root_preparation_id == null &&
    ['preparation_ids', 'external_effect_ids'].every(key =>
      report[key] == null || isDeepStrictEqual(report[key], [])) &&
    [report.operator_fee, report.burn].every(record =>
      record == null || (typeof record === 'object' && !Array.isArray(record) &&
        record.tx == null && record.auto_sent === false && record.predicted_claimable_wei === '0')),
  'carry contains payment evidence');
}

function verifyPageAmounts(page, report, rateLock) {
  const rate = amount(rateLock.tap_usd_au);
  requireEvidence(rate > 0n, 'zero TAP rate');
  const outputs = report.checkpoint_outputs;
  requireEvidence(Array.isArray(outputs), 'missing checkpoint outputs');
  const deferred = [];
  const aggregates = new Map();
  let paid = 0n, belowWei = 0n, held = 0n;
  let heldCount = 0, index = 0;
  for (const row of page.liabilities) {
    const revision = row.payout_revision;
    const heldAu = amount(row.held_au);
    held += heldAu;
    if (heldAu > 0n) heldCount++;
    const payable = amount(row.total_au) - heldAu - amount(row.paid_cum_au);
    requireEvidence(payable > 0n && payable >= amount(page.payout_min_au) &&
      row.chain_id === rateLock.chain_id, 'ineligible liability on resolved page');
    const wei = payable * 10n ** 18n / rate;
    if (wei === 0n) {
      deferred.push({ provider: row.provider, payout_revision: revision, to: row.target,
        payable_au: String(payable), reason: 'below_tap_wei_precision' });
      belowWei += payable;
      continue;
    }
    const aggregate = aggregates.get(row.provider) ?? amount(row.aggregate_paid_cum_au);
    aggregates.set(row.provider, aggregate + payable);
    const output = outputs[index++];
    requireEvidence(output?.role === 'provider' && output.provider === row.provider &&
      output.payout_revision === revision && output.to === row.target &&
      output.held_au === row.held_au && output.liability_total_au === row.total_au &&
      output.paid_cum_au_before === row.paid_cum_au &&
      output.aggregate_paid_cum_au_before === String(aggregate) &&
      output.net_au_paid === String(payable) && output.tap_wei === String(wei),
    'provider output differs from the pinned liability');
    paid += payable;
  }
  requireEvidence(index === outputs.length && report.spent_au === String(paid),
    'unexpected output or spent AU');
  requireEvidence(isDeepStrictEqual(report.canonical_deferred_liabilities, deferred) &&
    report.held_receipt_count === heldCount && report.held_au === String(held + belowWei) &&
    report.threshold_held_provider_count === deferred.length && report.threshold_held_au === String(belowWei),
  'below-wei evidence differs from the pinned liabilities');

  const checkpoint = report.tap_settlement_checkpoint;
  if (checkpoint != null) {
    requireEvidence(checkpoint.at === report.tap_liability_sweep.at &&
      checkpoint.fee_bps === page.fee_bps && checkpoint.provider_paid_au === String(paid) &&
      Array.isArray(checkpoint.outputs) && checkpoint.outputs.length === outputs.length &&
      outputs.every((out, i) => {
        const entry = checkpoint.outputs[i];
        return entry?.provider === out.provider && entry.payout_revision === out.payout_revision &&
          entry.to === out.to && entry.paid_cum_au_before === out.paid_cum_au_before &&
          entry.aggregate_paid_cum_au_before === out.aggregate_paid_cum_au_before &&
          entry.paid_au === out.net_au_paid && entry.tap_wei === out.tap_wei &&
          entry.prior_cumulative_claim_wei === out.previous_cumulative_wei &&
          entry.cumulative_claim_wei === out.new_cumulative_wei;
      }), 'final checkpoint differs from the pinned page');
  }
  if (index === 0) {
    const reason = page.liabilities.length === 0 ? emptyReason :
      (report.held_receipt_count > 0 ? holdReason : thresholdReason);
    verifyCarry(report, reason);
  } else {
    requireEvidence(report.status !== 'no_work' && report.outcome !== 'carry' &&
      report.carried_root == null && ![emptyReason, thresholdReason, holdReason].includes(report.reason),
    'payable page reported as a carry');
  }
}

export async function verifyTapSweepReport({ bundle, report, peerRpcUrl, rateLock,
  confirmed = false, fetchImpl = fetch }) {
  if (!Object.hasOwn(bundle, 'tap_liability_sweep')) {
    requireEvidence(!Object.hasOwn(report, 'tap_liability_sweep') && report.reason !== emptyReason &&
      Array.isArray(bundle.receipts) && bundle.receipts.length > 0,
    'page proof required for sweep evidence or a zero-receipt epoch');
    return;
  }
  const selector = bundle.tap_liability_sweep;
  requireEvidence(selector && isDeepStrictEqual(Object.keys(selector).sort(), ['after', 'schema_version', 'signed_length']) &&
    [1, 2].includes(selector.schema_version) && Number.isSafeInteger(selector.signed_length) && selector.signed_length > 0 &&
    (selector.after === null || (typeof selector.after === 'string' && cursorPattern.test(selector.after))),
  'invalid frozen page selector');
  requireEvidence(bundle.rail === 'tap' && Number.isSafeInteger(bundle.epoch) && bundle.epoch > 0 &&
    typeof bundle.epoch_apply_hash === 'string' && /^[0-9a-f]{64}$/.test(bundle.epoch_apply_hash) &&
    report.epoch === bundle.epoch && report.epoch_apply_hash === bundle.epoch_apply_hash &&
    (!Object.hasOwn(report, 'rail') || report.rail === 'tap') &&
    Array.isArray(bundle.receipts) && Number.isSafeInteger(report.receipt_count) &&
    report.receipt_count >= 0 && report.receipt_count <= bundle.receipts.length,
  'bundle and report identity or receipt count');
  requireEvidence(rateLock?.epoch === bundle.epoch && isDeepStrictEqual(rateLock, report.tap_rate_lock) &&
    Number.isSafeInteger(rateLock.rate_ts) && rateLock.rate_ts >= 0 && report.tap_usd_au === rateLock.tap_usd_au,
  'report differs from retained TAP rate lock');
  const at = bundle.at ?? rateLock.rate_ts;
  requireEvidence(Number.isSafeInteger(at) && at >= 0, 'invalid settlement time');
  const { resolveTapLiabilitySweep } = await import('./tap-liability-sweep.mjs');
  const signal = AbortSignal.timeout(60_000);
  const page = await resolveTapLiabilitySweep({ peerRpcUrl, epoch: bundle.epoch, epochApplyHash: bundle.epoch_apply_hash,
    at, signedLength: selector.signed_length, after: selector.after, schemaVersion: selector.schema_version,
    fetchImpl: (url, options) => fetchImpl(url, { ...options, signal }) });
  requireEvidence(page.signed_length === selector.signed_length && page.after === selector.after &&
    (page.next_after === null || (typeof page.next_after === 'string' && cursorPattern.test(page.next_after))) &&
    Array.isArray(page.liabilities) && Number.isSafeInteger(page.max_outputs) && page.max_outputs > 0 &&
    page.liabilities.length <= page.max_outputs, 'resolved page bounds');
  requireEvidence(isDeepStrictEqual(report.tap_liability_sweep, { ...selector, next_after: page.next_after,
    eligible_count: page.liabilities.length, at,
    ...(selector.schema_version === 2 ? { deferred: page.deferred } : {}) }), 'page cursor, timestamp or eligible count');
  const minimum = report.payout_minimum_lock;
  requireEvidence(report.payout_min_au === page.payout_min_au &&
    minimum?.type === 'tap_payout_minimum_lock' && minimum.key === 'params/payout_min_au' &&
    minimum.at === at && minimum.value === page.payout_min_au &&
    minimum.signed_length === selector.signed_length &&
    report.ledger_fee_bps === page.fee_bps && bundle.params?.fee_bps === page.fee_bps &&
    [bundle.params, bundle.audited_epoch?.params, bundle.recomputed?.params].every(params =>
      (!params || !Object.hasOwn(params, 'payout_min_au') || params.payout_min_au === page.payout_min_au) &&
      (!params || !Object.hasOwn(params, 'fee_bps') || params.fee_bps === page.fee_bps)),
  'active payout policy');
  verifyPageAmounts(page, report, rateLock);
  if (confirmed && report.checkpoint_outputs.length > 0) {
    requireEvidence(report.canonical_preparation_confirmed === true, 'missing confirmed Core preparation');
    const checkpoint = report.tap_settlement_checkpoint;
    if (checkpoint != null) {
      const { validateTapSettlementCheckpoint } = await import('../../scripts/validate-tap-settlement-checkpoint.mjs');
      validateTapSettlementCheckpoint(report);
      for (const key of ['epoch', 'epoch_apply_hash', 'root', 'cumulative_spent_wei', 'tap_rate_lock',
        'preparation_ids', 'root_preparation_id', 'external_effect_ids']) {
        requireEvidence(isDeepStrictEqual(report[key], checkpoint[key]), `checkpoint differs on ${key}`);
      }
    }
  }
}

if (import.meta.main ?? (process.argv[1] && fs.existsSync(process.argv[1]) &&
    fs.realpathSync(process.argv[1]) === fileURLToPath(import.meta.url))) {
  try {
    const [bundlePath, reportPath, peerRpcUrl, ratePath, phase] = process.argv.slice(2);
    requireEvidence(phase === 'dry' || phase === 'confirmed', 'invalid verification phase');
    const read = path => JSON.parse(fs.readFileSync(path, 'utf8'));
    const bundle = read(bundlePath);
    const rateLock = Object.hasOwn(bundle, 'tap_liability_sweep') ? read(ratePath) : undefined;
    await verifyTapSweepReport({ bundle, report: read(reportPath), peerRpcUrl, rateLock, confirmed: phase === 'confirmed' });
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}

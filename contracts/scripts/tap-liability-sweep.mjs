import { isDeepStrictEqual } from 'node:util';
import MayhemContract from '../../intercom/contract/contract.js';

const PREFIX = 'payout/liability/tap/';
const KEY = /^payout\/liability\/tap\/([0-9a-f]{64})\/([0-9a-f]{64})$/;
const HEX32 = /^[0-9a-f]{64}$/;
export const MAINNET_TOKEN = '0x5e7f6e008c6d9d7ad4c7eb75bd4ce62864cc7454';
export const MAINNET_POOL = '0xcfea9a256f1f96269d848cabf1ecb00fd2dd6a28';
const BATCH_SIZE = 32;
// Probe gates can fan out over distinct bucket epochs and auditors even on a one-row page.
const MAX_REQUESTS = 512;
const MAX_BYTES = 8 * 1024 * 1024;
const PARAMS = [
  'payout_min_au', 'fee_bps', 'max_tap_settlement_outputs', 'holdback_epochs',
  'challenge_epochs', 'canary_probe_holdback_bps', 'canary_probe_release_min_passes',
];

function requireState(condition, message) {
  if (!condition) throw new Error(message);
}

function checked(result) {
  if (result instanceof Error) throw result;
  return result;
}

const uint = (value) => Number.isSafeInteger(value) && value >= 0;
const hex32 = (value) => typeof value === 'string' && HEX32.test(value);

function validateGateRecord(key, value) {
  if (value === null) return;
  if (key.startsWith('disp/provider-open/')) {
    requireState(uint(value.count) && (value.provider === undefined ||
      key === `disp/provider-open/${value.provider}`), 'Invalid provider open dispute count');
  } else if (key.startsWith('probe/pass/')) {
    const [, , provider, epoch] = key.split('/');
    const { auditors, pass_count: count } = value;
    requireState((value.provider === undefined || value.provider === provider) &&
      (value.epoch === undefined || value.epoch === Number(epoch)) &&
      Array.isArray(auditors) && auditors.length === count && auditors.every(hex32) &&
      new Set(auditors).size === count, 'Invalid canary probe pass record');
    if (value.probes !== undefined) {
      requireState(Array.isArray(value.probes) && value.probes.length === count &&
        value.probes.every((p, i) => p?.auditor === auditors[i]), 'Inconsistent canary probe auditors');
    }
  } else if (key.startsWith('auditor/')) {
    requireState(typeof value.status === 'string' && (value.auditor === undefined ||
      key === `auditor/${value.auditor}`), 'Invalid canary auditor record');
  }
}

function snapshotReader(peerRpcUrl, signedLength, fetchImpl) {
  const endpoint = new URL('state', `${String(peerRpcUrl).replace(/\/+$/, '')}/`);
  const signal = AbortSignal.timeout(60_000);
  const cache = new Map();
  let requests = 0;
  let remainingBytes = MAX_BYTES;

  function charge(bytes) {
    remainingBytes -= bytes;
    requireState(remainingBytes >= 0, 'TAP sweep response byte limit exceeded');
  }

  async function read(query) {
    requireState(++requests <= MAX_REQUESTS, 'TAP sweep state read limit exceeded');
    const url = new URL(endpoint);
    url.search = new URLSearchParams({ ...query, confirmed: 'true', signed_length: signedLength });
    const response = await fetchImpl(url, { redirect: 'error', signal });
    requireState(response?.ok === true, `TAP sweep state read failed (HTTP ${response?.status})`);
    let body;
    if (response.body) {
      const chunks = [];
      for await (const chunk of response.body) {
        charge(chunk.byteLength);
        chunks.push(chunk);
      }
      body = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(Buffer.concat(chunks)));
    } else {
      // Also support injected JSON-only transports used by the settlement scripts.
      body = await response.json();
      charge(Buffer.byteLength(JSON.stringify(body)));
    }
    requireState(body?.confirmed === true && body.signed_length === signedLength,
      'TAP sweep response is unconfirmed or has a different signed length');
    requireState(!Object.hasOwn(body, 'error'), 'TAP sweep state response contains an error');
    return body;
  }

  async function get(key) {
    if (cache.has(key)) return cache.get(key);
    const body = await read({ key });
    requireState(body.key === key && Object.hasOwn(body, 'value') && body.value !== undefined,
      `Invalid pinned state result for ${key}`);
    const value = body.value;
    if (key !== 'admin' && value !== null) {
      requireState(typeof value === 'object' && !Array.isArray(value), `Invalid state value for ${key}`);
    }
    validateGateRecord(key, value);
    cache.set(key, value);
    return value;
  }

  async function page(after, limit) {
    const body = await read({ prefix: PREFIX, limit, ...(after === null ? {} : { after }) });
    requireState(body.prefix === PREFIX && Array.isArray(body.values) && body.values.length <= limit &&
      typeof body.truncated === 'boolean', 'Invalid TAP liability prefix page');
    let last = after;
    for (const row of body.values) {
      requireState(typeof row?.key === 'string' && KEY.test(row.key) && (last === null || row.key > last),
        'Invalid or non-advancing TAP liability key');
      last = row.key;
    }
    requireState(body.truncated
      ? body.values.length === limit && body.next_cursor === last
      : body.next_cursor === null, 'Inconsistent TAP liability page cursor');
    return body;
  }
  return { get, page };
}

export function tapContractAt(peerRpcUrl, signedLength, fetchImpl = fetch) {
  requireState(uint(signedLength) && signedLength > 0, 'Invalid signed TAP checkout length');
  const snapshot = snapshotReader(peerRpcUrl, signedLength, fetchImpl);
  return Object.assign(Object.create(MayhemContract.prototype), { get: snapshot.get });
}

// Financial policy and oracle history may have been written after the liability
// page was frozen. Read these together at a fresh confirmed Core checkout.
export async function confirmedTapContract(peerRpcUrl, fetchImpl = fetch) {
  const url = new URL('state', String(peerRpcUrl).replace(/\/+$/, '') + '/');
  url.search = new URLSearchParams({ key: 'admin', confirmed: 'true' });
  const response = await fetchImpl(url, { redirect: 'error', signal: AbortSignal.timeout(10_000) });
  requireState(response?.ok === true, 'TAP confirmed Core checkout is unavailable');
  const head = await response.json();
  requireState(head?.key === 'admin' && hex32(head.value) && head.confirmed === true &&
    !Object.hasOwn(head, 'error') && uint(head.signed_length) && head.signed_length > 0,
    'Invalid TAP confirmed Core checkout');
  const contract = tapContractAt(peerRpcUrl, head.signed_length, fetchImpl);
  requireState(await contract.get('admin') === head.value, 'Inconsistent TAP confirmed Core admin');
  contract.signedLength = head.signed_length;
  return contract;
}

async function verifyAnchor(contract, epoch, hash, at) {
  const head = await contract.get('epoch/apply/state');
  requireState(uint(head?.updated_epoch) && epoch <= head.updated_epoch &&
    hex32(head.last_apply_hash) && head.pending_epoch == null, 'Incomplete or future TAP sweep epoch');
  const current = head.updated_epoch === epoch;
  requireState(!current || head.last_apply_hash === hash, 'Current epoch apply hash mismatch');
  const stored = await contract.get(contract.epochApplyAnchorKey(epoch));
  // The contract can reconstruct older missing timestamps from history. A sweep only
  // accepts a materialized anchor or a complete matching current apply record.
  requireState(stored !== null || (current && uint(head.last_settlement_unix)),
    'Missing epoch apply anchor');
  const anchor = checked(await contract.requireEpochApplyAnchor(epoch, hash, 'TAP sweep'));
  requireState(anchor.settlement_unix === at, 'TAP liability snapshot differs from the confirmed epoch anchor');
  requireState(typeof anchor.applied_at === 'string' && anchor.applied_at.length > 0 &&
    (!current || head.updated_at === anchor.applied_at), 'Inconsistent epoch anchor and apply state');
  if (head.last_settlement_unix != null) {
    requireState(uint(head.last_settlement_unix) && anchor.settlement_unix <= head.last_settlement_unix &&
      (!current || head.last_settlement_unix === anchor.settlement_unix), 'Inconsistent epoch anchor settlement time');
  }
}

async function sweepParams(contract, at) {
  const params = checked(await contract.targetedTapParamsAt(at, PARAMS));
  checked(contract.targetedTapPayoutSplit(params.fee_bps));
  return params;
}

async function mainnetPayments(contract) {
  const admin = await contract.get('admin');
  const payments = await contract.get('payments/current');
  requireState(hex32(admin) && payments?.set_by === admin && payments.set_by_role === 'admin' &&
    uint(payments.ver) && payments.ver > 0 && Array.isArray(payments.rails) && payments.rails.includes('tap') &&
    payments.tnk?.network === 'mainnet' && payments.tap?.chain_id === 1 &&
    payments.tap.token_address?.toLowerCase() === MAINNET_TOKEN &&
    payments.tap.pool_address?.toLowerCase() === MAINNET_POOL, 'Invalid canonical mainnet TAP payment scope');
  return payments;
}

async function openProviderPreparation(contract, provider, paid) {
  const tail = await contract.get(contract.payoutPreparationAggregateTailKey('tap', provider));
  if (tail === null) return null;
  requireState(hex32(tail.economic_op_id) && typeof tail.consumed === 'boolean' &&
    typeof tail.paid_cum_au_after === 'string' && /^(0|[1-9][0-9]*)$/.test(tail.paid_cum_au_after),
  'Invalid TAP preparation tail');
  if (tail.consumed) {
    requireState(tail.paid_cum_au_after === paid, 'Consumed TAP preparation differs from aggregate watermark');
    return null;
  }
  const record = await contract.get(contract.payoutPreparationRecordKey('tap', tail.economic_op_id));
  requireState(record?.type === 'targeted_payout_preparation' && record.rail === 'tap' &&
    record.kind === 'liability' && record.consumed === false && record.economic_op_id === tail.economic_op_id &&
    record.liability?.provider === provider && hex32(record.liability.payout_revision) &&
    record.liability.aggregate_paid_cum_au_before === tail.paid_cum_au_before &&
    /^(0|[1-9][0-9]*)$/.test(tail.paid_cum_au_before) && /^[1-9][0-9]*$/.test(record.liability.paid_au) &&
    BigInt(tail.paid_cum_au_before) + BigInt(record.liability.paid_au) === BigInt(tail.paid_cum_au_after) &&
    BigInt(paid) <= BigInt(tail.paid_cum_au_before), 'TAP preparation tail has no matching outstanding liability');
  return tail.economic_op_id;
}

function verifyBinding(binding, row, payments, verified = true) {
  requireState(binding?.type === 'provider_payout_binding' && binding.verified === verified &&
    binding.provider === row.provider && binding.rail === 'tap' && binding.revision === row.revision &&
    binding.target === row.target && /^0x[0-9a-f]{40}$/.test(binding.target) && binding.currency === null &&
    binding.chain_id === 1 && row.chain_id === 1 && binding.network === 'mainnet' &&
    uint(binding.activation_epoch) && binding.activation_epoch > 0 && binding.activation_epoch <= row.updated_epoch &&
    uint(binding.payment_config_version) && binding.payment_config_version > 0 &&
    binding.payment_config_version <= payments.ver, 'Inactive or mismatched immutable TAP payout binding');
}

async function refresh(contract, record, epoch, params, gates) {
  requireState(record.updated_epoch <= epoch, 'Balance updated after the requested epoch');
  if (record.holdbacks !== undefined) {
    const valid = Array.isArray(record.holdbacks) && record.holdbacks.every(bucket =>
      bucket.epoch <= record.updated_epoch && (bucket.probe_gate === undefined || typeof bucket.probe_gate === 'boolean'));
    requireState(valid, 'Invalid holdback bucket state');
  }
  requireState((record.pool_address === undefined || record.pool_address === MAINNET_POOL) &&
    (record.token_address === undefined || record.token_address === MAINNET_TOKEN), 'Mismatched TAP balance scope');
  const probe = checked(await contract.probeGateForEarning(record.provider, record, params));
  const refreshed = checked(contract.refreshEarningHoldback(record, epoch, gates.lockedEpochs, probe, gates.disputed));
  const payable = BigInt(refreshed.total_au) - BigInt(refreshed.held_au) - BigInt(refreshed.paid_cum_au);
  requireState(payable >= 0n, 'Negative refreshed TAP payable');
  return { held: refreshed.held_au, payable };
}

async function providerBudget(contract, provider, epoch, params) {
  const registered = await contract.get(`prov/${provider}`);
  requireState(registered?.provider === provider && ['active', 'banned'].includes(registered.status),
    'TAP liability provider status is not payable');
  const lockedEpochs = checked(contract.providerLockedEarningEpochs(registered, params));
  const disputed = checked(await contract.providerHasOpenDispute(provider));
  const gates = { lockedEpochs, disputed };
  // earningRecord() synthesizes a zero record on missing or mismatched pool scope.
  const earning = await contract.get(contract.earningKey(provider, 'tap'));
  checked(contract.guardianValidateEarningRecord(earning, provider, 'tap'));
  requireState(earning.chain_id === 1 && earning.pool_address === MAINNET_POOL, 'Mismatched TAP aggregate scope');
  const { payable } = await refresh(contract, earning, epoch, params, gates);
  return { gates, payable, paid: earning.paid_cum_au, scannedPaid: 0n, selected: 0n };
}

// Old v1/v2 selectors always use the original page width. A replacement for
// a failed draft can freeze a single-row page to isolate an oversized output.
export function validTapSweepPointer(p) {
  if (!p || typeof p !== 'object' || Array.isArray(p)) return false;
  const single = Object.hasOwn(p, 'limit');
  return Object.keys(p).length === (single ? 4 : 3) &&
    ['schema_version', 'signed_length', 'after'].every(k => Object.hasOwn(p, k)) &&
    [1, 2].includes(p.schema_version) && uint(p.signed_length) && p.signed_length > 0 &&
    (!single || (p.schema_version === 2 && p.limit === 1)) &&
    (p.after === null || (typeof p.after === 'string' && KEY.test(p.after)));
}

// Every TAP root preparation follows a nonempty list of provider preparations.
// Their immutable watermark locks cover all outputs the frozen selector could
// ever have reserved, even when binding validation prevents rebuilding a root.
export async function tapDraftWatermarks({ peerRpcUrl, bundle, fetchImpl }) {
  const p = bundle.tap_liability_sweep;
  requireState(validTapSweepPointer(p), 'Invalid TAP draft selector');
  const snapshot = snapshotReader(peerRpcUrl, p.signed_length, fetchImpl);
  const contract = Object.assign(Object.create(MayhemContract.prototype), { get: snapshot.get });
  await verifyAnchor(contract, bundle.epoch, bundle.epoch_apply_hash, bundle.at);
  await mainnetPayments(contract);
  const params = await sweepParams(contract, bundle.at);
  const page = await snapshot.page(p.after, Math.min(p.limit ?? BATCH_SIZE, params.max_tap_settlement_outputs));
  const rows = page.values.map(({ key, value }) => {
    const [, provider, revision] = KEY.exec(key);
    checked(contract.guardianValidatePayoutLiabilityRecord(value, provider, 'tap', revision));
    return { provider, revision, before: value.paid_cum_au, target: value.target, chainId: value.chain_id };
  });
  return { rows, next: page.next_cursor };
}

/**
 * Read one confirmed TAP liability page at signedLength. Resume at the same pin
 * with next_after, the last scanned full key. A null next_after ends the scan.
 * max_outputs is the active contract limit. Each row's payable AU equals its
 * total_au minus held_au minus paid_cum_au. All AU fields are decimal strings.
 */
export async function resolveTapLiabilitySweep({
  peerRpcUrl, epoch, epochApplyHash, at, signedLength, after = null, schemaVersion = 1, limit, fetchImpl = fetch,
}) {
  requireState(uint(signedLength) && signedLength > 0 && uint(epoch) && epoch > 0 &&
    hex32(epochApplyHash) && uint(at), 'Invalid pinned TAP sweep inputs');
  requireState(after === null || (typeof after === 'string' && KEY.test(after)), 'Invalid TAP sweep after cursor');
  requireState(schemaVersion === 1 || schemaVersion === 2, 'Unknown TAP liability sweep version');
  const snapshot = snapshotReader(peerRpcUrl, signedLength, fetchImpl);
  // Only read-only reducer methods are used. No peer, wallet or writable storage is constructed.
  const contract = Object.assign(Object.create(MayhemContract.prototype), { get: snapshot.get });
  await verifyAnchor(contract, epoch, epochApplyHash, at);
  const payments = await mainnetPayments(contract);
  const params = await sweepParams(contract, at);
  const maxOutputs = params.max_tap_settlement_outputs;
  requireState(limit === undefined || (schemaVersion === 2 && limit === 1), 'Invalid TAP recovery page limit');
  const page = await snapshot.page(after, Math.min(limit ?? BATCH_SIZE, maxOutputs));
  const minimum = BigInt(params.payout_min_au);
  const providers = new Map();
  const liabilities = [];
  const deferred = [];
  for (const { key, value: row } of page.values) {
    const [, provider, revision] = KEY.exec(key);
    checked(contract.guardianValidatePayoutLiabilityRecord(row, provider, 'tap', revision));
    const binding = await contract.get(contract.providerPayoutBindingFeatureKey('tap', provider, revision));
    const unavailable = schemaVersion === 2 && (binding === null || binding.verified === false);
    if (!unavailable || binding !== null) verifyBinding(binding, row, payments, !unavailable);
    if (!providers.has(provider)) {
      const budget = await providerBudget(contract, provider, epoch, params);
      if (schemaVersion === 2) budget.preparation = await openProviderPreparation(contract, provider, budget.paid);
      providers.set(provider, budget);
    }
    const budget = providers.get(provider);
    const { held, payable } = await refresh(contract, row, epoch, params, budget.gates);
    budget.scannedPaid += BigInt(row.paid_cum_au);
    requireState(budget.scannedPaid <= BigInt(budget.paid), 'Revision paid exceeds aggregate paid watermark');
    if (payable === 0n || payable < minimum) continue;
    if (unavailable || budget.preparation) {
      // Keep every held entitlement in the canonical liability ledger. A later sweep
      // reconsiders it after the binding or the provider's cumulative reservation clears.
      const reason = unavailable ? { reason: 'unverified_binding' }
        : { reason: 'outstanding_preparation', economic_op_id: budget.preparation };
      deferred.push({ provider, payout_revision: revision, ...reason });
      continue;
    }
    budget.selected += payable;
    requireState(budget.selected <= budget.payable, 'Selected revisions exceed refreshed aggregate payable');
    liabilities.push({
      provider, rail: 'tap', payout_revision: revision, target: binding.target, chain_id: binding.chain_id,
      total_au: row.total_au, held_au: held, paid_cum_au: row.paid_cum_au,
      aggregate_paid_cum_au: budget.paid, updated_epoch: row.updated_epoch,
    });
  }
  return {
    signed_length: signedLength, after, next_after: page.next_cursor, payout_min_au: params.payout_min_au,
    fee_bps: params.fee_bps, max_outputs: maxOutputs, liabilities,
    ...(schemaVersion === 2 ? { deferred } : {}),
  };
}

// Rehearse the complete ordered plan at a fresh confirmed Core checkout before
// reserving its first provider. A frozen page can outlive its original budget.
export async function preflightTapSweepPreparations(plan, peerRpcUrl, fetchImpl, prior, verifyExisting) {
  const current = await confirmedTapContract(peerRpcUrl, fetchImpl);
  if (prior?.root) {
    const record = await current.get('settle/targeted/tap/' + prior.epoch);
    requireState(record?.type === 'targeted_tap_settlement' && record.epoch === prior.epoch &&
      record.epoch_apply_hash === prior.epoch_apply_hash && record.root_confirmed === true &&
      record.root === prior.root && record.cumulative_spent_wei === prior.cumulative_spent_wei &&
      isDeepStrictEqual(record.entries, prior.entries) && isDeepStrictEqual(record.provider_entries, prior.providers) &&
      isDeepStrictEqual(record.refunds, prior.refunds), 'TAP prior claim distribution differs from confirmed Core');
  }
  const staged = new Map();
  const view = Object.assign(Object.create(current), {
    get: key => staged.has(key) ? staged.get(key) : current.get(key),
  });
  const { epoch, at, epoch_apply_hash: anchor, outputs } = plan.intent;
  checked(await view.targetedPayoutSettlementUpdates(
    outputs.map(row => ({ ...row, role: 'provider', au: row.paid_au })), 'tap', epoch, at,
    plan.preparation_ids
  ));
  for (const p of plan.preparations) {
    const key = view.payoutPreparationRecordKey('tap', p.economic_op_id);
    if (await view.get(key) !== null) {
      const record = checked(await view.validatePayoutPreparationRecord({ rail: 'tap', epoch, epochApplyHash: anchor,
        economicOpId: p.economic_op_id, kind: p.kind, outputIndex: p.output_index,
        payload: p.payload, liability: p.liability, externalEffectIds: p.external_effect_ids }));
      if (verifyExisting) await verifyExisting(record, p, await current.get('admin'));
      continue;
    }
    if (p.liability) {
      const lock = view.payoutPreparationLiabilityLockKey('tap', p.liability);
      requireState(await view.get(lock) === null, 'TAP liability already reserved');
      const state = checked(await view.validatePayoutPreparationLiability({
        rail: 'tap', epoch, prepared_at: at, liability: p.liability,
      }));
      staged.set(lock, { economic_op_id: p.economic_op_id });
      staged.set(state.tail_key, { economic_op_id: p.economic_op_id,
        paid_cum_au_before: p.liability.aggregate_paid_cum_au_before,
        paid_cum_au_after: state.paid_cum_au_after, consumed: false });
    }
    for (const id of p.external_effect_ids) {
      requireState(await view.get(view.payoutPreparationEffectLockKey('tap', id)) === null,
        'TAP root effect already reserved');
    }
  }
  checked(await view.validatePayoutPreparationConsumption(plan.preparations.map(p => ({ ...p, rail: 'tap' }))));
}

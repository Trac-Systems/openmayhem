import MayhemContract from '../../intercom/contract/contract.js';

const PREFIX = 'payout/liability/tap/';
const KEY = /^payout\/liability\/tap\/([0-9a-f]{64})\/([0-9a-f]{64})$/;
const HEX32 = /^[0-9a-f]{64}$/;
const MAINNET_TOKEN = '0x5e7f6e008c6d9d7ad4c7eb75bd4ce62864cc7454';
const MAINNET_POOL = '0xcfea9a256f1f96269d848cabf1ecb00fd2dd6a28';
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

async function verifyAnchor(contract, epoch, hash) {
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
  requireState(typeof anchor.applied_at === 'string' && anchor.applied_at.length > 0 &&
    (!current || head.updated_at === anchor.applied_at), 'Inconsistent epoch anchor and apply state');
  if (head.last_settlement_unix != null) {
    requireState(uint(head.last_settlement_unix) && anchor.settlement_unix <= head.last_settlement_unix &&
      (!current || head.last_settlement_unix === anchor.settlement_unix), 'Inconsistent epoch anchor settlement time');
  }
}

async function sweepParams(contract, at) {
  const params = {};
  for (const key of PARAMS) {
    const record = await contract.paramRecord(key); // Includes the reducer default for an absent schedule.
    requireState(record.key === key && uint(record.current?.effective_at) && uint(record.current.ver) &&
      (record.pending == null || (uint(record.pending.effective_at) &&
        uint(record.pending.ver) && record.pending.ver > record.current.ver &&
        record.pending.effective_at >= record.current.effective_at)), `Invalid parameter schedule for ${key}`);
    const active = contract.paramActiveEntry(record, at);
    requireState(active.effective_at <= at, `No active value for ${key} at preparation time`);
    params[key] = active.value;
  }
  checked(contract.validateParamValues(params));
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

function verifyBinding(binding, row, payments) {
  requireState(binding?.type === 'provider_payout_binding' && binding.verified === true &&
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

/**
 * Read one confirmed TAP liability page at signedLength. Resume at the same pin
 * with next_after, the last scanned full key. A null next_after ends the scan.
 * max_outputs is the active contract limit. Each row's payable AU equals its
 * total_au minus held_au minus paid_cum_au. All AU fields are decimal strings.
 */
export async function resolveTapLiabilitySweep({
  peerRpcUrl, epoch, epochApplyHash, at, signedLength, after = null, fetchImpl = fetch,
}) {
  requireState(uint(signedLength) && signedLength > 0 && uint(epoch) && epoch > 0 &&
    hex32(epochApplyHash) && uint(at), 'Invalid pinned TAP sweep inputs');
  requireState(after === null || (typeof after === 'string' && KEY.test(after)), 'Invalid TAP sweep after cursor');
  const snapshot = snapshotReader(peerRpcUrl, signedLength, fetchImpl);
  // Only read-only reducer methods are used. No peer, wallet or writable storage is constructed.
  const contract = Object.assign(Object.create(MayhemContract.prototype), { get: snapshot.get });
  await verifyAnchor(contract, epoch, epochApplyHash);
  const payments = await mainnetPayments(contract);
  const params = await sweepParams(contract, at);
  const maxOutputs = params.max_tap_settlement_outputs;
  const page = await snapshot.page(after, Math.min(BATCH_SIZE, maxOutputs));
  const minimum = BigInt(params.payout_min_au);
  const providers = new Map();
  const liabilities = [];
  for (const { key, value: row } of page.values) {
    const [, provider, revision] = KEY.exec(key);
    checked(contract.guardianValidatePayoutLiabilityRecord(row, provider, 'tap', revision));
    const binding = await contract.get(contract.providerPayoutBindingFeatureKey('tap', provider, revision));
    verifyBinding(binding, row, payments);
    if (!providers.has(provider)) providers.set(provider, await providerBudget(contract, provider, epoch, params));
    const budget = providers.get(provider);
    const { held, payable } = await refresh(contract, row, epoch, params, budget.gates);
    budget.scannedPaid += BigInt(row.paid_cum_au);
    requireState(budget.scannedPaid <= BigInt(budget.paid), 'Revision paid exceeds aggregate paid watermark');
    if (payable === 0n || payable < minimum) continue;
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
  };
}

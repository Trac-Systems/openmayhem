// Canonical proxy protocol policy, controlled by the existing application admin.
// This approves interface/metering semantics, not individual providers or models.
import b4a from 'b4a';
import { blake3 } from '@tracsystems/blake3';
import { ProxyValidationError, PROXY_MAX_RECORD_BYTES } from './proxy-protocol.js';
import { PROXY_PREFIX, proxyRegistryKeys, validateProxyRegistryConfig } from './proxy-registry.js';
import { withProxyDiscoveryWrites } from './proxy-discovery.js';
import { validateProxySettlementPolicy, proxySettlementPolicyDigest } from './proxy-finance.js';

const check = (ok, message) => { if (!ok) throw new ProxyValidationError(message); };
const hex = value => check(typeof value === 'string' && /^[0-9a-f]{64}$/.test(value), 'invalid proxy policy digest');
const integer = value => check(Number.isSafeInteger(value) && value > 0, 'invalid proxy policy revision/limit');
const id = value => check(typeof value === 'string' && /^[a-z][a-z0-9_-]{0,63}$/.test(value), 'invalid proxy policy identifier');
const bool = value => check(typeof value === 'boolean', 'invalid proxy policy status');
function shape(value, names) {
  check(value !== null && typeof value === 'object' && !Array.isArray(value), 'invalid proxy policy object');
  const keys = Object.keys(value).sort();
  const expected = [...names].sort();
  check(keys.length === expected.length && keys.every((key, i) => key === expected[i]), 'invalid proxy policy fields');
}
function sorted(values, max, validate) {
  check(Array.isArray(values) && values.length > 0 && values.length <= max, 'invalid proxy policy list');
  for (let i = 0; i < values.length; i++) {
    validate(values[i]);
    check(i === 0 || values[i - 1] < values[i], 'proxy policy list must be sorted/distinct');
  }
}
function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === 'object') return Object.fromEntries(Object.keys(value).sort().map(k => [k, canonical(value[k])]));
  return value;
}

export function validateProxyPolicy(value) {
  shape(value, ['op', 'context', 'revision', 'action']);
  check(value.op === 'proxy_policy', 'invalid proxy policy operation');
  shape(value.context, ['network_id', 'msb_bootstrap', 'subnet_bootstrap', 'contract_version']);
  check(typeof value.context.network_id === 'string' && /^[a-z0-9_-]{1,128}$/.test(value.context.network_id), 'invalid proxy policy network');
  hex(value.context.msb_bootstrap); hex(value.context.subnet_bootstrap); integer(value.context.contract_version);
  check(value.context.contract_version <= 0xffff_ffff, 'invalid proxy policy contract version');
  integer(value.revision);
  const action = value.action;
  check(action !== null && typeof action === 'object', 'invalid proxy policy action');
  switch (action.kind) {
    case 'configure': {
      shape(action, ['kind', 'config']);
      const config = action.config;
      shape(config, ['network_id', 'msb_bootstrap', 'subnet_bootstrap', 'contract_version', 'enabled', 'fee_policy_hash',
        'active_issuers', 'max_permit_epochs', 'max_mutations_per_provider_epoch', 'max_mutations_per_epoch',
        'max_active_memberships', 'max_created_markets_per_provider_epoch', 'max_offer_slots_per_membership']);
      check(typeof config.network_id === 'string' && /^[a-z0-9_-]{1,128}$/.test(config.network_id), 'invalid proxy policy network');
      hex(config.msb_bootstrap); hex(config.subnet_bootstrap); integer(config.contract_version);
      check(config.contract_version <= 0xffff_ffff, 'invalid proxy policy contract version');
      validateProxyRegistryConfig(config, config);
      break;
    }
    case 'set_family':
      shape(action, ['kind', 'family_id', 'label', 'enabled']);
      id(action.family_id); bool(action.enabled);
      check(typeof action.label === 'string' && /^[\x20-\x7e]{1,128}$/.test(action.label), 'invalid proxy family label');
      break;
    case 'set_endpoint': {
      shape(action, ['kind', 'contract_hash', 'policy']);
      hex(action.contract_hash);
      const policy = action.policy;
      shape(policy, ['enabled', 'endpoint', 'family', 'max_context', 'ctx_brackets', 'outcome_classes']);
      bool(policy.enabled); integer(policy.max_context);
      check(policy.max_context <= 0xffff_ffff, 'invalid proxy endpoint maximum context');
      check(['openai_chat_completions', 'openai_completions', 'openai_responses', 'mayhem_decisions'].includes(policy.endpoint), 'unsupported proxy endpoint');
      check(policy.family === (policy.endpoint === 'mayhem_decisions' ? 'decisions' : 'llm'), 'proxy endpoint family mismatch');
      sorted(policy.ctx_brackets, 32, id);
      sorted(policy.outcome_classes, 32, outcome => {
        if (outcome !== '') { hex(outcome); check(policy.family === 'decisions', 'outcome class requires decisions'); }
      });
      break;
    }
    case 'set_metering':
      shape(action, ['kind', 'policy_hash', 'policy']);
      hex(action.policy_hash);
      shape(action.policy, ['enabled', 'units']);
      bool(action.policy.enabled);
      sorted(action.policy.units, 16, id);
      // The hash identifies an implementation-reviewed metering contract. The
      // connector/receipt verifier must implement it before routing can admit it.
      break;
    case 'set_settlement':
      shape(action, ['kind', 'policy_hash', 'enabled', 'policy']);
      hex(action.policy_hash); bool(action.enabled); validateProxySettlementPolicy(action.policy);
      break;
    case 'set_status':
      shape(action, ['kind', 'scope', 'id', 'revoked', 'reason_hash']);
      check(['provider', 'admission'].includes(action.scope), 'invalid proxy revocation scope');
      hex(action.id); hex(action.reason_hash); bool(action.revoked);
      break;
    case 'set_admission_generation':
      shape(action, ['kind', 'entitlement_id', 'issuance_revision', 'permit_digest']);
      hex(action.entitlement_id); hex(action.permit_digest); integer(action.issuance_revision);
      break;
    default: throw new ProxyValidationError('unsupported proxy policy action');
  }
  check(b4a.byteLength(JSON.stringify(value)) <= PROXY_MAX_RECORD_BYTES, 'proxy policy exceeds size limit');
  return value;
}

export async function proxyPolicyFeatureKey(value) {
  validateProxyPolicy(value);
  const bytes = b4a.from(`mayhem/proxy/policy/v1\0${JSON.stringify(canonical(value))}`);
  return `proxy/policy/${b4a.toString(await blake3(bytes), 'hex')}`;
}

// Require the current canonical admin BEFORE calling; authority is not supplied
// by this record. Like registry mutation, this returns only a fully validated plan.
export async function prepareProxyPolicyMutation(value, context, read) {
  validateProxyPolicy(value);
  value = JSON.parse(JSON.stringify(value));
  for (const field of Object.keys(value.context)) check(value.context[field] === context[field], 'proxy policy network/contract mismatch');
  const operationKey = await proxyPolicyFeatureKey(value);
  const headKey = `${PROXY_PREFIX}policy-head`;
  const head = await read(headKey);
  if (head?.revision === value.revision && head?.operation_key === operationKey) return { duplicate: true, writes: [], result: head.result };
  check(value.revision === (head?.revision ?? 0) + 1, 'stale or out-of-order proxy policy');
  const action = value.action;
  let target;
  let record;
  switch (action.kind) {
    case 'configure':
      validateProxyRegistryConfig(action.config, context);
      target = proxyRegistryKeys.config; record = action.config;
      break;
    case 'set_family':
      target = `${PROXY_PREFIX}family/${action.family_id}`;
      record = { enabled: action.enabled, label: action.label };
      break;
    case 'set_endpoint':
      target = `${PROXY_PREFIX}endpoint-policy/${action.contract_hash}`; record = action.policy;
      break;
    case 'set_metering':
      target = `${PROXY_PREFIX}metering-policy/${action.policy_hash}`; record = action.policy;
      break;
    case 'set_settlement':
      check(await proxySettlementPolicyDigest(action.policy) === action.policy_hash,
        'proxy settlement policy hash mismatch');
      target = `${PROXY_PREFIX}settlement-policy/${action.policy_hash}`;
      record = { enabled: action.enabled, policy: action.policy };
      break;
    case 'set_status':
      target = `${PROXY_PREFIX}${action.scope}-revoked/${action.id}`;
      record = action.revoked ? { reason_hash: action.reason_hash, revision: value.revision } : null;
      break;
    case 'set_admission_generation': {
      target = `${PROXY_PREFIX}admission-generation/${action.entitlement_id}`;
      const previous = await read(target);
      check(action.issuance_revision > (previous?.revision ?? 0), 'proxy admission generation must increase');
      const used = await read(`${PROXY_PREFIX}admission-used/entitlement/${action.entitlement_id}`);
      check(!used, 'proxy admission already consumed; use explicit revocation');
      record = { revision: action.issuance_revision, permit_digest: action.permit_digest };
      break;
    }
  }
  if (action.kind === 'set_endpoint' || action.kind === 'set_metering' || action.kind === 'set_settlement') {
    const old = await read(target);
    if (old) {
      const { enabled: oldEnabled, ...oldDefinition } = old;
      const { enabled: newEnabled, ...newDefinition } = record;
      check(JSON.stringify(canonical(oldDefinition)) === JSON.stringify(canonical(newDefinition)), 'proxy contract hash cannot change meaning');
    }
  }
  const result = { revision: value.revision, operation_key: operationKey, action: action.kind };
  return { duplicate: false, result, writes: withProxyDiscoveryWrites([{ key: target, value: record },
    { key: headKey, value: { ...result, result } }]) };
}

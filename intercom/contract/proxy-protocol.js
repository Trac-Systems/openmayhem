// Proxy wire validation shared by ingress and deterministic application rules.
// Structural validity is not entitlement, signature verification or live capacity.
// Keep in sync with mayhem-proto/src/proxy.rs and the shared wire fixtures.
import b4a from 'b4a';
import { blake3 } from '@tracsystems/blake3';

export const PROXY_SCHEMA_VERSION = 1;
export const PROXY_MAX_RECORD_BYTES = 16_384;
export const PROXY_MARKET_DOMAIN = 'mayhem/proxy/market/v1';
export const PROXY_MEMBERSHIP_DOMAIN = 'mayhem/proxy/membership/v1';
export const PROXY_OFFER_DOMAIN = 'mayhem/proxy/offer/v1';
export const PROXY_ADMISSION_DOMAIN = 'mayhem/proxy/admission/v1';
export const PROXY_ADMISSION_FEE_AU = '10000000000000000000';
const U128_MAX = (1n << 128n) - 1n;
const U32_MAX = 0xffff_ffff;
const RAILS = ['fiat', 'tap', 'tnk'];
const ENDPOINTS = ['mayhem_decisions', 'openai_chat_completions', 'openai_completions', 'openai_responses'];

function requireValue(condition, message) {
  if (!condition) throw new Error(message);
}

function shape(value, fields) {
  requireValue(value !== null && typeof value === 'object' && !Array.isArray(value), 'proxy record must be an object');
  const keys = Object.keys(value).sort();
  const expected = [...fields].sort();
  requireValue(keys.length === expected.length && keys.every((key, i) => key === expected[i]),
    'proxy record has missing or unsupported fields');
}

function hex(value) {
  requireValue(typeof value === 'string' && /^[0-9a-f]{64}$/.test(value), 'proxy digest/key must be lowercase 64-hex');
}

function identifier(value) {
  requireValue(typeof value === 'string' && /^[a-z][a-z0-9_-]{0,63}$/.test(value), 'invalid proxy identifier');
}

function integer(value, minimum = 1, maximum = Number.MAX_SAFE_INTEGER) {
  requireValue(Number.isSafeInteger(value) && value >= minimum && value <= maximum, 'invalid proxy integer');
}

function money(value) {
  requireValue(typeof value === 'string' && /^(0|[1-9][0-9]{0,38})$/.test(value), 'proxy money must be a canonical decimal string');
  const amount = BigInt(value);
  requireValue(amount <= U128_MAX, 'proxy money exceeds u128');
  return amount;
}

function sorted(values, maximum, key = value => value) {
  requireValue(Array.isArray(values) && values.length > 0 && values.length <= maximum, 'invalid proxy list length');
  requireValue(values.every((value, i) => i === 0 || key(values[i - 1]) < key(value)), 'proxy list must be distinct and sorted');
}

function rails(values) {
  sorted(values, RAILS.length);
  requireValue(values.every(value => RAILS.includes(value)), 'unsupported proxy rail');
}

function endpoints(values) {
  sorted(values, 3, value => value?.endpoint);
  for (const value of values) {
    shape(value, ['endpoint', 'contract_hash']);
    requireValue(ENDPOINTS.includes(value.endpoint), 'unsupported proxy endpoint');
    hex(value.contract_hash);
  }
}

function units(values) {
  sorted(values, 16);
  values.forEach(identifier);
}

function base(value) {
  requireValue(value.schema_version === PROXY_SCHEMA_VERSION, 'unsupported proxy schema version');
  requireValue(value.lane === 'proxy', 'proxy lane required');
}

function modelText(value) {
  requireValue(typeof value === 'string' && b4a.byteLength(value, 'utf8') <= 512
    && !/[\u0000-\u001f\u007f-\u009f]/.test(value), 'invalid proxy model claim');
  // Rust strings cannot contain unpaired UTF-16 surrogates. Reject them here too.
  for (const character of value) {
    const code = character.codePointAt(0);
    requireValue(code < 0xd800 || code > 0xdfff, 'invalid proxy model Unicode');
  }
}

function canonicalBody(value) {
  function ordered(value) {
    if (typeof value === 'number') {
      integer(value, 0);
      return value;
    }
    if (Array.isArray(value)) return value.map(ordered);
    if (value !== null && typeof value === 'object') {
      return Object.fromEntries(Object.keys(value).sort().map(key => [key, ordered(value[key])]));
    }
    return value;
  }
  const body = JSON.stringify(ordered(value));
  requireValue(b4a.byteLength(body, 'utf8') <= PROXY_MAX_RECORD_BYTES, 'proxy record exceeds size bound');
  return body;
}

function signingBytes(domain, value) {
  return b4a.from(`${domain}\0${canonicalBody(value)}`, 'utf8');
}

export function validateProxyMarket(value) {
  shape(value, ['schema_version', 'lane', 'creator_pubkey', 'slug', 'model', 'family', 'endpoints', 'metering', 'pricing']);
  base(value);
  hex(value.creator_pubkey);
  identifier(value.slug);
  shape(value.model, ['family_id', 'model_id', 'revision', 'quantization']);
  identifier(value.model.family_id);
  [value.model.model_id, value.model.revision, value.model.quantization].forEach(modelText);
  requireValue(value.family === 'llm' || value.family === 'decisions', 'unsupported proxy family');
  endpoints(value.endpoints);
  requireValue(value.endpoints.every(e => (e.endpoint === 'mayhem_decisions' ? 'decisions' : 'llm') === value.family),
    'proxy endpoint/family mismatch');
  shape(value.metering, ['policy_hash', 'units']);
  hex(value.metering.policy_hash);
  units(value.metering.units);
  requireValue(value.pricing === 'provider_offers', 'unsupported proxy pricing mechanism');
  canonicalBody(value);
  return value;
}

export function proxyMarketSigningBytes(value) {
  validateProxyMarket(value);
  return signingBytes(PROXY_MARKET_DOMAIN, value);
}

export async function proxyMarketId(value) {
  return b4a.toString(await blake3(proxyMarketSigningBytes(value)), 'hex');
}

export function proxyMarketHandle(value) {
  validateProxyMarket(value);
  return `proxy/${value.creator_pubkey}/${value.slug}`;
}

export function validateProxyMembership(value) {
  shape(value, ['schema_version', 'lane', 'market_id', 'provider_pubkey', 'revision', 'endpoints',
    'served_context', 'max_concurrency', 'recipe_hash', 'connection_revision', 'capacity_group', 'accepted_rails']);
  base(value);
  [value.market_id, value.provider_pubkey, value.recipe_hash, value.capacity_group].forEach(hex);
  integer(value.revision);
  integer(value.connection_revision);
  integer(value.served_context, 1, U32_MAX);
  integer(value.max_concurrency, 1, U32_MAX);
  endpoints(value.endpoints);
  rails(value.accepted_rails);
  canonicalBody(value);
  return value;
}

export async function validateProxyMembershipForMarket(value, market) {
  validateProxyMembership(value);
  requireValue(value.market_id === await proxyMarketId(market), 'proxy membership market mismatch');
  requireValue(value.endpoints.every(e => market.endpoints.some(m => m.endpoint === e.endpoint && m.contract_hash === e.contract_hash)),
    'proxy membership endpoint contract mismatch');
  return value;
}

export function proxyMembershipSigningBytes(value) {
  validateProxyMembership(value);
  return signingBytes(PROXY_MEMBERSHIP_DOMAIN, value);
}

export async function proxyMembershipDigest(value) {
  return b4a.toString(await blake3(proxyMembershipSigningBytes(value)), 'hex');
}

export function validateProxyAdmissionPermit(value) {
  shape(value, ['schema_version', 'lane', 'purpose', 'network_id', 'contract_version', 'provider_pubkey',
    'issuer_pubkey', 'entitlement_id', 'fee_policy_hash', 'invoice_commitment', 'evidence_commitment',
    'initial_operation_digest', 'nonce', 'issuance_revision', 'rail', 'accepted_amount', 'accepted_value_au',
    'valid_from_epoch', 'expires_after_epoch']);
  base(value);
  requireValue(value.purpose === 'proxy_admission_fee', 'wrong proxy admission purpose');
  requireValue(typeof value.network_id === 'string' && /^[a-z0-9_-]{1,128}$/.test(value.network_id), 'invalid proxy admission network');
  integer(value.contract_version, 1, U32_MAX);
  for (const field of ['provider_pubkey', 'issuer_pubkey', 'entitlement_id', 'fee_policy_hash',
    'invoice_commitment', 'evidence_commitment', 'initial_operation_digest', 'nonce']) hex(value[field]);
  integer(value.issuance_revision);
  integer(value.valid_from_epoch);
  integer(value.expires_after_epoch);
  requireValue(value.expires_after_epoch >= value.valid_from_epoch, 'invalid proxy admission epoch window');
  requireValue(RAILS.includes(value.rail), 'unsupported proxy rail');
  requireValue(money(value.accepted_amount) > 0n && money(value.accepted_value_au) === BigInt(PROXY_ADMISSION_FEE_AU),
    'proxy admission must attest exactly the agreed fee allocation');
  canonicalBody(value);
  return value;
}

export function proxyAdmissionSigningBytes(value) {
  validateProxyAdmissionPermit(value);
  return signingBytes(PROXY_ADMISSION_DOMAIN, value);
}

export async function proxyAdmissionDigest(value) {
  return b4a.toString(await blake3(proxyAdmissionSigningBytes(value)), 'hex');
}

// Context comes from one confirmed canonical checkout, never the request body.
// This validates bindings/signature only; the registry separately enforces unused
// evidence/entitlement, issuer/revision revocation and the provider's operation signature.
export async function verifyProxyAdmissionPermit(envelope, context, verifySignature) {
  shape(envelope, ['permit', 'issuer_signature']);
  const value = validateProxyAdmissionPermit(envelope.permit);
  requireValue(typeof envelope.issuer_signature === 'string' && /^[0-9a-f]{128}$/.test(envelope.issuer_signature), 'invalid proxy issuer signature');
  shape(context, ['network_id', 'contract_version', 'provider_pubkey', 'initial_operation_digest',
    'fee_policy_hash', 'epoch', 'max_permit_epochs', 'active_issuers']);
  integer(context.epoch);
  integer(context.max_permit_epochs);
  integer(context.contract_version, 1, U32_MAX);
  requireValue(value.network_id === context.network_id && value.contract_version === context.contract_version,
    'proxy admission network/contract mismatch');
  requireValue(value.provider_pubkey === context.provider_pubkey && value.initial_operation_digest === context.initial_operation_digest,
    'proxy admission provider/operation mismatch');
  requireValue(value.fee_policy_hash === context.fee_policy_hash, 'proxy admission fee policy mismatch');
  requireValue(value.valid_from_epoch <= context.epoch && context.epoch <= value.expires_after_epoch
    && value.expires_after_epoch - value.valid_from_epoch + 1 <= context.max_permit_epochs, 'proxy admission outside permitted epoch window');
  requireValue(Array.isArray(context.active_issuers) && context.active_issuers.length > 0
    && context.active_issuers.length <= 16 && context.active_issuers.includes(value.issuer_pubkey), 'proxy admission issuer not authorized');
  context.active_issuers.forEach(hex);
  requireValue(typeof verifySignature === 'function' && await verifySignature(envelope.issuer_signature,
    proxyAdmissionSigningBytes(value), value.issuer_pubkey) === true, 'invalid proxy issuer signature');
  return value;
}

export function validateProxyOffer(value) {
  shape(value, ['schema_version', 'lane', 'market_id', 'provider_pubkey', 'membership_revision', 'revision',
    'endpoint', 'ctx_bracket', 'outcome_class', 'metering_policy_hash', 'rates', 'per_request_au', 'min_session_au', 'accepted_rails']);
  base(value);
  [value.market_id, value.provider_pubkey, value.metering_policy_hash].forEach(hex);
  integer(value.membership_revision);
  integer(value.revision);
  requireValue(ENDPOINTS.includes(value.endpoint), 'unsupported proxy endpoint');
  identifier(value.ctx_bracket);
  requireValue(typeof value.outcome_class === 'string', 'invalid proxy outcome class');
  if (value.outcome_class !== '') {
    hex(value.outcome_class);
    requireValue(value.endpoint === 'mayhem_decisions', 'outcome class requires a decisions endpoint');
  }
  sorted(value.rates, 16, value => value?.unit);
  for (const rate of value.rates) {
    shape(rate, ['unit', 'per_unit_au', 'granularity']);
    identifier(rate.unit);
    money(rate.per_unit_au);
    integer(rate.granularity);
  }
  const perRequest = money(value.per_request_au);
  const minimum = money(value.min_session_au);
  requireValue(perRequest > 0n || minimum > 0n || value.rates.some(r => money(r.per_unit_au) > 0n),
    'free proxy offers require a separately supported accounting policy');
  rails(value.accepted_rails);
  canonicalBody(value);
  return value;
}

export async function validateProxyOfferForMembership(value, market, member) {
  validateProxyOffer(value);
  await validateProxyMembershipForMarket(member, market);
  requireValue(value.market_id === member.market_id && value.provider_pubkey === member.provider_pubkey
    && value.membership_revision === member.revision, 'proxy offer membership binding mismatch');
  requireValue(member.endpoints.some(e => e.endpoint === value.endpoint), 'proxy offer endpoint unavailable in membership');
  requireValue(value.metering_policy_hash === market.metering.policy_hash, 'proxy offer metering policy mismatch');
  requireValue(value.rates.length === market.metering.units.length
    && value.rates.every((r, i) => r.unit === market.metering.units[i]), 'proxy offer must price every metering unit exactly once');
  requireValue(value.accepted_rails.every(r => member.accepted_rails.includes(r)), 'proxy offer rail not enabled by membership');
  return value;
}

export function validateProxyOfferRevision(value, lastRevision) {
  validateProxyOffer(value);
  integer(lastRevision, 0);
  requireValue(value.revision > lastRevision, 'stale proxy offer revision');
}

export function proxyOfferSigningBytes(value) {
  validateProxyOffer(value);
  return signingBytes(PROXY_OFFER_DOMAIN, value);
}

export async function proxyOfferDigest(value) {
  return b4a.toString(await blake3(proxyOfferSigningBytes(value)), 'hex');
}

// Cumulative subtotal for one logical request/session, never summed per stream chunk.
// Caller verifies usage evidence, locked acceptance and platform fees.
export function proxyOfferCost(value, usage) {
  validateProxyOffer(value);
  requireValue(usage !== null && typeof usage === 'object' && !Array.isArray(usage), 'invalid proxy usage');
  requireValue(Object.keys(usage).length <= value.rates.length, 'unpriced proxy usage unit');
  let total = money(value.per_request_au);
  for (const [unit, count] of Object.entries(usage)) {
    integer(count, 0);
    const rate = value.rates.find(r => r.unit === unit);
    requireValue(rate !== undefined, 'unpriced proxy usage unit');
    const numerator = money(rate.per_unit_au) * BigInt(count);
    requireValue(numerator <= U128_MAX, 'proxy cost overflow');
    const divisor = BigInt(rate.granularity);
    total += numerator / divisor + (numerator % divisor === 0n ? 0n : 1n);
    requireValue(total <= U128_MAX, 'proxy cost overflow');
  }
  const minimum = money(value.min_session_au);
  return (total < minimum ? minimum : total).toString();
}

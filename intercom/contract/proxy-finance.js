// Explicit proxy authorization/receipt wire. These pure validators neither mutate
// balances nor prove canonical admission, payout readiness or independent usage.
import b4a from 'b4a';
import { blake3 } from '@tracsystems/blake3';
import {
  ProxyValidationError, proxyCanonicalSigningBytes, validateProxyOffer,
  validateProxyOfferForMembership, proxyOfferCost,
} from './proxy-protocol.js';

export const PROXY_TERMS_DOMAIN = 'mayhem/proxy/spend-terms/v1';
export const PROXY_BUYER_TERMS_DOMAIN = 'mayhem/proxy/buyer-spend-authorization/v1';
export const PROXY_PROVIDER_TERMS_DOMAIN = 'mayhem/proxy/provider-spend-acceptance/v1';
export const PROXY_RECEIPT_DOMAIN = 'mayhem/proxy/usage-receipt/v1';
export const PROXY_BUYER_RECEIPT_DOMAIN = 'mayhem/proxy/buyer-usage-ack/v1';
export const PROXY_PROVIDER_RECEIPT_DOMAIN = 'mayhem/proxy/provider-usage-receipt/v1';
export const PROXY_SETTLEMENT_POLICY_DOMAIN = 'mayhem/proxy/settlement-policy/v1';
export const PROXY_CLOSURE_DOMAIN = 'mayhem/proxy/reservation-closure/v1';
export const PROXY_BUYER_CLOSURE_DOMAIN = 'mayhem/proxy/buyer-reservation-closure/v1';
export const PROXY_PROVIDER_CLOSURE_DOMAIN = 'mayhem/proxy/provider-reservation-closure/v1';
export const PROXY_EXPIRY_DOMAIN = 'mayhem/proxy/reservation-expiry/v1';
export const PROXY_BUYER_EXPIRY_DOMAIN = 'mayhem/proxy/buyer-reservation-expiry/v1';
const MAX = Number.MAX_SAFE_INTEGER;
const U128_MAX = (1n << 128n) - 1n;
const OUTCOMES = ['cancelled', 'complete', 'partial', 'refused', 'running'];
const TERMS_FIELDS = ['schema_version','lane','network_id','msb_bootstrap','subnet_bootstrap',
  'contract_version','buyer_pubkey','billing_id','billing_attempt','session_id','reservation_id',
  'billing_epoch','acceptance_expires_after_epoch','reservation_expires_after_epoch',
  'reservation_receipt_grace_epochs','payout_revision','request_hash','endpoint_contract',
  'recipe_hash','connection_digest','connection_revision','capacity_lease','offer','rail',
  'served_context','settlement_policy_hash','payment_terms_hash','rules_ver','max_usage',
  'max_spend_au','prior_spend_au','prior_reserved_au','max_total_spend_au'];
const RECEIPT_FIELDS = ['schema_version','lane','accepted_terms','seq','final','outcome',
  'result_hash','observation_hash','usage','au_owed_cum','billing_au_owed_cum','at_ms'];
function need(ok, message) { if (!ok) throw new ProxyValidationError(message); }
function shape(v, fields) {
  need(v && typeof v === 'object' && !Array.isArray(v), 'proxy financial record must be an object');
  const keys = Object.keys(v).sort();
  const expected = [...fields].sort();
  need(keys.length === expected.length && keys.every((k,i) => k === expected[i]), 'proxy financial record has missing or unsupported fields');
}
function integer(v, min = 0, max = MAX) { need(Number.isSafeInteger(v) && v >= min && v <= max, 'invalid proxy financial integer'); }
function hex(v) { need(typeof v === 'string' && /^[a-f0-9]{64}$/.test(v), 'invalid proxy financial digest'); }
function money(v) {
  need(typeof v === 'string' && /^(0|[1-9][0-9]{0,38})$/.test(v), 'invalid proxy financial amount');
  const n = BigInt(v); need(n <= U128_MAX, 'proxy financial amount overflow'); return n;
}
function add(a,b) { const n = a+b; need(n <= U128_MAX, 'proxy financial amount overflow'); return n; }
function base(v) { need(v.schema_version === 1 && v.lane === 'proxy', 'unsupported proxy financial schema/lane'); }
function signature(v) { need(typeof v === 'string' && /^[a-f0-9]{128}$/.test(v), 'invalid proxy signature encoding'); }
async function hash(domain, v) { return b4a.toString(await blake3(proxyCanonicalSigningBytes(domain,v)), 'hex'); }
function usage(v, offer = null) {
  need(v && typeof v === 'object' && !Array.isArray(v), 'proxy usage must be an object');
  const keys = Object.keys(v).sort();
  need(keys.length > 0 && keys.length <= 16 && keys.every(k => /^[a-z][a-z0-9_-]{0,63}$/.test(k)), 'invalid proxy usage units');
  keys.forEach(k => integer(v[k]));
  if (offer) need(keys.length === offer.rates.length && keys.every((k,i) => k === offer.rates[i].unit), 'proxy usage must include every priced unit exactly once');
}

export function validateProxySettlementPolicy(v) {
  shape(v,['schema_version','lane','payable_outcomes','allow_checkpoints',
    ...(v && Object.hasOwn(v,'hold_expiry') ? ['hold_expiry'] : [])]); base(v);
  if(Object.hasOwn(v,'hold_expiry'))need(v.hold_expiry==='release_unfinalized_and_block_retry', 'unsupported proxy hold expiry policy');
  need(typeof v.allow_checkpoints === 'boolean', 'invalid proxy checkpoint policy');
  const o = v.payable_outcomes;
  need(Array.isArray(o) && o.length > 0 && o.length <= 4 && o.includes('complete')
    && o.every((n,i) => OUTCOMES.includes(n) && n !== 'running' && (i === 0 || o[i-1] < n)), 'invalid payable proxy outcomes');
  proxyCanonicalSigningBytes(PROXY_SETTLEMENT_POLICY_DOMAIN,v);
}
export async function proxySettlementPolicyDigest(v) { validateProxySettlementPolicy(v); return hash(PROXY_SETTLEMENT_POLICY_DOMAIN,v); }

export function validateProxySpendTerms(v) {
  shape(v,TERMS_FIELDS); base(v);
  integer(v.contract_version,1,0xffffffff); integer(v.rules_ver,1,0xffffffff);
  need(typeof v.network_id === 'string' && /^[a-z0-9_-]{1,128}$/.test(v.network_id), 'invalid proxy spend network');
  for (const k of ['msb_bootstrap','subnet_bootstrap','buyer_pubkey','billing_id','session_id',
    'reservation_id','payout_revision','request_hash','endpoint_contract','recipe_hash',
    'connection_digest','capacity_lease','settlement_policy_hash','payment_terms_hash']) hex(v[k]);
  for (const k of ['billing_attempt','billing_epoch','connection_revision']) integer(v[k],1);
  integer(v.served_context,0,0xffffffff);
  integer(v.acceptance_expires_after_epoch,v.billing_epoch);
  integer(v.reservation_expires_after_epoch,v.acceptance_expires_after_epoch+1);
  integer(v.reservation_receipt_grace_epochs,0,MAX-v.reservation_expires_after_epoch);
  validateProxyOffer(v.offer);
  need(v.offer.accepted_rails.includes(v.rail), 'proxy offer rail mismatch');
  usage(v.max_usage,v.offer);
  need(Object.values(v.max_usage).some(n => n > 0), 'empty proxy authorized usage');
  const maximum = money(proxyOfferCost(v.offer,v.max_usage));
  need(maximum > 0n && maximum === money(v.max_spend_au), 'proxy reserve differs from priced usage bound');
  need(add(add(money(v.prior_spend_au),money(v.prior_reserved_au)),maximum) <= money(v.max_total_spend_au), 'proxy authorization budget exceeded');
  proxyCanonicalSigningBytes(PROXY_TERMS_DOMAIN,v);
}
export async function proxySpendTermsDigest(v) { validateProxySpendTerms(v); return hash(PROXY_TERMS_DOMAIN,v); }
export function proxyBuyerSpendSigningBytes(v) { validateProxySpendTerms(v); return proxyCanonicalSigningBytes(PROXY_BUYER_TERMS_DOMAIN,v); }
export function proxyProviderSpendSigningBytes(v) { validateProxySpendTerms(v); return proxyCanonicalSigningBytes(PROXY_PROVIDER_TERMS_DOMAIN,v); }
export async function validateProxyNewAcceptance(v,market,member,currentOffer,policy,epoch) {
  validateProxySpendTerms(v);
  await validateProxyOfferForMembership(v.offer,market,member);
  need(b4a.equals(proxyCanonicalSigningBytes('offer-comparison',v.offer),proxyCanonicalSigningBytes('offer-comparison',currentOffer)), 'proxy offer was superseded');
  need(epoch === v.billing_epoch && epoch <= v.acceptance_expires_after_epoch, 'proxy quote is not valid for active billing epoch');
  need(await proxySettlementPolicyDigest(policy) === v.settlement_policy_hash, 'proxy settlement policy mismatch');
  need(member.recipe_hash === v.recipe_hash && member.connection_revision === v.connection_revision
    && member.served_context === v.served_context
    && member.endpoints.some(e => e.endpoint === v.offer.endpoint && e.contract_hash === v.endpoint_contract), 'proxy spend membership mismatch');
}

export function validateProxyReceiptBody(v) {
  shape(v,RECEIPT_FIELDS); base(v);
  for (const k of ['accepted_terms','result_hash','observation_hash']) hex(v[k]);
  integer(v.seq,1); integer(v.at_ms);
  need(typeof v.final === 'boolean' && OUTCOMES.includes(v.outcome) && v.final !== (v.outcome === 'running'), 'invalid proxy receipt finality');
  usage(v.usage); money(v.au_owed_cum); money(v.billing_au_owed_cum);
  proxyCanonicalSigningBytes(PROXY_RECEIPT_DOMAIN,v);
}
export async function proxyReceiptDigest(v) { validateProxyReceiptBody(v); return hash(PROXY_RECEIPT_DOMAIN,v); }
export function proxyBuyerReceiptSigningBytes(v) { validateProxyReceiptBody(v); return proxyCanonicalSigningBytes(PROXY_BUYER_RECEIPT_DOMAIN,v); }
export function proxyProviderReceiptSigningBytes(v) { validateProxyReceiptBody(v); return proxyCanonicalSigningBytes(PROXY_PROVIDER_RECEIPT_DOMAIN,v); }
export async function validateProxyReceiptFor(v,terms,policy,previous = null) {
  validateProxyReceiptBody(v);
  need(v.accepted_terms === await proxySpendTermsDigest(terms), 'proxy receipt terms mismatch');
  need(await proxySettlementPolicyDigest(policy) === terms.settlement_policy_hash, 'proxy receipt policy mismatch');
  need(v.final ? policy.payable_outcomes.includes(v.outcome) : policy.allow_checkpoints, 'proxy outcome is not payable under accepted policy');
  usage(v.usage,terms.offer);
  need(Object.keys(v.usage).every(k => v.usage[k] <= terms.max_usage[k]), 'proxy usage exceeds authorization');
  need(v.au_owed_cum === proxyOfferCost(terms.offer,v.usage) && money(v.au_owed_cum) <= money(terms.max_spend_au), 'proxy receipt price mismatch');
  const cumulative = add(money(terms.prior_spend_au),money(v.au_owed_cum));
  need(cumulative === money(v.billing_au_owed_cum) && cumulative <= money(terms.max_total_spend_au), 'proxy cumulative authorization exceeded');
  if (previous) {
    await validateProxyReceiptFor(previous,terms,policy);
    if (b4a.equals(proxyCanonicalSigningBytes(PROXY_RECEIPT_DOMAIN,v),proxyCanonicalSigningBytes(PROXY_RECEIPT_DOMAIN,previous))) return;
    need(!previous.final && v.seq > previous.seq && v.at_ms >= previous.at_ms
      && money(v.au_owed_cum) >= money(previous.au_owed_cum)
      && Object.keys(v.usage).every(k => v.usage[k] >= previous.usage[k]), 'proxy receipt head cannot advance');
  }
}

export function verifyProxySpendAuthorization(v,verify) {
  shape(v,['terms','buyer_sig','provider_sig']); signature(v.buyer_sig); signature(v.provider_sig);
  need(verify(v.buyer_sig,proxyBuyerSpendSigningBytes(v.terms),v.terms.buyer_pubkey) === true
    && verify(v.provider_sig,proxyProviderSpendSigningBytes(v.terms),v.terms.offer.provider_pubkey) === true,
    'proxy spend signature rejected');
  proxyCanonicalSigningBytes('proxy-authorization-envelope-bound',v);
}
export async function verifyProxyUsageReceipt(v,terms,policy,previous,verify) {
  shape(v,['body','buyer_sig','provider_sig']); signature(v.buyer_sig); signature(v.provider_sig);
  await validateProxyReceiptFor(v.body,terms,policy,previous);
  need(verify(v.buyer_sig,proxyBuyerReceiptSigningBytes(v.body),terms.buyer_pubkey) === true
    && verify(v.provider_sig,proxyProviderReceiptSigningBytes(v.body),terms.offer.provider_pubkey) === true,
    'proxy receipt signature rejected');
  proxyCanonicalSigningBytes('proxy-receipt-envelope-bound',v);
}

// Both parties may waive an attempt's unfinalized charge once its remote outcome
// is known. This is not a paid usage receipt or evidence that arbitrary external
// execution really stopped; the trusted controller must establish that evidence.
export function validateProxyClosureBody(v) {
  shape(v,['schema_version','lane','accepted_terms','outcome','evidence_hash','at_ms']); base(v);
  hex(v.accepted_terms); hex(v.evidence_hash); integer(v.at_ms);
  need(['not_executed','cancelled','failed','completed_unbilled'].includes(v.outcome),'invalid proxy closure outcome');
  proxyCanonicalSigningBytes(PROXY_CLOSURE_DOMAIN,v);
}
export async function proxyClosureDigest(v) { validateProxyClosureBody(v); return hash(PROXY_CLOSURE_DOMAIN,v); }
export function proxyBuyerClosureSigningBytes(v) { validateProxyClosureBody(v); return proxyCanonicalSigningBytes(PROXY_BUYER_CLOSURE_DOMAIN,v); }
export function proxyProviderClosureSigningBytes(v) { validateProxyClosureBody(v); return proxyCanonicalSigningBytes(PROXY_PROVIDER_CLOSURE_DOMAIN,v); }
export async function verifyProxyClosure(v,terms,verify) {
  shape(v,['body','buyer_sig','provider_sig']); signature(v.buyer_sig); signature(v.provider_sig);
  validateProxyClosureBody(v.body);
  need(v.body.accepted_terms===await proxySpendTermsDigest(terms),'proxy closure terms mismatch');
  need(verify(v.buyer_sig,proxyBuyerClosureSigningBytes(v.body),terms.buyer_pubkey)===true
    &&verify(v.provider_sig,proxyProviderClosureSigningBytes(v.body),terms.offer.provider_pubkey)===true,'proxy closure signature rejected');
  proxyCanonicalSigningBytes('proxy-closure-envelope-bound',v);
}

// Expiry is buyer-authorized financial cleanup under the originally accepted
// policy. It explicitly does NOT establish backend termination or permit retry.
export function validateProxyExpiryBody(v) {
  shape(v,['schema_version','lane','accepted_terms','observed_epoch','at_ms']); base(v);
  hex(v.accepted_terms); integer(v.observed_epoch,1); integer(v.at_ms);
  proxyCanonicalSigningBytes(PROXY_EXPIRY_DOMAIN,v);
}
export async function proxyExpiryDigest(v) { validateProxyExpiryBody(v); return hash(PROXY_EXPIRY_DOMAIN,v); }
export function proxyBuyerExpirySigningBytes(v) { validateProxyExpiryBody(v); return proxyCanonicalSigningBytes(PROXY_BUYER_EXPIRY_DOMAIN,v); }
export async function verifyProxyExpiry(v,terms,policy,epoch,verify) {
  shape(v,['body','buyer_sig']); signature(v.buyer_sig); validateProxyExpiryBody(v.body);
  need(v.body.accepted_terms===await proxySpendTermsDigest(terms),'proxy expiry terms mismatch');
  need(await proxySettlementPolicyDigest(policy)===terms.settlement_policy_hash
    &&policy.hold_expiry==='release_unfinalized_and_block_retry','proxy expiry is not enabled by accepted policy');
  integer(epoch);
  need(v.body.observed_epoch<=epoch&&v.body.observed_epoch>terms.reservation_expires_after_epoch+terms.reservation_receipt_grace_epochs,
    'proxy reservation receipt grace has not expired');
  need(verify(v.buyer_sig,proxyBuyerExpirySigningBytes(v.body),terms.buyer_pubkey)===true,'proxy expiry signature rejected');
  proxyCanonicalSigningBytes('proxy-expiry-envelope-bound',v);
}

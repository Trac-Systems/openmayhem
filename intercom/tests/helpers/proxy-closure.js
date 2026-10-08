import b4a from 'b4a';
import { proxySpendTermsDigest, proxyBuyerClosureSigningBytes, proxyProviderClosureSigningBytes,
  proxyBuyerExpirySigningBytes } from '../../contract/proxy-finance.js';
const sign = (wallet, bytes) => b4a.toString(wallet.sign(bytes), 'hex');
export async function closure(f, outcome = 'not_executed', changes = {}) {
  const body = { schema_version: 1, lane: 'proxy', accepted_terms: await proxySpendTermsDigest(f.terms),
    outcome, evidence_hash: 'e'.repeat(64), at_ms: 4000, ...changes };
  return { op: 'proxy_close_reservation', provider: f.provider.publicKey, closure: { body,
    buyer_sig: sign(f.buyer.wallet, proxyBuyerClosureSigningBytes(body)),
    provider_sig: sign(f.provider.wallet, proxyProviderClosureSigningBytes(body)) } };
}
export async function expiry(f, changes = {}) {
  const body = { schema_version: 1, lane: 'proxy', accepted_terms: await proxySpendTermsDigest(f.terms),
    observed_epoch: f.terms.reservation_expires_after_epoch + f.terms.reservation_receipt_grace_epochs + 1,
    at_ms: 5000, ...changes };
  return { op: 'proxy_expire_reservation', buyer: f.buyer.publicKey, expiry: { body,
    buyer_sig: sign(f.buyer.wallet, proxyBuyerExpirySigningBytes(body)) } };
}
export function nextAttempt(t, changes = {}) {
  const id = n => n.toString(16).padStart(64, '0');
  return { ...structuredClone(t), billing_attempt: t.billing_attempt + 1,
    session_id: id(5000 + t.billing_attempt), reservation_id: id(6000 + t.billing_attempt),
    capacity_lease: id(7000 + t.billing_attempt), ...changes };
}

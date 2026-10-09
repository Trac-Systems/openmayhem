// Recovery for an immutable signed intention, including never-admitted terms.
// No current-offer lookup, balance disclosure, ledger write or history traversal.
import b4a from 'b4a';
import MayhemContract from '../../contract/contract.js';
import { validateProxySpendTerms, proxyBuyerSpendSigningBytes, proxySpendTermsDigest,
  verifyProxySpendAuthorization } from '../../contract/proxy-finance.js';
import { acceptedSpend, proxyReservationKeys } from '../../contract/proxy-reservations.js';

export const PROXY_INTENT_STATE_SERVICE = 'proxy_intent_state';
export const PROXY_INTENT_STATE_MAX_BYTES = 131072;
export const PROXY_INTENT_STATE_MAX_AGE_MS = 15000;
const hex = v => typeof v === 'string' && /^[a-f0-9]{64}$/.test(v);
const need = (ok, message) => { if (!ok) throw new Error(`Proxy intent state: ${message}.`); };
const keys = (v, expected) => v && typeof v === 'object' && !Array.isArray(v)
  && Object.keys(v).sort().join('|') === expected;

export function validateProxyIntentStateRequest(v) {
  need(keys(v, 'intent|request_nonce|requester') && hex(v.requester) && hex(v.request_nonce)
    && keys(v.intent, 'buyer_sig|terms') && /^[a-f0-9]{128}$/.test(v.intent.buyer_sig)
    && b4a.byteLength(JSON.stringify(v)) <= 32768, 'invalid query');
  validateProxySpendTerms(v.intent.terms);
  need([v.intent.terms.buyer_pubkey, v.intent.terms.offer.provider_pubkey].includes(v.requester),
    'requester is not a party');
}

export async function readProxyIntentState({ request, withCanonicalSnapshot, verifySignature }) {
  validateProxyIntentStateRequest(request);
  request = JSON.parse(JSON.stringify(request));
  need(typeof withCanonicalSnapshot === 'function' && typeof verifySignature === 'function',
    'service is not configured');
  const t = request.intent.terms;
  need(verifySignature(request.intent.buyer_sig, proxyBuyerSpendSigningBytes(t), t.buyer_pubkey) === true,
    'buyer signature rejected');
  const digest = await proxySpendTermsDigest(t);
  return withCanonicalSnapshot(async snapshot => {
    await snapshot.assertCurrent();
    for (const key of ['network_id', 'msb_bootstrap', 'subnet_bootstrap']) {
      need(t[key] === snapshot.context[key], 'intention network differs');
    }
    const ledger = Object.create(MayhemContract.prototype);
    ledger.get = key => snapshot.read(key);
    ledger.put = ledger.del = () => { throw new Error('Intent observation cannot write.'); };
    const existing = await ledger.get(proxyReservationKeys.accepted(digest));
    let authorization = null;
    if (existing !== null) {
      const accepted = await acceptedSpend(ledger, digest);
      verifyProxySpendAuthorization(accepted.authorization, verifySignature);
      need(accepted.authorization.buyer_sig === request.intent.buyer_sig, 'accepted buyer signature differs');
      authorization = accepted.authorization;
    } else {
      // Absence at one key cannot conceal a partial/corrupt admitted obligation.
      // Other attempts/providers may own these shared identifiers; do not expose
      // their data or treat their records as this intention's acceptance.
      const paths = [ledger.receiptBillingKey(t.billing_id), ledger.receiptReservationKey(t.reservation_id),
        ledger.receiptHeadKey(t.billing_id, t.billing_attempt),
        ledger.targetedSpendSessionKey(t.buyer_pubkey, t.rail, t.reservation_id),
        ledger.targetedSpendSessionIndexKey(t.buyer_pubkey, t.rail, t.session_id),
        ledger.targetedSpendBillingAttemptKey(t.buyer_pubkey, t.rail, t.billing_id, t.billing_attempt),
        proxyReservationKeys.resolution(digest), proxyReservationKeys.expiry(digest)];
      for (const path of paths) {
        const value = await ledger.get(path);
        need(value === null || (value.accepted_terms !== digest && value.latest_accepted_terms !== digest
          && value.reservation_id !== t.reservation_id && value.active_reservation_id !== t.reservation_id),
          'financial footprint exists without its accepted intention');
      }
    }
    // New acceptance requires billing_epoch === (pending ?? updated) + 1.
    // Use the COMPLETED epoch: a pending epoch is not final enough for release.
    // Existing acceptances remain admitted even after expiry/withdrawal/upgrades.
    const status = authorization ? 'admitted' : snapshot.context.epoch >= t.billing_epoch ? 'expired' : 'open';
    await snapshot.assertCurrent();
    const result = { ok: true, schema_version: 1, lane: 'proxy', ...request,
      accepted_terms: digest, status, authorization, context: snapshot.context, proof: snapshot.proof };
    need(b4a.byteLength(JSON.stringify(result)) <= PROXY_INTENT_STATE_MAX_BYTES, 'response exceeds bound');
    return result;
  }, { financial: true });
}

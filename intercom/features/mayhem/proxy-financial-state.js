// Exact-attempt financial recovery through the authenticated canonical service.
// No append, history traversal, dispatch permission or backend-capacity claim.
import b4a from 'b4a';
import MayhemContract from '../../contract/contract.js';
import { acceptedSpend, proxyReservationKeys, normalizeProxySpendSessionRecord,
  validateProxyCanonicalReceiptHead, validateProxyBillingAnchor } from '../../contract/proxy-reservations.js';
import { verifyProxySpendAuthorization, verifyProxyUsageReceipt, verifyProxyClosure,
  verifyProxyExpiry } from '../../contract/proxy-finance.js';

export const PROXY_FINANCIAL_STATE_SERVICE = 'proxy_financial_state';
export const PROXY_FINANCIAL_STATE_MAX_BYTES = 131072;
export const PROXY_FINANCIAL_STATE_MAX_AGE_MS = 15000;
const hex = value => typeof value === 'string' && /^[a-f0-9]{64}$/.test(value);
const need = (ok, message) => { if (!ok) throw new Error(`Proxy financial state: ${message}.`); };

export function validateProxyFinancialStateRequest(value) {
  need(value && typeof value === 'object' && !Array.isArray(value)
    && Object.keys(value).sort().join('|') === 'accepted_terms|request_nonce|requester'
    && hex(value.accepted_terms) && hex(value.request_nonce) && hex(value.requester), 'invalid request');
}

export async function readProxyFinancialState({ request, withCanonicalSnapshot, verifySignature }) {
  validateProxyFinancialStateRequest(request);
  request = { ...request };
  need(typeof withCanonicalSnapshot === 'function' && typeof verifySignature === 'function', 'service is not configured');
  return withCanonicalSnapshot(async snapshot => {
    await snapshot.assertCurrent();
    const ledger = Object.create(MayhemContract.prototype);
    ledger.get = key => snapshot.read(key);
    ledger.put = ledger.del = () => { throw new Error('Financial recovery cannot write.'); };
    const accepted = await acceptedSpend(ledger, request.accepted_terms);
    const terms = accepted.authorization.terms;
    need([terms.buyer_pubkey, terms.offer.provider_pubkey].includes(request.requester), 'requester is not a party');
    for (const key of ['network_id', 'msb_bootstrap', 'subnet_bootstrap']) {
      need(terms[key] === snapshot.context[key], 'accepted network differs');
    }
    verifyProxySpendAuthorization(accepted.authorization, verifySignature);
    const paths = {
      session: ledger.targetedSpendSessionKey(terms.buyer_pubkey, terms.rail, terms.reservation_id),
      reservation: ledger.receiptReservationKey(terms.reservation_id),
      billing: ledger.receiptBillingKey(terms.billing_id),
      receipt_head: ledger.receiptHeadKey(terms.billing_id, terms.billing_attempt),
      resolution: proxyReservationKeys.resolution(request.accepted_terms),
      expiry: proxyReservationKeys.expiry(request.accepted_terms),
    };
    const state = {};
    for (const [name, key] of Object.entries(paths)) state[name] = await ledger.get(key);
    if (state.session !== null) {
      await normalizeProxySpendSessionRecord(state.session, terms.buyer_pubkey, terms.rail, terms.reservation_id);
      need(state.session.accepted_terms === request.accepted_terms, 'session differs');
    }
    need(state.reservation?.lane === 'proxy' && state.reservation.accepted_terms === request.accepted_terms,
      'reservation is missing or differs');
    if (state.receipt_head !== null) {
      await validateProxyCanonicalReceiptHead(ledger, state.receipt_head);
      need(state.receipt_head.accepted_terms === request.accepted_terms, 'receipt differs');
      await verifyProxyUsageReceipt(state.receipt_head.receipt, terms, accepted.settlement_policy, null, verifySignature);
    }
    if (state.resolution !== null) {
      need(state.resolution.accepted_terms === request.accepted_terms, 'resolution differs');
      await verifyProxyClosure(state.resolution.closure, terms, verifySignature);
    }
    if (state.expiry !== null) {
      need(state.expiry.accepted_terms === request.accepted_terms, 'expiry differs');
      await verifyProxyExpiry(state.expiry.expiry, terms, accepted.settlement_policy, snapshot.context.epoch, verifySignature);
    }
    // Recovery may read an old attempt after a later attempt became active. Do
    // not confuse that later anchor with permission to dispatch the old request.
    if (state.billing?.latest_accepted_terms === request.accepted_terms) {
      await validateProxyBillingAnchor(state.billing, terms, state.session !== null && !state.session.settlement_ready);
    } else {
      need(state.receipt_head?.settlement_ready === true || state.resolution !== null, 'unresolved attempt lost its billing anchor');
    }
    await snapshot.assertCurrent();
    const result = { ok: true, schema_version: 1, lane: 'proxy', ...request,
      context: snapshot.context, proof: snapshot.proof, accepted, ...state };
    need(b4a.byteLength(JSON.stringify(result)) <= PROXY_FINANCIAL_STATE_MAX_BYTES, 'response exceeds bound');
    return result;
  }, { financial: true });
}

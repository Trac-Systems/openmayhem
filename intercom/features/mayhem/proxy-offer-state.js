// Provider-owned canonical inputs to countersigning. No buyer account lookup,
// capacity reservation or financial mutation. Publication repeats writer checks.
import b4a from 'b4a';
import MayhemContract from '../../contract/contract.js';
import { validateProxyOffer, proxyOfferDigest } from '../../contract/proxy-protocol.js';
import { readActiveProxyOffer } from '../../contract/proxy-registry.js';
import { prepareProxyFinancialAdmission } from '../../contract/proxy-finance-policy.js';
import { proxySettlementPolicyDigest } from '../../contract/proxy-finance.js';
import { proxyReservationKeys, proxyPaymentTermsDigest } from '../../contract/proxy-reservations.js';

export const PROXY_OFFER_STATE_SERVICE = 'proxy_offer_state';
export const PROXY_OFFER_STATE_MAX_BYTES = 131072;
export const PROXY_OFFER_STATE_MAX_AGE_MS = 15000;
const hex = v => typeof v === 'string' && /^[a-f0-9]{64}$/.test(v);
const need = (ok, message) => { if (!ok) throw new Error(`Proxy offer state: ${message}.`); };
const checked = v => { if (v instanceof Error) throw v; return v; };

// Shared with buyer quotes: caller performs its role-specific exact-key checks.
export function validateProxyOfferQuery(value) {
  need(value && typeof value === 'object' && !Array.isArray(value)
    && [value.request_nonce, value.requester, value.settlement_policy_hash].every(hex)
    && ['fiat', 'tnk', 'tap'].includes(value.rail)
    && b4a.byteLength(JSON.stringify(value)) <= 32768, 'invalid request');
  validateProxyOffer(value.offer);
  need(value.offer.accepted_rails.includes(value.rail), 'offer does not accept this rail');
}
export function validateProxyOfferStateRequest(value) {
  validateProxyOfferQuery(value);
  need(Object.keys(value).sort().join('|') === 'offer|rail|request_nonce|requester|settlement_policy_hash'
    && value.requester === value.offer.provider_pubkey, 'requester does not own this offer');
}

// Only called inside a fresh canonical snapshot. The payout target is internal;
// neither observation response includes it. Reads are exact keys, never history.
export async function readProxyOfferInputs(request, snapshot) {
  const ledger = Object.create(MayhemContract.prototype);
  ledger.get = key => snapshot.read(key);
  ledger.put = ledger.del = () => { throw new Error('Offer observation cannot write.'); };
  const applied = checked(await ledger.epochApplyStateRecord());
  const billingEpoch = (applied.pending_epoch ?? applied.updated_epoch) + 1;
  need(Number.isSafeInteger(billingEpoch) && billingEpoch > 0, 'invalid billing epoch');
  const selected = await readActiveProxyOffer(request.offer, snapshot.context, key => ledger.get(key));
  need(selected !== null && selected.offer_digest === await proxyOfferDigest(request.offer),
    'offer is no longer active or admitted; requote');
  await prepareProxyFinancialAdmission(key => ledger.get(key), request.offer.provider_pubkey, billingEpoch);
  const policy = await ledger.get(proxyReservationKeys.settlementPolicy(request.settlement_policy_hash));
  need(policy?.enabled === true && await proxySettlementPolicyDigest(policy.policy) === request.settlement_policy_hash,
    'settlement policy is not enabled');
  const provider = request.offer.provider_pubkey;
  const registration = await ledger.get(`prov/${provider}`);
  need(registration?.status === 'active' && registration.accepted_rails?.includes(request.rail),
    'provider payment rail is not active');
  const pointer = await ledger.get(ledger.providerPayoutBindingPointerKey(provider, request.rail));
  need(pointer?.provider === provider && pointer.rail === request.rail, 'payout pointer is missing');
  const revision = pointer.pending_revision !== null && pointer.pending_activation_epoch <= billingEpoch
    ? pointer.pending_revision : pointer.current_revision;
  const payout = checked(await ledger.providerPayoutBindingForEpoch(provider, request.rail, revision,
    billingEpoch, { requireCurrentReadiness: true }));
  const rules = await ledger.currentRules();
  return { ledger, payout, fields: {
    context: snapshot.context, proof: snapshot.proof, billing_epoch: billingEpoch,
    market: selected.market, membership: selected.membership, settlement_policy: policy.policy,
    payout_revision: payout.revision, payment_terms_hash: await proxyPaymentTermsDigest(rules, payout),
    rules_ver: rules.ver,
  } };
}

export async function readProxyOfferState({ request, withCanonicalSnapshot }) {
  validateProxyOfferStateRequest(request);
  request = JSON.parse(JSON.stringify(request));
  need(typeof withCanonicalSnapshot === 'function', 'service is not configured');
  return withCanonicalSnapshot(async snapshot => {
    await snapshot.assertCurrent();
    const { fields } = await readProxyOfferInputs(request, snapshot);
    await snapshot.assertCurrent();
    const response = { ok: true, schema_version: 1, lane: 'proxy', ...request, ...fields };
    need(b4a.byteLength(JSON.stringify(response)) <= PROXY_OFFER_STATE_MAX_BYTES, 'response exceeds bound');
    return response;
  }, { financial: true });
}

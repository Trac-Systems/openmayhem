// Fresh buyer-bound inputs to negotiation, never a spend authorization or a
// reservation. Final publication repeats admission on the canonical writer.
import b4a from 'b4a';
import MayhemContract from '../../contract/contract.js';
import { validateProxyOffer, proxyOfferDigest } from '../../contract/proxy-protocol.js';
import { readActiveProxyOffer } from '../../contract/proxy-registry.js';
import { prepareProxyFinancialAdmission } from '../../contract/proxy-finance-policy.js';
import { proxySettlementPolicyDigest, verifyProxySpendAuthorization } from '../../contract/proxy-finance.js';
import { acceptedSpend, proxyReservationKeys, proxyPaymentTermsDigest,
  validateProxyBillingAnchor } from '../../contract/proxy-reservations.js';

export const PROXY_QUOTE_STATE_SERVICE = 'proxy_quote_state';
export const PROXY_QUOTE_STATE_MAX_BYTES = 131072;
export const PROXY_QUOTE_STATE_MAX_AGE_MS = 15000;
const hex = v => typeof v === 'string' && /^[a-f0-9]{64}$/.test(v);
const need = (ok, message) => { if (!ok) throw new Error(`Proxy quote state: ${message}.`); };
const checked = v => { if (v instanceof Error) throw v; return v; };

export function validateProxyQuoteStateRequest(value) {
  need(value && typeof value === 'object' && !Array.isArray(value)
    && Object.keys(value).sort().join('|') === 'billing_id|offer|rail|request_nonce|requester|settlement_policy_hash'
    && [value.billing_id, value.request_nonce, value.requester, value.settlement_policy_hash].every(hex)
    && ['fiat', 'tnk', 'tap'].includes(value.rail)
    && b4a.byteLength(JSON.stringify(value)) <= 32768, 'invalid request');
  validateProxyOffer(value.offer);
  need(value.offer.accepted_rails.includes(value.rail), 'offer does not accept this rail');
}

export async function readProxyQuoteState({ request, withCanonicalSnapshot, verifySignature }) {
  validateProxyQuoteStateRequest(request);
  request = JSON.parse(JSON.stringify(request));
  need(typeof withCanonicalSnapshot === 'function' && typeof verifySignature === 'function', 'service is not configured');
  return withCanonicalSnapshot(async snapshot => {
    await snapshot.assertCurrent();
    const ledger = Object.create(MayhemContract.prototype);
    ledger.get = key => snapshot.read(key);
    ledger.put = ledger.del = () => { throw new Error('Quote observation cannot write.'); };
    const applied = checked(await ledger.epochApplyStateRecord());
    const billingEpoch = (applied.pending_epoch ?? applied.updated_epoch) + 1;
    need(Number.isSafeInteger(billingEpoch) && billingEpoch > 0, 'invalid billing epoch');
    const selected = await readActiveProxyOffer(request.offer, snapshot.context, key => ledger.get(key));
    need(selected !== null && selected.offer_digest === await proxyOfferDigest(request.offer), 'offer is no longer active or admitted; requote');
    // Check quotas without applying the returned counter writes.
    await prepareProxyFinancialAdmission(key => ledger.get(key), request.offer.provider_pubkey, billingEpoch);
    const policy = await ledger.get(proxyReservationKeys.settlementPolicy(request.settlement_policy_hash));
    need(policy?.enabled === true && await proxySettlementPolicyDigest(policy.policy) === request.settlement_policy_hash,
      'settlement policy is not enabled');
    const provider = request.offer.provider_pubkey;
    const registration = await ledger.get(`prov/${provider}`);
    need(registration?.status === 'active' && registration.accepted_rails?.includes(request.rail), 'provider payment rail is not active');
    const pointer = await ledger.get(ledger.providerPayoutBindingPointerKey(provider, request.rail));
    need(pointer?.provider === provider && pointer.rail === request.rail, 'payout pointer is missing');
    const revision = pointer.pending_revision !== null && pointer.pending_activation_epoch <= billingEpoch
      ? pointer.pending_revision : pointer.current_revision;
    const payout = checked(await ledger.providerPayoutBindingForEpoch(provider, request.rail, revision,
      billingEpoch, { requireCurrentReadiness: true }));
    const rules = await ledger.currentRules();
    const paymentTerms = await proxyPaymentTermsDigest(rules, payout);
    const balance = checked(await ledger.balanceRecord(request.requester, request.rail));
    checked(ledger.guardianValidateBalanceRecord(balance, request.requester, request.rail));
    if (request.rail === 'tap') need(balance.chain_id === payout.chain_id, 'TAP funding and payout chains differ');
    const accounting = checked(await ledger.targetedSpendAccountingState(request.requester, request.rail));
    const reserved = checked(ledger.safeAddAu(accounting.summary.reserved_au, accounting.legacy_reserved_au));
    need(ledger.compareAu(reserved, balance.au) <= 0, 'funding state is inconsistent');
    const billing = await ledger.get(ledger.receiptBillingKey(request.billing_id));
    if (billing !== null) {
      // The transport-bound requester may inspect only its own logical purchase.
      need(billing.user === request.requester && billing.rail === request.rail, 'billing identity differs');
      const old = await acceptedSpend(ledger, billing.latest_accepted_terms);
      verifyProxySpendAuthorization(old.authorization, verifySignature);
      await validateProxyBillingAnchor(billing, old.authorization.terms, billing.active_reservation_id !== null);
    }
    await snapshot.assertCurrent();
    const response = { ok: true, schema_version: 1, lane: 'proxy', ...request,
      context: snapshot.context, proof: snapshot.proof, billing_epoch: billingEpoch,
      market: selected.market, membership: selected.membership, settlement_policy: policy.policy,
      payout_revision: payout.revision, payment_terms_hash: paymentTerms, rules_ver: rules.ver,
      funding: { balance_au: balance.au, reserved_au: reserved,
        available_au: checked(ledger.safeSubAu(balance.au, reserved)),
        chain_id: request.rail === 'tap' ? balance.chain_id : null },
      billing: billing === null ? null : Object.fromEntries([
        'latest_attempt', 'latest_accepted_terms', 'active_reservation_id', 'retry_blocked',
        'request_hash', 'endpoint_contract', 'max_total_spend_au', 'spent_au', 'reserved_au',
      ].map(key => [key, billing[key]])) };
    need(b4a.byteLength(JSON.stringify(response)) <= PROXY_QUOTE_STATE_MAX_BYTES, 'response exceeds bound');
    return response;
  }, { financial: true });
}

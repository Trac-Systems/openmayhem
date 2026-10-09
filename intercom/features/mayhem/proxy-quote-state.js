// Fresh buyer-bound inputs to negotiation, never a spend authorization or a
// reservation. Final publication repeats admission on the canonical writer.
import b4a from 'b4a';
import { validateProxyOfferQuery, readProxyOfferInputs } from './proxy-offer-state.js';
import { verifyProxySpendAuthorization } from '../../contract/proxy-finance.js';
import { acceptedSpend, validateProxyBillingAnchor } from '../../contract/proxy-reservations.js';

export const PROXY_QUOTE_STATE_SERVICE = 'proxy_quote_state';
export const PROXY_QUOTE_STATE_MAX_BYTES = 131072;
export const PROXY_QUOTE_STATE_MAX_AGE_MS = 15000;
const hex = v => typeof v === 'string' && /^[a-f0-9]{64}$/.test(v);
const need = (ok, message) => { if (!ok) throw new Error(`Proxy quote state: ${message}.`); };
const checked = v => { if (v instanceof Error) throw v; return v; };

export function validateProxyQuoteStateRequest(value) {
  validateProxyOfferQuery(value);
  need(Object.keys(value).sort().join('|') === 'billing_id|offer|rail|request_nonce|requester|settlement_policy_hash'
    && hex(value.billing_id), 'invalid request');
}

export async function readProxyQuoteState({ request, withCanonicalSnapshot, verifySignature }) {
  validateProxyQuoteStateRequest(request);
  request = JSON.parse(JSON.stringify(request));
  need(typeof withCanonicalSnapshot === 'function' && typeof verifySignature === 'function', 'service is not configured');
  return withCanonicalSnapshot(async snapshot => {
    await snapshot.assertCurrent();
    const { ledger, payout, fields } = await readProxyOfferInputs(request, snapshot);
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
      ...fields,
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

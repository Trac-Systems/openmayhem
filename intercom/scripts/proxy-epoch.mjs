// Explicit proxy input for the shared epoch finalizer. These are frozen canonical
// heads plus exact accepted-spend records, never vendor usage or a native enclave.
import b4a from 'b4a';
import PeerWallet from 'trac-wallet';
import { verifyProxySpendAuthorization, verifyProxyUsageReceipt } from '../contract/proxy-finance.js';
import { validateProxyCanonicalReceiptHead, proxyReservationKeys } from '../contract/proxy-reservations.js';
const need = (ok, message) => { if (!ok) throw new Error(message); };
const verify = (signature, bytes, key) => PeerWallet.verify(b4a.from(signature,'hex'),bytes,b4a.from(key,'hex'));

export function proxyEpochAcceptances(bundle, receipts) {
  const expected = new Set(receipts.filter(head => head?.lane === 'proxy').map(head => head.accepted_terms));
  const value = bundle.proxy_acceptances ?? {};
  need(value && typeof value === 'object' && !Array.isArray(value), 'proxy_acceptances must be an exact-key object');
  const keys = Object.keys(value);
  need(keys.length === expected.size && keys.every(key => /^[a-f0-9]{64}$/.test(key) && expected.has(key)),
    'proxy accepted-spend records must match the frozen receipt heads exactly');
  return new Map(keys.map(key => [proxyReservationKeys.accepted(key),value[key]]));
}

export async function proxyEpochReceipt(head, records, epoch) {
  need(head?.lane === 'proxy' && head.receipt?.body?.lane === 'proxy', 'proxy epoch lane mismatch');
  const accepted = await validateProxyCanonicalReceiptHead({ get: async key => records.get(key) ?? null },head);
  need(head.settlement_ready === true && head.settlement_epoch === epoch && head.epoch === epoch
    && BigInt(head.incremental_au) > 0n, 'proxy epoch requires a payable canonical final for this settlement epoch');
  const terms = accepted.authorization.terms;
  verifyProxySpendAuthorization(accepted.authorization,verify);
  await verifyProxyUsageReceipt(head.receipt,terms,accepted.settlement_policy,null,verify);
  return { entry: head, head, envelope: head.receipt, proxy: true,
    // Accounting identity is derived from signed accepted terms. Never inject it
    // into the signed receipt body/leaf or synthesize native pricing dimensions.
    body: { lane:'proxy', billing_id:terms.billing_id,billing_attempt:terms.billing_attempt,
      billing_epoch:terms.billing_epoch,reservation_id:terms.reservation_id,
      session_id:terms.session_id,user:terms.buyer_pubkey,provider:terms.offer.provider_pubkey,
      payout_revision:terms.payout_revision,rail:terms.rail,seq:head.receipt.body.seq },
  };
}

import crypto from 'node:crypto';
import { stableJson } from '../../scripts/recompute-epoch-roots.mjs';
import { proxyReservationKeys } from '../../contract/proxy-reservations.js';

export async function proxyEpochBundle(ledger, heads, epoch, { maxApplyBatch = 2, ...params } = {}) {
  const index = await ledger.get(ledger.receiptEpochIndexKey(epoch));
  const snapshot = { schema_version:1,type:'canonical_epoch_receipt_snapshot',settlement_epoch:epoch,
    metadata:index,identities:heads.map(h=>({billing_id:h.billing_id,billing_attempt:h.billing_attempt})),heads };
  snapshot.snapshot_sha256=crypto.createHash('sha256').update(stableJson(snapshot)).digest('hex');
  const proxy_acceptances={};
  for(const head of heads)if(head.lane==='proxy')proxy_acceptances[head.accepted_terms]=await ledger.get(proxyReservationKeys.accepted(head.accepted_terms));
  return {epoch,params:{fee_bps:1500,epoch_seconds:3600,max_apply_batch:maxApplyBatch,max_market_usage_entries:20,...params},
    receipts:heads,receipt_snapshot:snapshot,proxy_acceptances,
    deposits:[],payouts:[],price_derivations:[],prior_earnings:{},prior_fee_cum_au:'0',prior_burn_cum_au:'0'};
}

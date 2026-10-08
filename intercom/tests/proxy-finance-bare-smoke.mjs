// Run from the checkout root with the release-supported Bare runtime.
import fs from 'fs';
import b4a from 'b4a';
import PeerWallet from 'trac-wallet';
import * as f from '../contract/proxy-finance.js';
import MayhemContract from '../contract/contract.js';
import {prepareProxyUsageReceipt,normalizeProxySpendSessionRecord,proxyReservationKeys} from '../contract/proxy-reservations.js';
const check=(v,m)=>{if(!v)throw new Error(m);};
const cases=JSON.parse(fs.readFileSync('crates/mayhem-proto/tests/fixtures/proxy-finance-v1.json','utf8')).cases;
for(const r of cases) {
  check(await f.proxySpendTermsDigest(r.terms)===r.digests.terms,'terms digest changed');
  check(await f.proxyReceiptDigest(r.receipt)===r.digests.receipt,'receipt digest changed');
  await f.validateProxyNewAcceptance(r.terms,r.market,r.membership,r.terms.offer,r.policy,51);
  await f.validateProxyReceiptFor(r.receipt,r.terms,r.policy,r.checkpoint);
  await f.validateProxyReceiptFor(r.receipt,r.terms,r.policy,r.receipt);
}
const buyer=new PeerWallet(); const provider=new PeerWallet();
await buyer.ready; await provider.ready; await buyer.generateKeyPair(); await provider.generateKeyPair();
const r=JSON.parse(JSON.stringify(cases[0]));
r.terms.buyer_pubkey=b4a.toString(buyer.publicKey,'hex');
r.terms.offer.provider_pubkey=b4a.toString(provider.publicKey,'hex');
r.receipt.accepted_terms=await f.proxySpendTermsDigest(r.terms);
const verify=(s,m,k)=>buyer.verify(b4a.from(s,'hex'),m,b4a.from(k,'hex'));
const sign=(w,m)=>b4a.toString(w.sign(m),'hex');
const auth={terms:r.terms,buyer_sig:sign(buyer,f.proxyBuyerSpendSigningBytes(r.terms)),provider_sig:sign(provider,f.proxyProviderSpendSigningBytes(r.terms))};
f.verifyProxySpendAuthorization(auth,verify);
const receipt={body:r.receipt,buyer_sig:sign(buyer,f.proxyBuyerReceiptSigningBytes(r.receipt)),provider_sig:sign(provider,f.proxyProviderReceiptSigningBytes(r.receipt))};
await f.verifyProxyUsageReceipt(receipt,r.terms,r.policy,null,verify);
let rejected=false;
try { await f.verifyProxyUsageReceipt({...receipt,buyer_sig:auth.buyer_sig},r.terms,r.policy,null,verify); } catch { rejected=true; }
check(rejected,'cross-domain signature accepted');

// Exercise production Bare's actual shared hold/index helpers, with synthetic
// canonical accepted records. This is a runtime check, not a live payment proof.
const t=r.terms;
t.prior_spend_au='0'; t.prior_reserved_au='0';
const digest=await f.proxySpendTermsDigest(t);
const authorization={terms:t,buyer_sig:sign(buyer,f.proxyBuyerSpendSigningBytes(t)),
  provider_sig:sign(provider,f.proxyProviderSpendSigningBytes(t))};
const identity={...Object.fromEntries(['billing_id','billing_attempt','billing_epoch','reservation_id',
  'reservation_expires_after_epoch','reservation_receipt_grace_epochs','session_id','rail','payout_revision'].map(k=>[k,t[k]])),
  user:t.buyer_pubkey,provider:t.offer.provider_pubkey};
const session={type:'targeted_spend_session',lane:'proxy',accepted_terms:digest,authorization,settlement_policy:r.policy,
  ...identity,max_spend_au:t.max_spend_au,settlement_ready:false,closed_at:null,feature_key:'test/reserve',reserved_at:1000,
  recorded_at:'test/reserve',updated_at:'test/reserve'};
await normalizeProxySpendSessionRecord(session,t.buyer_pubkey,t.rail,t.reservation_id);
const ledger=new MayhemContract({peer:{}},{}), records=new Map();
ledger.get=async key=>records.get(key)??null;
ledger.put=ledger.del=()=>{throw new Error('planner wrote during validation');};
records.set(proxyReservationKeys.accepted(digest),{type:'proxy_accepted_spend',accepted_terms:digest,authorization,settlement_policy:r.policy,max_checkpoints:8});
records.set('epoch/apply/state',{updated_epoch:t.billing_epoch-1,pending_epoch:null});
const sessionKey=ledger.targetedSpendSessionKey(t.buyer_pubkey,t.rail,t.reservation_id);
records.set(sessionKey,session);
records.set(ledger.targetedSpendSummaryKey(t.buyer_pubkey,t.rail),{type:'targeted_spend_summary',user:t.buyer_pubkey,rail:t.rail,
  denom:'au_usd',reserved_au:t.max_spend_au,balance_au_at_last_reserve:t.max_spend_au,updated_at:'test/reserve'});
records.set(ledger.receiptBillingKey(t.billing_id),{type:'proxy_billing_anchor',lane:'proxy',billing_id:t.billing_id,user:t.buyer_pubkey,
  latest_attempt:t.billing_attempt,active_reservation_id:t.reservation_id,max_total_spend_au:t.max_total_spend_au,
  rail:t.rail,request_hash:t.request_hash,endpoint_contract:t.endpoint_contract,latest_accepted_terms:digest,retry_blocked:false,
  created_at:'test/reserve',updated_at:'test/reserve',spent_au:'0',reserved_au:t.max_spend_au});
records.set(ledger.receiptReservationKey(t.reservation_id),{type:'receipt_reservation_identity',lane:'proxy',accepted_terms:digest,
  ...identity,status:'active',closed_at:null,close_record_key:null});
const body={...r.receipt,accepted_terms:digest,billing_au_owed_cum:r.receipt.au_owed_cum};
const envelope={op:'proxy_record_usage',provider:t.offer.provider_pubkey,receipt:{body,buyer_sig:sign(buyer,f.proxyBuyerReceiptSigningBytes(body)),
  provider_sig:sign(provider,f.proxyProviderReceiptSigningBytes(body))}};
const plan=await prepareProxyUsageReceipt(ledger,envelope,t,verify);
check(plan.result.au===body.au_owed_cum&&!plan.duplicate,'receipt plan failed');
for(const w of plan.writes)if(w.delete)records.delete(w.key);else records.set(w.key,w.value);
check(records.get(sessionKey).max_spend_au===body.au_owed_cum,'final hold differs from verified charge');
check((await prepareProxyUsageReceipt(ledger,envelope,t,verify)).duplicate,'receipt replay failed');
console.log('Bare proxy finance: 12 wire vectors, real signatures, shared hold/index finalization and exact replay passed.');

const closureCases=JSON.parse(fs.readFileSync('crates/mayhem-proto/tests/fixtures/proxy-closure-v1.json','utf8')).cases;
for(const row of closureCases) {
  check(await f.proxySettlementPolicyDigest(row.policy)===row.digests.policy,'expiry policy digest changed');
  check(await f.proxyClosureDigest(row.closure)===row.digests.closure,'closure digest changed');
  check(await f.proxyExpiryDigest(row.expiry)===row.digests.expiry,'expiry digest changed');
  check(f.proxyBuyerClosureSigningBytes(row.closure).toString('utf8')===row.signing_utf8.buyer_closure,'closure signing changed');
  check(f.proxyProviderClosureSigningBytes(row.closure).toString('utf8')===row.signing_utf8.provider_closure,'provider closure signing changed');
  check(f.proxyBuyerExpirySigningBytes(row.expiry).toString('utf8')===row.signing_utf8.buyer_expiry,'expiry signing changed');
}
const cb={schema_version:1,lane:'proxy',accepted_terms:digest,outcome:'cancelled',evidence_hash:'e'.repeat(64),at_ms:4000};
await f.verifyProxyClosure({body:cb,buyer_sig:sign(buyer,f.proxyBuyerClosureSigningBytes(cb)),
  provider_sig:sign(provider,f.proxyProviderClosureSigningBytes(cb))},t,verify);
console.log('Bare proxy closure: 12 endpoint/rail vectors and actual role-separated closure signatures passed.');

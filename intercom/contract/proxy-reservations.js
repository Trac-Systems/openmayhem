// Deterministic reservation preparation against the same ledger view used by
// native accounting. Returns a write plan; no writes/network/clock/history scan.
// The admitted feature transport and application must BOTH run this before
// activation. A prepared plan is not an authorization token and cannot be sent
// by a client as arbitrary ledger writes.
import b4a from 'b4a';
import { blake3 } from '@tracsystems/blake3';
import { ProxyValidationError, proxyCanonicalSigningBytes } from './proxy-protocol.js';
import { readActiveProxyOffer } from './proxy-registry.js';
import { proxySpendTermsDigest, proxySettlementPolicyDigest, validateProxySpendTerms,
  validateProxyNewAcceptance, verifyProxySpendAuthorization, validateProxyReceiptBody,
  validateProxyReceiptFor, proxyReceiptDigest, verifyProxyUsageReceipt } from './proxy-finance.js';

const copy=v=>JSON.parse(JSON.stringify(v));
const need=(v,m)=>{if(!v)throw new ProxyValidationError(m);};
const checked=v=>{if(v instanceof Error)throw new ProxyValidationError(v.message);return v;};
const same=(a,b)=>b4a.equals(proxyCanonicalSigningBytes('comparison',a),proxyCanonicalSigningBytes('comparison',b));
const hash=async(domain,v)=>b4a.toString(await blake3(proxyCanonicalSigningBytes(domain,v)),'hex');
const shape=(v,keys)=>need(v && typeof v==='object' && !Array.isArray(v)
  && Object.keys(v).sort().join('|')===[...keys].sort().join('|'),'invalid proxy reservation fields');
const integer=(v)=>need(Number.isSafeInteger(v)&&v>=0,'invalid proxy reservation counter/time');
const ref=v=>need(typeof v==='string'&&v.length>0&&v.length<=256,'invalid proxy reservation reference');
const money=v=>{need(typeof v==='string'&&/^(0|[1-9][0-9]{0,38})$/.test(v)&&BigInt(v)<(1n<<128n),'invalid proxy reservation amount');return BigInt(v);};
const IDENTITY_FIELDS=['billing_id','billing_attempt','billing_epoch','reservation_id',
  'reservation_expires_after_epoch','reservation_receipt_grace_epochs','session_id',
  'user','rail','provider','payout_revision'];

export const proxyReservationKeys={
  accepted:digest=>`proxy/v1/accepted/${digest}`,
  settlementPolicy:digest=>`proxy/v1/settlement-policy/${digest}`,
};

export function validateProxyReservationEnvelope(v) {
  shape(v,['op','authorization','at']);
  need(v.op==='proxy_spend_reserve','invalid proxy reservation operation'); integer(v.at);
  shape(v.authorization,['terms','buyer_sig','provider_sig']);
  validateProxySpendTerms(v.authorization.terms);
  for(const s of [v.authorization.buyer_sig,v.authorization.provider_sig]) need(typeof s==='string'&&/^[0-9a-f]{128}$/.test(s),'invalid proxy reservation signature');
  proxyCanonicalSigningBytes('proxy-reservation-envelope-bound',v);
}
export async function proxyReservationFeatureKey(v) {
  validateProxyReservationEnvelope(v);
  return `proxy/spend/${await proxySpendTermsDigest(v.authorization.terms)}`;
}

// Existing rules and exact verified payout identity, not a new fee or upstream invoice.
export async function proxyPaymentTermsDigest(rules,binding) {
  need(rules && Number.isSafeInteger(rules.ver)&&rules.ver>0
    && typeof rules.hash==='string'&&/^[0-9a-f]{64}$/.test(rules.hash),'invalid proxy payment rules');
  need(binding?.verified===true && ['fiat','tnk','tap'].includes(binding.rail),'unverified proxy payout binding');
  return hash('mayhem/proxy/payment-terms/v1',{schema_version:1,lane:'proxy',
    rules_ver:rules.ver,rules_hash:rules.hash,payout_binding:binding});
}

function identity(t) {
  return {billing_id:t.billing_id,billing_attempt:t.billing_attempt,billing_epoch:t.billing_epoch,
    reservation_id:t.reservation_id,reservation_expires_after_epoch:t.reservation_expires_after_epoch,
    reservation_receipt_grace_epochs:t.reservation_receipt_grace_epochs,session_id:t.session_id,
    user:t.buyer_pubkey,rail:t.rail,provider:t.offer.provider_pubkey,payout_revision:t.payout_revision};
}

// Sparse shared-session reads recognize this explicit lane, without pretending
// an external model is a native enclave. The original authorization is immutable;
// after closure max_spend_au represents only the retained, verified settlement.
export async function normalizeProxySpendSessionRecord(r,user,rail,reservationId=null) {
  shape(r,['type','lane','accepted_terms','authorization','settlement_policy',
    ...IDENTITY_FIELDS,
    'max_spend_au','settlement_ready','closed_at','feature_key','reserved_at','recorded_at','updated_at']);
  need(r.type==='targeted_spend_session'&&r.lane==='proxy','invalid proxy session record');
  shape(r.authorization,['terms','buyer_sig','provider_sig']);
  const t=r.authorization.terms; validateProxySpendTerms(t);
  need(r.accepted_terms===await proxySpendTermsDigest(t),'proxy session terms mismatch');
  need(t.settlement_policy_hash===await proxySettlementPolicyDigest(r.settlement_policy),'proxy session outcome policy mismatch');
  for(const s of [r.authorization.buyer_sig,r.authorization.provider_sig]) need(typeof s==='string'&&/^[0-9a-f]{128}$/.test(s),'invalid stored proxy signature');
  const id=identity(t);
  for(const k of Object.keys(id)) need(r[k]===id[k],'proxy session identity mismatch');
  need(r.user===user&&r.rail===rail&&(reservationId===null||r.reservation_id===reservationId),'proxy session key mismatch');
  need(typeof r.settlement_ready==='boolean','invalid proxy settlement status');
  const held=money(r.max_spend_au);
  need(held<=money(t.max_spend_au),'proxy session hold exceeds authorization');
  if(r.settlement_ready) ref(r.closed_at);
  else need(r.closed_at===null&&r.max_spend_au===t.max_spend_au,'proxy live session hold changed');
  integer(r.reserved_at); for(const k of ['feature_key','recorded_at','updated_at']) ref(r[k]);
  return copy(r);
}

export async function prepareProxyReservation(ledger,envelope,context,verify) {
  validateProxyReservationEnvelope(envelope);
  envelope=copy(envelope);
  const {terms:t}=envelope.authorization;
  verifyProxySpendAuthorization(envelope.authorization,verify);
  for(const k of ['network_id','msb_bootstrap','subnet_bootstrap']) need(t[k]===context[k],'proxy reservation network mismatch');
  const digest=await proxySpendTermsDigest(t);
  const key=await proxyReservationFeatureKey(envelope);
  const acceptedKey=proxyReservationKeys.accepted(digest);
  const existing=await ledger.get(acceptedKey);
  if(existing!==null) {
    need(existing.type==='proxy_accepted_spend'&&existing.accepted_terms===digest
      &&same(existing.authorization,envelope.authorization),'proxy accepted reservation is inconsistent');
    return {duplicate:true,writes:[],result:copy(existing.result)};
  }
  need(t.contract_version===context.contract_version,'proxy reservation contract mismatch');
  const applied=checked(await ledger.epochApplyStateRecord());
  const activeEpoch=(applied.pending_epoch??applied.updated_epoch)+1;
  need(Number.isSafeInteger(activeEpoch)&&t.billing_epoch===activeEpoch,'proxy reservation is not for active billing epoch');
  const selected=await readActiveProxyOffer(t.offer,context,path=>ledger.get(path));
  need(selected!==null,'proxy offer is no longer active or admitted');
  const policyRecord=await ledger.get(proxyReservationKeys.settlementPolicy(t.settlement_policy_hash));
  need(policyRecord?.enabled===true,'proxy settlement policy is not enabled');
  await validateProxyNewAcceptance(t,selected.market,selected.membership,selected.offer,policyRecord.policy,activeEpoch);
  const provider=await ledger.get(`prov/${t.offer.provider_pubkey}`);
  need(provider?.status==='active'&&provider.accepted_rails?.includes(t.rail),'proxy provider payment rail is not active');
  const payout=checked(await ledger.providerPayoutBindingForEpoch(t.offer.provider_pubkey,t.rail,t.payout_revision,activeEpoch,{requireCurrentReadiness:true}));
  const rules=await ledger.currentRules();
  need(rules?.ver===t.rules_ver&&await proxyPaymentTermsDigest(rules,payout)===t.payment_terms_hash,'proxy payment terms are not current');
  const balance=checked(await ledger.balanceRecord(t.buyer_pubkey,t.rail));
  checked(ledger.guardianValidateBalanceRecord(balance,t.buyer_pubkey,t.rail));
  if(t.rail==='tap')need(balance.chain_id===payout.chain_id,'proxy TAP balance and payout chains differ');
  const accounting=checked(await ledger.targetedSpendAccountingState(t.buyer_pubkey,t.rail));
  const anchorKey=ledger.receiptBillingKey(t.billing_id);
  const oldAnchor=await ledger.get(anchorKey);
  // Sequential retries are integrated with canonical receipt/close transitions in
  // the next step. Never guess prior cost or allow a second uncertain dispatch.
  need(oldAnchor===null,'proxy billing already exists; recover the accepted attempt');
  need(t.billing_attempt===1&&t.prior_spend_au==='0'&&t.prior_reserved_au==='0','new proxy billing must begin without claimed prior work');
  const sessionKey=ledger.targetedSpendSessionKey(t.buyer_pubkey,t.rail,t.reservation_id);
  const sessionIndexKey=ledger.targetedSpendSessionIndexKey(t.buyer_pubkey,t.rail,t.session_id);
  const billingAttemptKey=ledger.targetedSpendBillingAttemptKey(t.buyer_pubkey,t.rail,t.billing_id,t.billing_attempt);
  const reservationKey=ledger.receiptReservationKey(t.reservation_id);
  for(const path of [sessionKey,sessionIndexKey,billingAttemptKey,reservationKey]) need(await ledger.get(path)===null,'proxy reservation identity is already in use');
  need(!accounting.hold.sessions.some(s=>s.session_id===t.session_id||s.reservation_id===t.reservation_id
    ||(s.billing_id===t.billing_id&&s.billing_attempt===t.billing_attempt)),'proxy reservation collides with a legacy hold');
  const reserved=checked(ledger.safeAddAu(accounting.summary.reserved_au,t.max_spend_au));
  const total=checked(ledger.safeAddAu(accounting.legacy_reserved_au,reserved));
  need(ledger.compareAu(total,balance.au)<=0,'Insufficient unreserved credit balance.');
  const id=identity(t);
  const session={type:'targeted_spend_session',lane:'proxy',accepted_terms:digest,
    authorization:envelope.authorization,settlement_policy:copy(policyRecord.policy),...id,
    max_spend_au:t.max_spend_au,settlement_ready:false,closed_at:null,feature_key:key,
    reserved_at:envelope.at,recorded_at:key,updated_at:key};
  await normalizeProxySpendSessionRecord(session,t.buyer_pubkey,t.rail,t.reservation_id);
  const result={ok:true,op:'proxySpendReserve',accepted_terms:digest,...id,
    reserved_au:total,available_au:checked(ledger.safeSubAu(balance.au,total))};
  return {duplicate:false,result,writes:[
    {key:ledger.targetedSpendSummaryKey(t.buyer_pubkey,t.rail),value:{...accounting.summary,
      reserved_au:reserved,balance_au_at_last_reserve:balance.au,updated_at:key}},
    {key:sessionKey,value:session},
    {key:sessionIndexKey,value:ledger.targetedSpendReservationIndexRecord(session,sessionKey,key)},
    {key:billingAttemptKey,value:ledger.targetedSpendReservationIndexRecord(session,sessionKey,key)},
    {key:anchorKey,value:{type:'proxy_billing_anchor',lane:'proxy',billing_id:t.billing_id,user:t.buyer_pubkey,
      max_total_spend_au:t.max_total_spend_au,latest_attempt:t.billing_attempt,spent_au:'0',reserved_au:t.max_spend_au,
      active_reservation_id:t.reservation_id,created_at:key,updated_at:key}},
    {key:reservationKey,value:{type:'receipt_reservation_identity',lane:'proxy',accepted_terms:digest,...id,
      status:'active',closed_at:null,close_record_key:null,recorded_at:key}},
    {key:acceptedKey,value:{type:'proxy_accepted_spend',accepted_terms:digest,
      authorization:envelope.authorization,settlement_policy:copy(policyRecord.policy),result,recorded_at:key}},
  ]};
}

export function validateProxyUsageEnvelope(v) {
  shape(v,['op','receipt']);
  need(v.op==='proxy_record_usage','invalid proxy usage operation');
  shape(v.receipt,['body','buyer_sig','provider_sig']);
  validateProxyReceiptBody(v.receipt.body);
  for(const s of [v.receipt.buyer_sig,v.receipt.provider_sig])need(typeof s==='string'&&/^[0-9a-f]{128}$/.test(s),'invalid proxy receipt signature');
  proxyCanonicalSigningBytes('proxy-usage-envelope-bound',v);
}

export async function proxyUsageFeatureKey(v) {
  validateProxyUsageEnvelope(v);
  return `proxy/usage/${await proxyReceiptDigest(v.receipt.body)}`;
}

async function acceptedSpend(ledger,digest) {
  const accepted=await ledger.get(proxyReservationKeys.accepted(digest));
  need(accepted?.type==='proxy_accepted_spend'&&accepted.accepted_terms===digest,'proxy accepted spend is missing');
  const t=accepted.authorization?.terms;
  need(await proxySpendTermsDigest(t)===digest,'proxy accepted spend terms changed');
  need(await proxySettlementPolicyDigest(accepted.settlement_policy)===t.settlement_policy_hash,'proxy accepted settlement policy changed');
  return accepted;
}

function headIdentity(head,t) {
  for(const k of ['billing_id','billing_attempt','billing_epoch','session_id','reservation_id','user','rail','provider','payout_revision']) {
    need(head[k]===identity(t)[k],'proxy receipt head identity mismatch');
  }
}

// Canonical stored heads are already signature-checked on admission/application.
// Epoch allocation must still bind all common accounting fields to their exact
// immutable terms. It must never interpret proxy usage as native market demand.
export async function validateProxyCanonicalReceiptHead(ledger,head) {
  need(head?.type==='canonical_receipt_head'&&head.lane==='proxy','invalid proxy receipt head');
  const accepted=await acceptedSpend(ledger,head.accepted_terms);
  const t=accepted.authorization.terms, body=head.receipt?.body;
  await validateProxyReceiptFor(body,t,accepted.settlement_policy);
  headIdentity(head,t);
  need(head.receipt_seq===body.seq&&head.receipt_hash===await proxyReceiptDigest(body)
    &&head.incremental_au===body.au_owed_cum,'proxy receipt head amount or digest mismatch');
  need(head.settlement_ready===body.final,'proxy receipt head finality mismatch');
  if(body.final&&money(body.au_owed_cum)>0n) {
    integer(head.settlement_epoch); integer(head.index_position);
    need(head.settlement_epoch>=t.billing_epoch&&head.epoch===head.settlement_epoch,'proxy receipt settlement epoch mismatch');
  } else need(head.epoch===null&&head.settlement_epoch===null&&head.index_position===null,'proxy unpayable receipt is indexed');
  return accepted;
}

// Checkpoints replace one exact-key head without releasing the accepted hold.
// Finalization retains only the verified attempt charge; shared epoch settlement
// performs the actual debit/earning. No current offer/quote/readiness is consulted
// when recovering work that was already accepted under frozen terms.
export async function prepareProxyUsageReceipt(ledger,envelope,context,verify) {
  validateProxyUsageEnvelope(envelope); envelope=copy(envelope);
  const receipt=envelope.receipt, body=receipt.body;
  const accepted=await acceptedSpend(ledger,body.accepted_terms);
  const t=accepted.authorization.terms;
  for(const k of ['network_id','msb_bootstrap','subnet_bootstrap'])need(t[k]===context[k],'proxy receipt network mismatch');
  const headKey=ledger.receiptHeadKey(t.billing_id,t.billing_attempt);
  const previous=await ledger.get(headKey);
  if(previous!==null) {
    await validateProxyCanonicalReceiptHead(ledger,previous);
    need(previous.accepted_terms===body.accepted_terms,'proxy receipt changed accepted attempt');
  }
  await verifyProxyUsageReceipt(receipt,t,accepted.settlement_policy,previous?.receipt.body??null,verify);
  const key=await proxyUsageFeatureKey(envelope), receiptHash=await proxyReceiptDigest(body);
  const result=head=>({ok:true,op:'proxyRecordUsage',accepted_terms:body.accepted_terms,
    billing_id:t.billing_id,billing_attempt:t.billing_attempt,billing_epoch:t.billing_epoch,
    epoch:head.settlement_epoch,receipt_seq:head.receipt_seq,receipt_hash:head.receipt_hash,
    final:head.settlement_ready,au:head.incremental_au});
  if(previous!==null&&same(previous.receipt,receipt))return {duplicate:true,writes:[],result:result(previous)};
  need(previous?.settlement_ready!==true,'proxy finalized receipt cannot change');
  need(await ledger.get(ledger.receiptConsumedKey(t.billing_id,t.billing_attempt))===null,'proxy consumed receipt cannot advance');
  const anchorKey=ledger.receiptBillingKey(t.billing_id), anchor=await ledger.get(anchorKey);
  need(anchor?.type==='proxy_billing_anchor'&&anchor.lane==='proxy'&&anchor.billing_id===t.billing_id
    &&anchor.user===t.buyer_pubkey&&anchor.latest_attempt===t.billing_attempt
    &&anchor.active_reservation_id===t.reservation_id&&anchor.max_total_spend_au===t.max_total_spend_au
    &&anchor.spent_au===t.prior_spend_au&&anchor.reserved_au===t.max_spend_au
    &&t.prior_reserved_au==='0','proxy receipt billing anchor is inconsistent');
  const state=checked(await ledger.targetedSpendReservationState(t.buyer_pubkey,t.rail,t.reservation_id,t.session_id));
  need(state.kind==='sharded'&&state.session.lane==='proxy'&&state.session.accepted_terms===body.accepted_terms
    &&state.session.settlement_ready===false&&same(state.session.authorization,accepted.authorization)
    &&same(state.session.settlement_policy,accepted.settlement_policy),'proxy receipt does not match its live reserved session');
  const reservationKey=ledger.receiptReservationKey(t.reservation_id), reservation=await ledger.get(reservationKey);
  need(reservation?.type==='receipt_reservation_identity'&&reservation.lane==='proxy'
    &&reservation.accepted_terms===body.accepted_terms,'proxy receipt reservation is missing');
  for(const [field,value]of Object.entries(identity(t)))need(reservation[field]===value,'proxy receipt reservation identity mismatch');
  need(reservation.status==='active'&&reservation.closed_at===null&&reservation.close_record_key===null,'proxy reservation already closed');
  const settlementEpoch=checked(await ledger.receiptSettlementEpoch(checked(await ledger.epochApplyStateRecord())));
  need(t.billing_epoch<=settlementEpoch,'proxy receipt billing epoch is in the future');
  const payable=body.final&&money(body.au_owed_cum)>0n;
  const index=payable?checked(await ledger.nextReceiptEpochIndex(settlementEpoch,t.billing_id,t.billing_attempt)):null;
  const head={type:'canonical_receipt_head',lane:'proxy',accepted_terms:body.accepted_terms,
    billing_id:t.billing_id,billing_attempt:t.billing_attempt,billing_epoch:t.billing_epoch,
    epoch:payable?settlementEpoch:null,settlement_epoch:payable?settlementEpoch:null,
    index_position:index?.position??null,settlement_ready:body.final,user:t.buyer_pubkey,rail:t.rail,
    provider:t.offer.provider_pubkey,payout_revision:t.payout_revision,session_id:t.session_id,
    reservation_id:t.reservation_id,receipt_seq:body.seq,receipt_hash:receiptHash,
    incremental_au:body.au_owed_cum,receipt,feature_key:key,updated_at:key};
  const writes=[{key:headKey,value:head}];
  if(body.final) {
    const closeKey=ledger.receiptReservationCloseKey(t.reservation_id);
    need(await ledger.get(closeKey)===null,'proxy reservation close record already exists');
    const closure=checked(ledger.prepareShardedTargetedReservationClosure({summary:state.summary,
      session:state.session,reservation,head,closeRecordKey:closeKey,closedBy:t.offer.provider_pubkey,
      closedByRole:'provider',at:body.at_ms,reason:'final_receipt'}));
    writes.push({key:ledger.targetedSpendSummaryKey(t.buyer_pubkey,t.rail),value:closure.summary},
      {key:reservationKey,value:closure.reservation},{key:closeKey,value:closure.close_record},
      {key:anchorKey,value:{...anchor,spent_au:body.billing_au_owed_cum,reserved_au:'0',active_reservation_id:null,updated_at:key}});
    if(payable) {
      need(index.index.revision<Number.MAX_SAFE_INTEGER,'proxy receipt epoch revision overflow');
      writes.push({key:state.sessionKey,value:closure.session},
        {key:index.page_key,value:index.page},
        {key:index.index_key,value:{...index.index,revision:index.index.revision+1,updated_at:key}});
    } else {
      // Zero-cost verified completion releases the entire hold and has no debit,
      // earning or epoch index entry. Keep the signed final head for recovery.
      for(const k of [state.sessionKey,state.sessionIndexKey,state.billingAttemptKey])writes.push({key:k,delete:true});
    }
  }
  return {duplicate:false,writes,result:result(head)};
}

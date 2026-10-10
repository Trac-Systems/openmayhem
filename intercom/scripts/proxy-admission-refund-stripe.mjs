// Original-method FIAT return execution. No retail bridging or contract writes.
import { createPublicKey } from 'node:crypto';
import { digest, need, shape, uint } from './proxy-admission-wire.mjs';
import { RefundJournal, validateRefundWork, signed, same, PREPARE, DELIVERY } from './proxy-admission-refund-common.mjs';
import { RetryWork, ReviewWork } from './retail-crypto-verification.mjs';
export { RefundJournal } from './proxy-admission-refund-common.mjs';
export const validateFiatRefundWork=(work,policy,key,now=Date.now())=>validateRefundWork(work,policy,key,'fiat',now);
const objectId = (v, prefix) => typeof v === 'string' && new RegExp(`^${prefix}_[A-Za-z0-9]{1,100}$`).test(v);

export class StripeAdmissionRefund {
  constructor({ account,livemode,currency,credential,policy,key,journalRoot,retryWindowMs,maxLookupPages,fetcher=fetch,now=Date.now }) {
    need(objectId(account,'acct')&&typeof livemode==='boolean'&&/^[a-z]{3}$/.test(currency)&&typeof credential==='string'&&credential.length>=16
      &&uint(retryWindowMs,1000)&&retryWindowMs<=23*3600000&&uint(maxLookupPages,1)&&maxLookupPages<=16,'explicit bounded Stripe refund configuration required');
    need(key.asymmetricKeyType==='ed25519','refund executor key must be Ed25519');
    this.pubkey=createPublicKey(key).export({format:'der',type:'spki'}).subarray(-32).toString('hex');
    this.o={account,livemode,currency,credential,policy,key,journalRoot,retryWindowMs,maxLookupPages,fetcher,now};
  }
  async checked(work) {
    await validateFiatRefundWork(work,this.o.policy,this.pubkey,this.o.now());
    const d=work.authorization.body.destination;
    need(d.stripe_account===this.o.account&&d.livemode===this.o.livemode&&d.currency===this.o.currency,'Stripe custody profile differs');
    return new RefundJournal(this.o.journalRoot,work.authorization_digest);
  }
  request(work) {
    const b=work.authorization.body;
    const body=new URLSearchParams({ payment_intent:b.destination.payment_intent_id, amount:b.amount_base_units,
      'metadata[purpose]':'proxy_admission_refund','metadata[refund_id]':work.refund_id,
      'metadata[authorization_digest]':work.authorization_digest,'metadata[invoice_commitment]':b.invoice_commitment }).toString();
    return { schema_version:1,purpose:'proxy_admission_refund_request',authorization_digest:work.authorization_digest,
      account:this.o.account,livemode:this.o.livemode,currency:this.o.currency,
      idempotency_key:`mayhem-admission-refund-${work.authorization_digest}`,body };
  }
  async prepare(work) {
    const journal=await this.checked(work), expected=this.request(work); let stored=journal.get('request');
    if(!stored) { need(work.action==='prepare','missing original refund request requires review'); stored=journal.retain('request',{...expected,created_at_ms:this.o.now()}); }
    const {created_at_ms,...retained}=stored;
    need(uint(created_at_ms,1)&&same(retained,expected),'retained refund request differs');
    const proof=signed(PREPARE,{schema_version:1,purpose:'proxy_admission_refund_preparation',refund_id:work.refund_id,
      authorization_digest:work.authorization_digest,executor_pubkey:this.pubkey,reference:{rail:'fiat',idempotency_key:stored.idempotency_key}},this.o.key);
    if(work.preparation!==null) need(same(proof,work.preparation),'retained refund preparation differs');
    return proof;
  }
  async stripe(method,suffix,body,signal,key) {
    const response=await this.o.fetcher(`https://api.stripe.com/v1/${suffix}`,{method,redirect:'error',signal:AbortSignal.any([signal,AbortSignal.timeout(15000)]),
      headers:{authorization:`Bearer ${this.o.credential}`,'stripe-account':this.o.account,
        ...(body===undefined?{}:{'content-type':'application/x-www-form-urlencoded','idempotency-key':key})},...(body===undefined?{}:{body})});
    if(!response.ok) { await response.body?.cancel(); if(response.status===429||response.status>=500) throw new RetryWork('refund_processor_unavailable',30);
      throw new ReviewWork('refund_processor_rejected'); }
    const length=response.headers.get('content-length'); need(length===null||(/^\d+$/.test(length)&&Number(length)<=262144),'refund response exceeds bound');
    need(response.body,'missing Stripe refund response'); const reader=response.body.getReader(),chunks=[];let size=0;
    try { for(;;) {const {done,value}=await reader.read();if(done) break;size+=value.byteLength;need(size<=262144,'refund response exceeds bound');chunks.push(Buffer.from(value));} }
    finally { await reader.cancel().catch(()=>{}); }
    return JSON.parse(Buffer.concat(chunks).toString('utf8'));
  }
  checkedRefund(r,work) {
    const b=work.authorization.body,m=r?.metadata;
    need(r?.object==='refund'&&objectId(r.id,'re')&&r.payment_intent===b.destination.payment_intent_id
      &&r.charge===work.payment_evidence.receipt.charge_id&&uint(r.amount,1)&&String(r.amount)===b.amount_base_units&&r.currency===this.o.currency
      &&m?.purpose==='proxy_admission_refund'&&m.refund_id===work.refund_id&&m.authorization_digest===work.authorization_digest
      &&m.invoice_commitment===b.invoice_commitment,'Stripe refund bindings differ');
    return r;
  }
  async find(work,journal,signal) {
    const known=journal.get('refund');
    if(known) { shape(known,['refund_id']);need(objectId(known.refund_id,'re'),'invalid retained refund id');return known.refund_id; }
    let cursor=null,found=null; const seen=new Set();
    for(let n=0;n<this.o.maxLookupPages;n++) {
      const query=new URLSearchParams({payment_intent:work.authorization.body.destination.payment_intent_id,limit:'100',...(cursor?{starting_after:cursor}:{})});
      const page=await this.stripe('GET',`refunds?${query}`,undefined,signal);
      need(page?.object==='list'&&Array.isArray(page.data)&&page.data.length<=100&&typeof page.has_more==='boolean','invalid refund lookup');
      for(const r of page.data) if(r?.metadata?.refund_id===work.refund_id) {
        this.checkedRefund(r,work);if(found&&found!==r.id) throw new ReviewWork('duplicate_refund_requires_review');found=r.id;
      }
      if(!page.has_more) { if(found) journal.retain('refund',{refund_id:found}); return found; }
      const last=page.data.at(-1)?.id; need(objectId(last,'re')&&!seen.has(last),'refund lookup cursor did not progress');seen.add(last);cursor=last;
    }
    throw new ReviewWork('refund_lookup_exceeds_bound'); // No POST after incomplete absence evidence.
  }
  async execute(work,grant,signal) {
    const preparation=await this.prepare(work),journal=await this.checked(work),stored=journal.get('request'),b=work.authorization.body;
    shape(grant,['schema_version','purpose','refund_id','action','first_dispatch_at_ms']);
    need(grant.schema_version===1&&grant.purpose==='proxy_admission_refund'&&grant.refund_id===work.refund_id
      &&['dispatch','reconcile'].includes(grant.action)&&uint(grant.first_dispatch_at_ms,b.approved_at_ms)&&grant.first_dispatch_at_ms<b.expires_at_ms,'invalid refund dispatch grant');
    const pi=await this.stripe('GET',`payment_intents/${b.destination.payment_intent_id}`,undefined,signal);
    need(pi.id===b.destination.payment_intent_id&&pi.status==='succeeded'&&pi.livemode===this.o.livemode&&pi.currency===this.o.currency
      &&uint(pi.amount_received,1)&&String(pi.amount_received)===work.payment_evidence.receipt.amount_base_units
      &&pi.latest_charge===work.payment_evidence.receipt.charge_id&&pi.metadata?.purpose==='proxy_admission_fee'
      &&pi.metadata.invoice_id===b.invoice_id&&pi.metadata.invoice_commitment===b.invoice_commitment
      &&pi.metadata.provider_pubkey===work.invoice.provider_pubkey&&pi.metadata.initial_operation_digest===work.invoice.initial_operation_digest,'original Stripe payment differs');
    let id=await this.find(work,journal,signal);
    if(!id) {
      const now=this.o.now();
      if(now<stored.created_at_ms||now-stored.created_at_ms>=this.o.retryWindowMs) throw new ReviewWork('refund_idempotency_window_expired');
      const charge=await this.stripe('GET',`charges/${pi.latest_charge}`,undefined,signal);
      need(charge.id===pi.latest_charge&&charge.payment_intent===pi.id&&charge.livemode===this.o.livemode&&charge.paid===true&&charge.captured===true
        &&charge.disputed===false&&charge.currency===this.o.currency&&charge.amount===pi.amount_received&&uint(charge.amount_refunded)
        &&BigInt(charge.amount)-BigInt(charge.amount_refunded)>=BigInt(b.amount_base_units),'Stripe remaining refundable balance differs');
      signal.throwIfAborted();
      const beforeSend=this.o.now();
      if(beforeSend>=work.lease_expires_at_ms) throw new RetryWork('refund_lease_expired',1);
      if(beforeSend<stored.created_at_ms||beforeSend-stored.created_at_ms>=this.o.retryWindowMs) throw new ReviewWork('refund_idempotency_window_expired');
      // Reuse identical bytes/key only inside the conservative retention window.
      // The persisted creation time predates every possible first send.
      const result=this.checkedRefund(await this.stripe('POST','refunds',stored.body,signal,stored.idempotency_key),work);
      id=result.id;journal.retain('refund',{refund_id:id});
    }
    // Always retrieve independently. A POST success/pending status is not proof.
    const result=this.checkedRefund(await this.stripe('GET',`refunds/${id}`,undefined,signal),work);
    if(result.status==='pending') throw new RetryWork('refund_pending',30);
    if(result.status!=='succeeded') throw new ReviewWork(result.status==='requires_action'?'refund_requires_action':'refund_failed_or_unknown');
    return signed(DELIVERY,{schema_version:1,purpose:'proxy_admission_refund_delivery',refund_id:work.refund_id,
      authorization_digest:work.authorization_digest,preparation_digest:await digest('refund-preparation',preparation),executor_pubkey:this.pubkey,
      receipt:{rail:'fiat',refund_id:id,stripe_account:this.o.account,livemode:this.o.livemode,payment_intent_id:pi.id,currency:this.o.currency,
        amount_base_units:b.amount_base_units,status:'succeeded'}},this.o.key);
  }
}

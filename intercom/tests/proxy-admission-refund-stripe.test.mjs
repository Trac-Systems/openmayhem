import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { generateKeyPairSync,randomBytes,randomUUID,sign,verify } from 'node:crypto';
import { StripeAdmissionRefund,RefundJournal } from '../scripts/proxy-admission-refund-stripe.mjs';
import { digest,invoiceCommitment } from '../scripts/proxy-admission-wire.mjs';
import { proxyCanonicalSigningBytes } from '../contract/proxy-protocol.js';
import {checkRecoveredExecution,recoveryFor} from './helpers/proxy-admission-refund-recovery.mjs';
const fixtures=JSON.parse(fs.readFileSync(new URL('./fixtures/proxy-admission-worker-v1.json',import.meta.url)));
const h=()=>randomBytes(32).toString('hex');
const key=()=>{const k=generateKeyPairSync('ed25519');return {...k,hex:k.publicKey.export({format:'der',type:'spki'}).subarray(-32).toString('hex')};};
async function fixture(t) {
  const original=structuredClone(fixtures.cases.find(c=>c.rail==='fiat')),review=key(),executor=key(),issuer=key(),invoiceId=randomUUID();
  const invoice={...original.verify_work.invoice,provider_pubkey:h(),entitlement_id:h(),issuer_pubkey:issuer.hex};
  invoice.invoice_commitment=await invoiceCommitment(invoiceId,invoice);
  const reference={...original.verify_work.payment_reference,payment_intent_id:`pi_${h()}`,verified_webhook_event_id:`evt_${h()}`};
  const receipt={...original.evidence_completion.receipt,...reference,amount_base_units:String(BigInt(invoice.amount_base_units)+10n)};delete receipt.verified_webhook_event_id;
  receipt.physical_key=`fiat/${receipt.stripe_account}/${receipt.livemode?'live':'test'}/${receipt.payment_intent_id}`;
  const evidence={receipt,canonical_epoch:100,evidence_commitment:await digest('mayhem/proxy/admission-evidence/v1',{invoice_commitment:invoice.invoice_commitment,payment_reference:reference,receipt})};
  const policy={enabled:true,policy_hash:h(),authorizers:[review.hex],executors:[executor.hex],reasons:['excess'],rails:['fiat'],max_authorization_ms:60000,lease_ms:60000};
  let now=Date.now(); const first=now;
  const body={schema_version:1,purpose:'proxy_admission_refund',refund_id:randomUUID(),invoice_id:invoiceId,payment_id:randomUUID(),
    invoice_commitment:invoice.invoice_commitment,evidence_commitment:evidence.evidence_commitment,physical_key:receipt.physical_key,policy_hash:policy.policy_hash,
    authorizer_pubkey:review.hex,reason:'excess',amount_base_units:'10',destination:{rail:'fiat',stripe_account:receipt.stripe_account,livemode:receipt.livemode,
      payment_intent_id:receipt.payment_intent_id,currency:receipt.currency},authorization_method:'operator_review',return_evidence_hash:h(),approved_at_ms:now-1,expires_at_ms:now+59000};
  const authorization={body,signature:sign(null,proxyCanonicalSigningBytes('mayhem/proxy/admission-refund-authorization/v1',body),review.privateKey).toString('hex')};
  const work={refund_id:body.refund_id,action:'prepare',lease_token:h(),lease_expires_at_ms:now+60000,authorization,
    authorization_digest:await digest('mayhem/proxy/admission-refund-authorization/v1',body),invoice,payment_reference:reference,payment_evidence:evidence,preparation:null,first_dispatch_at_ms:null};
  const grant={schema_version:1,purpose:'proxy_admission_refund',refund_id:body.refund_id,action:'dispatch',first_dispatch_at_ms:now};
  const root=fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(),'admission-refund-test-')));fs.chmodSync(root,0o700);
  t.after(()=>fs.rmSync(root,{recursive:true,force:true}));
  const state={calls:[],refunds:[],dropAfter:false,dropBefore:false,status:'succeeded',hasMore:false,corrupt:null};
  const fetcher=async(url,options)=>{
    assert.equal(new URL(url).origin,'https://api.stripe.com');assert.equal(options.redirect,'error');assert.equal(options.headers['stripe-account'],receipt.stripe_account);
    state.calls.push({url,method:options.method,body:options.body,key:options.headers['idempotency-key']});let response;
    const pathname=new URL(url).pathname;
    if(pathname.startsWith('/v1/payment_intents/')) response={id:receipt.payment_intent_id,status:'succeeded',livemode:receipt.livemode,currency:receipt.currency,
      amount_received:Number(receipt.amount_base_units),latest_charge:receipt.charge_id,metadata:{purpose:'proxy_admission_fee',invoice_id:invoiceId,invoice_commitment:invoice.invoice_commitment,
        provider_pubkey:invoice.provider_pubkey,initial_operation_digest:invoice.initial_operation_digest}};
    else if(pathname.startsWith('/v1/charges/')) response={id:receipt.charge_id,payment_intent:receipt.payment_intent_id,livemode:receipt.livemode,
      paid:true,captured:true,disputed:false,currency:receipt.currency,amount:Number(receipt.amount_base_units),amount_refunded:0};
    else if(pathname==='/v1/refunds'&&options.method==='POST') {
      if(state.dropBefore){state.dropBefore=false;throw new TypeError('simulated connection loss before upstream acceptance');}
      const form=new URLSearchParams(options.body),existing=state.refunds.find(r=>r.metadata.authorization_digest===work.authorization_digest);
      response=existing??{id:`re_${h()}`,object:'refund',amount:Number(form.get('amount')),currency:receipt.currency,charge:receipt.charge_id,
        payment_intent:form.get('payment_intent'),status:state.status,metadata:{purpose:form.get('metadata[purpose]'),refund_id:form.get('metadata[refund_id]'),
          authorization_digest:form.get('metadata[authorization_digest]'),invoice_commitment:form.get('metadata[invoice_commitment]')}};
      if(!existing)state.refunds.push(response);
      if(state.dropAfter){state.dropAfter=false;throw new TypeError('simulated connection loss after upstream acceptance');}
    } else if(pathname==='/v1/refunds') response={object:'list',has_more:state.hasMore,data:state.refunds};
    else response={...state.refunds.find(r=>r.id===pathname.split('/').at(-1)),status:state.status};
    if(state.corrupt)response=state.corrupt(response,pathname);
    return new Response(JSON.stringify(response),{status:200,headers:{'content-type':'application/json'}});
  };
  const options={account:receipt.stripe_account,livemode:receipt.livemode,currency:receipt.currency,credential:'public-test-stripe-refund-fixture-only',policy,key:executor.privateKey,
    journalRoot:root,retryWindowMs:23*3600000,maxLookupPages:2,fetcher,now:()=>now};
  return {work,grant,state,root,options,executor,review,first,advance:(ms)=>{now=first+ms;work.lease_expires_at_ms=now+60000;},adapter:()=>new StripeAdmissionRefund(options),
    resumed:(preparation)=>({...work,action:'reconcile',preparation,first_dispatch_at_ms:grant.first_dispatch_at_ms})};
}
const signal=()=>new AbortController().signal;
test('signed recovery resumes the same Stripe return without replacing identity',async t=>{
 const f=await fixture(t);await checkRecoveredExecution(f);assert.equal(f.state.calls.filter(c=>c.method==='POST').length,1);
});
test('signed recovery cannot reset the original Stripe idempotency retention window',async t=>{
 const f=await fixture(t),p=await f.adapter().prepare(f.work);f.advance(25*3600000);
 const resumed=f.resumed(p);resumed.recovery=await recoveryFor(f,resumed,f.first+25*3600000);
 await assert.rejects(f.adapter().execute(resumed,{...f.grant,action:'reconcile'},signal()),e=>e.reason==='refund_idempotency_window_expired');
 assert.equal(f.state.calls.filter(c=>c.method==='POST').length,0);
});
test('retains exact request before sending, independently retrieves succeeded refund and signs delivery',async t=>{
  const f=await fixture(t),adapter=f.adapter(),preparation=await adapter.prepare(f.work);
  assert.equal(f.state.calls.length,0);const journal=new RefundJournal(f.root,f.work.authorization_digest),request=journal.get('request');
  assert.equal(request.idempotency_key,preparation.body.reference.idempotency_key);
  assert.equal(fs.statSync(journal.file('request')).mode&0o777,0o600);
  const delivery=await adapter.execute(f.work,f.grant,signal());
  assert.equal(delivery.body.receipt.status,'succeeded');assert.equal(delivery.body.receipt.amount_base_units,'10');
  assert(verify(null,proxyCanonicalSigningBytes('mayhem/proxy/admission-refund-delivery/v1',delivery.body),f.executor.publicKey,Buffer.from(delivery.signature,'hex')));
  assert.equal(f.state.calls.filter(c=>c.method==='POST').length,1);assert.match(f.state.calls.at(-1).url,/\/refunds\/re_/);
  const again=await f.adapter().execute(f.resumed(preparation),{...f.grant,action:'reconcile'},signal());
  assert.deepEqual(again,delivery);assert.equal(f.state.calls.filter(c=>c.method==='POST').length,1);
});
test('lost accepted POST acknowledgement recovers by original-payment lookup without sending again',async t=>{
  const f=await fixture(t),p=await f.adapter().prepare(f.work);f.state.dropAfter=true;
  await assert.rejects(f.adapter().execute(f.work,f.grant,signal()),/connection loss/);f.advance(1000);
  const result=await f.adapter().execute(f.resumed(p),{...f.grant,action:'reconcile'},signal());
  assert.equal(result.body.receipt.refund_id,f.state.refunds[0].id);assert.equal(f.state.calls.filter(c=>c.method==='POST').length,1);
});
test('unaccepted connection loss retries exact retained bytes and key only within safe retention',async t=>{
  const f=await fixture(t),p=await f.adapter().prepare(f.work);f.state.dropBefore=true;
  await assert.rejects(f.adapter().execute(f.work,f.grant,signal()));f.advance(1000);
  await f.adapter().execute(f.resumed(p),{...f.grant,action:'reconcile'},signal());
  const posts=f.state.calls.filter(c=>c.method==='POST');assert.equal(posts.length,2);assert.equal(posts[0].body,posts[1].body);assert.equal(posts[0].key,posts[1].key);assert.equal(f.state.refunds.length,1);
});
test('expired idempotency window never creates a new refund; known original result can still complete',async t=>{
  const f=await fixture(t),p=await f.adapter().prepare(f.work);f.state.dropAfter=true;
  await assert.rejects(f.adapter().execute(f.work,f.grant,signal()));f.advance(25*3600000);
  assert.equal((await f.adapter().execute(f.resumed(p),{...f.grant,action:'reconcile'},signal())).body.receipt.status,'succeeded');
  const g=await fixture(t),q=await g.adapter().prepare(g.work);g.advance(25*3600000);
  await assert.rejects(g.adapter().execute(g.resumed(q),{...g.grant,action:'reconcile'},signal()),e=>e.reason==='refund_idempotency_window_expired');
  assert.equal(g.state.calls.filter(c=>c.method==='POST').length,0);
});
test('pending/action-required/failed statuses never attest delivery or repeat a refund',async t=>{
  for(const status of ['pending','requires_action','failed','canceled']) {
    const f=await fixture(t),p=await f.adapter().prepare(f.work);f.state.status=status;
    await assert.rejects(f.adapter().execute(f.work,f.grant,signal()));
    await assert.rejects(f.adapter().execute(f.resumed(p),{...f.grant,action:'reconcile'},signal()));
    assert.equal(f.state.calls.filter(c=>c.method==='POST').length,1);
    f.state.status='succeeded';assert.equal((await f.adapter().execute(f.resumed(p),{...f.grant,action:'reconcile'},signal())).body.receipt.status,'succeeded');
  }
});
test('foreign original payment, refund fields, duplicate lookup and expired lease cannot send',async t=>{
  const f=await fixture(t);await f.adapter().prepare(f.work);f.state.corrupt=(v,p)=>p.includes('payment_intents')?{...v,livemode:!v.livemode}:v;
  await assert.rejects(f.adapter().execute(f.work,f.grant,signal()));assert.equal(f.state.calls.filter(c=>c.method==='POST').length,0);
  f.state.corrupt=null;f.work.lease_expires_at_ms=f.first-1;await assert.rejects(f.adapter().execute(f.work,f.grant,signal()),e=>e.code==='refund_lease_expired');
  assert.equal(f.state.calls.filter(c=>c.method==='POST').length,0);
  f.work.lease_expires_at_ms=f.first+60000;f.state.corrupt=(v,p)=>p.startsWith('/v1/refunds/')?{...v,amount:11}:v;
  await assert.rejects(f.adapter().execute(f.work,f.grant,signal()),/bindings differ/);
  f.state.corrupt=null;fs.unlinkSync(new RefundJournal(f.root,f.work.authorization_digest).file('refund'));
  f.state.refunds.push({...f.state.refunds[0],id:`re_${h()}`});
  await assert.rejects(f.adapter().execute(f.work,f.grant,signal()),e=>e.reason==='duplicate_refund_requires_review');
  assert.equal(f.state.calls.filter(c=>c.method==='POST').length,1);
});
test('tampered signature/policy and insecure journal reject before external requests',async t=>{
  const f=await fixture(t),bad=structuredClone(f.work);bad.authorization.body.amount_base_units='11';
  await assert.rejects(f.adapter().prepare(bad));await assert.rejects(new StripeAdmissionRefund({...f.options,policy:{...f.options.policy,policy_hash:h()}}).prepare(f.work));
  await f.adapter().prepare(f.work);const journal=new RefundJournal(f.root,f.work.authorization_digest);fs.chmodSync(journal.file('request'),0o644);
  await assert.rejects(f.adapter().prepare(f.work),/owner-only/);fs.chmodSync(journal.file('request'),0o600);
  assert.equal(f.state.calls.length,0);
});
test('retention expiration during preflight is rechecked immediately before POST',async t=>{
 const f=await fixture(t);await f.adapter().prepare(f.work);
 f.state.corrupt=(v,p)=>{if(p.includes('/charges/'))f.advance(25*3600000);return v;};
 await assert.rejects(f.adapter().execute(f.work,f.grant,signal()),e=>e.reason==='refund_idempotency_window_expired');
 assert.equal(f.state.calls.filter(c=>c.method==='POST').length,0);
});
test('worker resumes a lost SITE completion ACK without recreating the processor refund',async t=>{
 const {AdmissionRefundWorker}=await import('../scripts/proxy-admission-refund-worker.mjs');
 const f=await fixture(t);let preparation=null,failCompletion=true,defers=0,pulls=0;
 const api={post:async(action,body)=>{
  const base={schema_version:1,purpose:'proxy_admission_refund'};
  if(action==='pull')return {...base,work:pulls++===0?f.work:f.resumed(preparation)};
  if(action==='dispatch'){preparation=body.preparation;return {...f.grant,action:pulls===1?'dispatch':'reconcile'};}
  if(action==='delivered'){if(failCompletion){failCompletion=false;throw new TypeError('simulated completion ACK loss');}return {...base,accepted:true};}
  if(action==='defer'){defers++;assert.equal(body.review,false);return {...base,state:'reconcile'};}
  throw new Error('unexpected worker action');
 }};
 const worker=()=>new AdmissionRefundWorker({api,adapters:{fiat:f.adapter()},timeoutMs:5000});
 assert.equal((await worker().once()).outcome,'retry');assert.equal(defers,1);
 assert.equal((await worker().once()).outcome,'delivered');assert.equal(f.state.calls.filter(c=>c.method==='POST').length,1);
});
test('worker maps processor pending and actionable failure to durable retry/review',async t=>{
 const {AdmissionRefundWorker}=await import('../scripts/proxy-admission-refund-worker.mjs');
 for(const status of ['pending','requires_action']){
  const f=await fixture(t);f.state.status=status;let deferred=null;
  const api={post:async(action,body)=>{
   const base={schema_version:1,purpose:'proxy_admission_refund'};
   if(action==='pull')return {...base,work:f.work};if(action==='dispatch')return f.grant;
   if(action==='defer'){deferred=body;return {...base,state:body.review?'review':'reconcile'};}
   throw new Error('must not confirm incomplete refund');
  }};
  const result=await new AdmissionRefundWorker({api,adapters:{fiat:f.adapter()},timeoutMs:5000}).once();
  assert.equal(result.outcome,status==='pending'?'retry':'review');assert.equal(deferred.review,status!=='pending');
  assert.equal(f.state.calls.filter(c=>c.method==='POST').length,1);
 }
});
test('entry point is opt-in and execution API cannot allocate returns or accept arbitrary URL paths',async()=>{
 const {main,RefundApi}=await import('../scripts/proxy-admission-refund-worker.mjs');
 await assert.rejects(main({}),/disabled/);
 assert.throws(()=>new RefundApi({origin:'http://127.0.0.1:1234',credential:'public-test-execution-credential-only'}));
 const api=new RefundApi({origin:'https://example.invalid',credential:'public-test-execution-credential-only',fetcher:()=>{throw new Error('unexpected network');}});
 await assert.rejects(api.post('reserve',{},signal()),/cannot authorize/);
});

// Isolated integration driver: actual Core adapter and SITE HTTP boundaries,
// explicit synthetic Stripe transport. No environment credentials or live rail.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {createPrivateKey} from 'node:crypto';
import {StripeAdmissionRefund} from '../../scripts/proxy-admission-refund-stripe.mjs';
import {AdmissionRefundWorker,RefundApi} from '../../scripts/proxy-admission-refund-worker.mjs';
const chunks=[];let size=0;for await(const c of process.stdin){size+=c.length;if(size>32768)throw new Error('fixture input too large');chunks.push(c);}
const input=JSON.parse(Buffer.concat(chunks).toString('utf8')),origin=new URL(input.origin);
if(origin.hostname!=='127.0.0.1'||origin.protocol!=='http:'||origin.username||origin.password||origin.pathname!=='/')throw new Error('literal loopback fixture required');
const post=async(route,body)=>{
 const r=await fetch(`${origin.origin}/internal/proxy-admission-refunds/${route}`,{method:'POST',headers:{authorization:`Bearer ${input.credential}`,'content-type':'application/json'},
  body:JSON.stringify({schema_version:1,purpose:'proxy_admission_refund',...body}),signal:AbortSignal.timeout(10000)});
 if(!r.ok)throw new Error(`fixture boundary HTTP ${r.status}`);return r.json();
};
const root=fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(),'refund-http-fixture-')));fs.chmodSync(root,0o700);
try{
 const {work}=await post('pull',{rails:['fiat']}),r=work.payment_evidence.receipt,b=work.authorization.body;let refund=null,posts=0,gets=0;
 const adapter=new StripeAdmissionRefund({account:r.stripe_account,livemode:r.livemode,currency:r.currency,credential:'public-stripe-http-fixture-only',policy:input.policy,
  key:createPrivateKey(input.key),journalRoot:root,retryWindowMs:3600000,maxLookupPages:2,fetcher:async(url,options)=>{
   if(new URL(url).origin!=='https://api.stripe.com')throw new Error('unexpected processor');const p=new URL(url).pathname;let value;
   if(p.includes('/payment_intents/'))value={id:r.payment_intent_id,status:'succeeded',livemode:r.livemode,currency:r.currency,amount_received:Number(r.amount_base_units),latest_charge:r.charge_id,
    metadata:{purpose:'proxy_admission_fee',invoice_id:b.invoice_id,invoice_commitment:b.invoice_commitment,provider_pubkey:work.invoice.provider_pubkey,initial_operation_digest:work.invoice.initial_operation_digest}};
   else if(p.includes('/charges/'))value={id:r.charge_id,payment_intent:r.payment_intent_id,livemode:r.livemode,currency:r.currency,amount:Number(r.amount_base_units),amount_refunded:0,paid:true,captured:true,disputed:false};
   else if(options.method==='POST'){
    posts++;const form=new URLSearchParams(options.body);refund={id:'re_CoreHttpFixture',object:'refund',charge:r.charge_id,payment_intent:form.get('payment_intent'),currency:r.currency,amount:Number(form.get('amount')),status:'succeeded',
     metadata:{purpose:form.get('metadata[purpose]'),refund_id:form.get('metadata[refund_id]'),authorization_digest:form.get('metadata[authorization_digest]'),invoice_commitment:form.get('metadata[invoice_commitment]')}};value=refund;
   }else if(p==='/v1/refunds')value={object:'list',has_more:false,data:refund?[refund]:[]};
   else{gets++;value=refund;}
   return new Response(JSON.stringify(value),{status:200});
  }});
 const api=new RefundApi({origin:origin.origin,credential:input.credential,allowLoopbackHttp:true});
 let delivery,done;const originalPost=api.post.bind(api);
 api.post=async(action,body,signal)=>{
  if(action==='pull')return {schema_version:1,purpose:'proxy_admission_refund',work}; // already leased through actual HTTP above
  const result=await originalPost(action,body,signal);if(action==='delivered'){delivery=body.delivery;done=result;}return result;
 };
 const worker=new AdmissionRefundWorker({api,adapters:{fiat:adapter},timeoutMs:10000});
 const outcome=await worker.once();if(outcome.outcome!=='delivered')throw new Error('refund worker did not deliver');
 const replay=await post('delivered',{refund_id:work.refund_id,lease_token:work.lease_token,delivery});
 console.log(JSON.stringify({posts,independent_refund_reads:gets,done,replay,outcome,refund_id:work.refund_id}));
}finally{fs.rmSync(root,{recursive:true,force:true});}

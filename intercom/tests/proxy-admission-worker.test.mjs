import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import http from 'node:http';
import sodium from 'sodium-native';
import { AdmissionApi, AdmissionWorker, boundedJson, fixedOrigin, tapVerifier, stripeVerifier, tnkVerifier, main, canonicalAdmissionMsbReader } from '../scripts/proxy-admission-worker.mjs';
import { validateWork, validateEvidence, validateEvidenceSet, evidenceCommitment, evidenceSetCommitment, invoiceCommitment, base, PURPOSE } from '../scripts/proxy-admission-wire.mjs';
import { ERC20_TRANSFER_TOPIC, addressTopic, ReviewWork } from '../scripts/retail-crypto-verification.mjs';
const fixture=JSON.parse(fs.readFileSync(new URL('./fixtures/proxy-admission-worker-v1.json',import.meta.url)));
const h=n=>n.toString(16).padStart(64,'0'), clone=structuredClone, now=()=>1700000100000;
const pk=Buffer.alloc(32),sk=Buffer.alloc(64);sodium.crypto_sign_seed_keypair(pk,sk,Buffer.alloc(32,83));
const signer=bytes=>{const sig=Buffer.alloc(64);sodium.crypto_sign_detached(sig,bytes,sk);return sig.toString('hex');};
function policy(work,nonce){return {ok:true,schema_version:1,lane:'proxy',requester:h(1),request_nonce:nonce,context:{...work.invoice.network,epoch:100},
 proof:{view_key:h(2),tree_hash:h(3),signed_length:1,fork:0},registry_enabled:true,fee_policy_hash:work.invoice.fee_policy_hash,active_issuers:[work.invoice.issuer_pubkey],max_permit_epochs:20};}
function worker(c,phase,extra={}){const work=c[phase+'_work'];return new AdmissionWorker({phase,api:{phase},network:work.invoice.network,
 feePolicyHash:work.invoice.fee_policy_hash,issuerPubkey:work.invoice.issuer_pubkey,rails:[c.rail],coreOrigin:'http://127.0.0.1:9',allowLoopbackHttp:true,now,
 verifyReceipt:phase==='verify'?async()=>clone(c.evidence_completion.receipt):null,signPermit:phase==='issue'?signer:null,
 fetcher:async(url,o)=>Response.json(policy(work,JSON.parse(o.body).request_nonce)),...extra});}
const signal=()=>AbortSignal.timeout(10000);

test('worker MSB reads are fresh nonce-bound canonical-policy requests, with no numeric-status fallback', async () => {
 const work=fixture.cases[0].verify_work, nonces=[];
 let mutate=()=>{}, replay=null, mode='normal';
 const read=canonicalAdmissionMsbReader({coreOrigin:'https://fixture.invalid',network:work.invoice.network,
  feePolicyHash:work.invoice.fee_policy_hash,issuerPubkey:work.invoice.issuer_pubkey,
  fetcher:async(url,options)=>{
   assert.equal(url,'https://fixture.invalid/v1/proxy/admission-policy');assert.equal(options.method,'POST');
   assert.equal(options.redirect,'error');const body=JSON.parse(options.body);assert.equal(body.msb_frontier,true);
   assert.deepEqual(Object.keys(body).sort(),['msb_frontier','request_nonce']);nonces.push(body.request_nonce);
   if(mode==='replay') return Response.json(replay);
   const response={...policy(work,body.request_nonce),msb_frontier:true,
    msb_snapshot:{network_id:work.invoice.network.network_id,msb_bootstrap:work.invoice.network.msb_bootstrap,
     view_key:h(20),fork:0,signed_length:100,tree_hash:h(21),observed_at_ms:Date.now()}};
   mutate(response);replay=response;return Response.json(response);
  }});
 assert.equal((await read(signal())).signed_length,100);
 mode='replay';await assert.rejects(read(signal()),/does not match/);mode='normal';
 for(const change of [v=>delete v.msb_snapshot,v=>v.msb_frontier=false,v=>v.extra=true,
  v=>v.msb_snapshot.observed_at_ms-=20000,v=>v.msb_snapshot.network_id='999',
  v=>v.registry_enabled=false,v=>v.active_issuers=[h(1)],v=>v.fee_policy_hash=h(90)]){
  mutate=change;await assert.rejects(read(signal()));
 }
 assert.equal(new Set(nonces).size,nonces.length);
 const before=nonces.length;await assert.rejects(read(AbortSignal.abort()));assert.equal(nonces.length,before);
});

test('four shared fixtures validate hashes, actual positive amounts, sorted topups and original signatures',async()=>{
 for(const c of fixture.cases){await validateWork(c.verify_work,'verify');await validateWork(c.issue_work,'issue');
  const {canonical_epoch,evidence_commitment,receipt}=c.evidence_completion;
  await validateEvidence({canonical_epoch,evidence_commitment,receipt},c.verify_work);
  const total=await validateEvidenceSet(c.issue_work.evidence,c.issue_work);assert.ok(total>=BigInt(c.issue_work.invoice.amount_base_units));
  assert.deepEqual((await worker(c,'verify').complete(c.verify_work,signal())).body,c.evidence_completion);
  assert.deepEqual((await worker(c,'issue').complete(c.issue_work,signal())).body,c.permit_completion);
 }
 assert.equal(fixture.cases[3].evidence_completion.receipt.amount_base_units,'400000000000000000');
 assert.equal(await validateEvidenceSet(fixture.cases[3].issue_work.evidence,fixture.cases[3].issue_work),1100000000000000000n);
});

test('wrong role, network, immutable body and unknown/missing fields cannot sign',async()=>{
 const c=fixture.cases[0];let signed=0;const w=worker(c,'issue',{signPermit:()=>{signed++;throw new Error('must not sign');}});
 for(const mutate of [x=>x.phase='verify',x=>x.invoice.provider_pubkey=h(55),x=>x.permit.provider_pubkey=h(55),x=>x.permit.accepted_amount='1',x=>x.permit.evidence_commitment=h(1),x=>x.permit.nonce=null,x=>x.permit.extra=1,x=>delete x.payment_reference,x=>x.permit.network_id='999']){
  const work=clone(c.issue_work);mutate(work);await assert.rejects(w.complete(work,signal()));
 }assert.equal(signed,0);
 assert.throws(()=>worker(c,'issue',{verifyReceipt:async()=>{}}),/separate/);
});

test('duplicates, unsorted references, receipt swapping and overflow cannot aggregate',async()=>{
 const c=fixture.cases[3];
 for(const mutate of [x=>x.evidence.receipts.push(clone(x.evidence.receipts[0])),x=>x.evidence.receipts.reverse(),x=>x.evidence.receipts[0].payment_reference.log_index=9,
 x=>x.evidence.receipts[0].receipt.finalized=false,x=>x.evidence.receipts[0].receipt.amount_base_units='0',x=>x.evidence.receipts[0].receipt.amount_base_units='340282366920938463463374607431768211455']){
  const w=clone(c.issue_work);mutate(w);await assert.rejects(validateWork(w,'issue'));
 }
 const short=clone(c.issue_work);short.evidence.receipts.pop();short.evidence.evidence_commitment=await evidenceSetCommitment(short,short.evidence.receipts);short.permit.evidence_commitment=short.evidence.evidence_commitment;
 await assert.rejects(worker(c,'issue').complete(short,signal()),/verified_payment_short/);
});

test('canonical disablement, issuer removal, stale nonce, wrong network, expiry/reissue and late payment fail closed',async()=>{
 const c=fixture.cases[0];let signed=0;
 for(const mutate of [x=>x.registry_enabled=false,x=>x.active_issuers=[h(1)],x=>x.fee_policy_hash=h(9),x=>x.context.epoch=111,x=>x.request_nonce=h(1),x=>x.context.network_id='999',x=>x.extra=true]){
  const w=worker(c,'issue',{signPermit:()=>{signed++;throw new Error('must not sign');},fetcher:async(u,o)=>{const p=policy(c.issue_work,JSON.parse(o.body).request_nonce);mutate(p);return Response.json(p);}});
  await assert.rejects(w.complete(c.issue_work,signal()));
 }assert.equal(signed,0);
 for(const mutate of [x=>x.permit.issuance_revision=2,x=>x.evidence.receipts[0].reference_assigned_at_ms=x.invoice.quote_expires_at_ms+1]){
  const w=clone(c.issue_work);mutate(w);await assert.rejects(worker(c,'issue').complete(w,signal()),ReviewWork);
 }
});

test('fixed sources reject URL credentials/query/redirects and bounded readers refuse oversized bodies',async()=>{
 for(const u of ['http://example.com','https://u:p@example.com','https://example.com?x=1','https://example.com/path','http://localhost'])assert.throws(()=>fixedOrigin(u,{allowLoopbackHttp:true}));
 assert.equal(fixedOrigin('http://127.0.0.1:1',{allowLoopbackHttp:true}),'http://127.0.0.1:1');
 await assert.rejects(boundedJson('https://example.com',{fetcher:async()=>new Response('x'.repeat(33)),maxBytes:32}),/bound/);
 await assert.rejects(main({}),/disabled/);
});

test('renewed permits require independently fresh unused canonical facts and preserve every original fee binding',async()=>{
 for(const c of fixture.cases){
  const w=clone(c.issue_work),previous=clone(w.permit),epoch=previous.expires_after_epoch+1;
  w.previous_permit=previous;w.evidence.canonical_epoch=epoch;
  Object.assign(w.permit,{issuance_revision:2,nonce:h(91),valid_from_epoch:epoch,expires_after_epoch:epoch+9});
  let signed=0,mutate=()=>{},seen;
  const issuer=worker(c,'issue',{signPermit:bytes=>{signed++;return signer(bytes);},fetcher:async(u,o)=>{
   const q=JSON.parse(o.body);seen=q;const p={...policy(w,q.request_nonce),provider_pubkey:q.provider_pubkey,recovery:q.recovery,
    enrollment:{provider_pubkey:q.provider_pubkey,entitlement_id:null,provider_revoked:false,admission_revoked:false},
    recovery_state:{entitlement_used:null,invoice_used:null,evidence_used:null,admission_revoked:false,generation:null}};
   p.context.epoch=epoch;mutate(p);return Response.json(p);
  }});
  assert.equal((await issuer.complete(w,signal())).body.permit.issuance_revision,2);assert.equal(signed,1);
  assert.deepEqual(seen.recovery,{entitlement_id:previous.entitlement_id,invoice_commitment:previous.invoice_commitment,evidence_commitment:previous.evidence_commitment});
  for(const change of [p=>p.recovery_state.admission_revoked=true,p=>p.enrollment.provider_revoked=true,
   p=>p.recovery_state.invoice_used={provider_pubkey:h(72),entitlement_id:h(73)},p=>p.recovery_state.generation={revision:1,permit_digest:h(74)},
   p=>p.context.epoch=previous.expires_after_epoch,p=>p.recovery.invoice_commitment=h(75),p=>delete p.recovery_state,p=>p.request_nonce=h(76)]){
   mutate=change;await assert.rejects(issuer.complete(w,signal()));assert.equal(signed,1);
  }
  mutate=()=>{};
  for(const change of [p=>p.previous_permit.accepted_amount='1',p=>p.previous_permit.initial_operation_digest=h(77),
   p=>p.previous_permit.issuance_revision=9,p=>p.previous_permit.expires_after_epoch=epoch,p=>p.permit.nonce=previous.nonce,
   p=>delete p.previous_permit]){
   const altered=clone(w);change(altered);await assert.rejects(issuer.complete(altered,signal()));assert.equal(signed,1);
  }
 }
});

test('expired issuer work requests bounded durable reconciliation; lost recovery ACK retries the same lease',async()=>{
 const c=fixture.cases[0],w=clone(c.issue_work);let fail=false;const calls=[];
 const issuer=worker(c,'issue',{timeoutMs:1000,fetcher:async(u,o)=>{const p=policy(w,JSON.parse(o.body).request_nonce);p.context.epoch=w.permit.expires_after_epoch+1;return Response.json(p);},
  api:{phase:'issue',post:async(action,body)=>{
   calls.push({action,body});if(action==='pull')return {schema_version:1,purpose:PURPOSE,phase:'issue',work:w};
   if(action==='reconcile'&&fail)throw new Error('lost response');
   return {schema_version:1,purpose:PURPOSE,phase:'issue',accepted:true};
  }},signPermit:()=>{throw new Error('must not sign expired work');}});
 assert.equal((await issuer.runOnce()).status,'retry');assert.deepEqual(calls.map(x=>x.action),['pull','reconcile']);
 assert.deepEqual(calls[1].body,base(w));fail=true;calls.length=0;
 assert.equal((await issuer.runOnce()).status,'retry');assert.deepEqual(calls.map(x=>x.action),['pull','reconcile','retry']);
 assert.equal(calls[2].body.lease_token,w.lease_token);assert.equal(calls[2].body.code,'canonical_recovery_unavailable');
 const zero=clone(w);zero.permit.issuance_revision=0;await assert.rejects(worker(c,'issue').complete(zero,signal()),/invalid proxy integer/);
});

test('TAP exact log observes underpayment, uses finalized head and checks network/block',async()=>{
 const c=fixture.cases[0],w=c.verify_work,p=w.payment_reference,r=c.evidence_completion.receipt;
 let chain=31337,finalized=true,wrongBlock=false;const seen=[];
 const verify=tapVerifier({origin:'http://127.0.0.1:8',chainId:31337,tokenContract:p.token_contract,allowLoopbackHttp:true,
  fetcher:async(u,o)=>{const q=JSON.parse(o.body);seen.push(q.method);let result;
   if(q.method==='eth_chainId')result='0x'+chain.toString(16);
   else if(q.method==='eth_getTransactionReceipt')result={transactionHash:p.transaction_hash,status:'0x1',blockNumber:'0x64',blockHash:r.block_hash,logs:[1,3].map(index=>({address:p.token_contract,topics:[ERC20_TRANSFER_TOPIC,addressTopic(r.from_address),addressTopic(r.to_address)],data:'0x'+(index===3?4n:10n).toString(16),logIndex:'0x'+index.toString(16)}))};
   else result=q.params[0]==='finalized'?(finalized?{number:'0x70'}:null):{number:'0x64',hash:wrongBlock?'0x'+h(9):r.block_hash,timestamp:'0x'+(BigInt(r.paid_at_ms)/1000n).toString(16)};
   return Response.json({jsonrpc:'2.0',id:q.id,result});}});
 const actual=await verify(w,signal());assert.equal(actual.amount_base_units,'4');assert.equal(actual.log_index,3);
 finalized=false;await assert.rejects(verify(w,signal()),/finality_unavailable/);finalized=true;wrongBlock=true;await assert.rejects(verify(w,signal()),/block changed/);
 wrongBlock=false;chain=1;await assert.rejects(verify(w,signal()),/network/);assert.ok(!seen.includes('eth_sendRawTransaction'));
});

test('Stripe retrieves exact event/intent/charge and rejects wrong purpose, account, refund and dispute',async()=>{
 const c=fixture.cases[2],w=c.verify_work,p=w.payment_reference;let mutation=null;
 const make=()=>stripeVerifier({account:p.stripe_account,livemode:false,currency:'usd',credential:'synthetic_local_token',fetcher:async(u,o)=>{
  assert.equal(o.headers['stripe-account'],p.stripe_account);const i=w.invoice;let r;
  if(u.includes('/events/'))r={id:p.verified_webhook_event_id,livemode:false,type:'payment_intent.succeeded',data:{object:{id:p.payment_intent_id}},created:1700000050};
  else if(u.includes('/payment_intents/'))r={id:p.payment_intent_id,status:'succeeded',livemode:false,currency:'usd',amount:400,amount_received:400,latest_charge:'ch_localfixture',metadata:{purpose:PURPOSE,invoice_id:w.invoice_id,invoice_commitment:i.invoice_commitment,provider_pubkey:i.provider_pubkey,initial_operation_digest:i.initial_operation_digest}};
  else r={id:'ch_localfixture',payment_intent:p.payment_intent_id,livemode:false,paid:true,captured:true,refunded:false,disputed:false,amount_refunded:0,currency:'usd',amount:400};
  if(mutation)mutation(r);return Response.json(r);}});
 assert.equal((await make()(w,signal())).amount_base_units,'400');
 for(const change of [r=>{if(r.metadata)r.metadata.purpose='retail_credit';},r=>r.livemode=true,r=>r.refunded=true,r=>r.disputed=true]){
  mutation=change;await assert.rejects(make()(w,signal()));
 }
});

test('real loopback queue honors separate credentials, renews leases and recovers original signature after lost ACK',async t=>{
 const c=fixture.cases[0];let stored=null,pulls=0,renewals=0,loseAck=true;const received=[];
 const server=http.createServer(async(req,res)=>{
  let raw='';for await(const chunk of req)raw+=chunk;const body=JSON.parse(raw);const reply=x=>{res.setHeader('content-type','application/json');res.end(JSON.stringify(x));};
  if(req.url==='/v1/proxy/admission-policy')return reply(policy(c.issue_work,body.request_nonce));
  assert.equal(req.headers.authorization,'Bearer synthetic_issuer_credential');assert.equal(body.phase,'issue');
  const action=req.url.split('/').at(-1);
  if(action==='pull'){pulls++;const work=clone(c.issue_work);work.lease_expires_at_ms=now()+1000;return reply({schema_version:1,purpose:PURPOSE,phase:'issue',work});}
  if(action==='renew'){renewals++;return reply({schema_version:1,purpose:PURPOSE,phase:'issue',lease_expires_at_ms:now()+60000});}
  if(action==='permit'){received.push(body);if(stored)assert.deepEqual(body,stored);else stored=body;
   if(loseAck){loseAck=false;res.statusCode=503;return res.end('{}');}return reply({schema_version:1,purpose:PURPOSE,phase:'issue',accepted:true});}
  assert.equal(action,'retry');reply({schema_version:1,purpose:PURPOSE,phase:'issue',accepted:true});
 });
 await new Promise(r=>server.listen(0,'127.0.0.1',r));t.after(()=>new Promise(r=>server.close(r)));
 const origin=`http://127.0.0.1:${server.address().port}`;
 const api=new AdmissionApi({origin,credential:'synthetic_issuer_credential',phase:'issue',allowLoopbackHttp:true});
 const w=worker(c,'issue',{api,coreOrigin:origin,fetcher:fetch});
 assert.equal((await w.runOnce()).status,'retry');assert.equal((await w.runOnce()).status,'accepted');
 assert.equal(pulls,2);assert.equal(renewals,2);assert.equal(received.length,2);assert.deepEqual(stored,c.permit_completion);
 await assert.rejects(api.post('evidence',{phase:'issue'},signal()),/role/);
});

test('connected synthetic payment verification → authenticated canonical policy → issuer → actual canonical publication preserves native funds',async t=>{
 const {familyAdminFixture}=await import('./helpers/proxy-family-admin-fixture.mjs');
 const {default:MayhemFeature}=await import('../features/mayhem/index.js');
 const {requestProxyAdmissionPolicy}=await import('../src/rpc.js');
 const {proxyOperationDigest,proxyRegistryFeatureKey}=await import('../contract/proxy-protocol.js');
 const f=await familyAdminFixture(t),envelope=await f.create(),c=clone(fixture.cases[0]);
 const invoice=c.verify_work.invoice;
 Object.assign(invoice,{network:f.network,provider_pubkey:f.provider.publicKey,issuer_pubkey:f.issuer.publicKey,
  initial_operation_digest:await proxyOperationDigest(envelope.intent),fee_policy_hash:f.config.fee_policy_hash});
 invoice.invoice_commitment=await invoiceCommitment(c.verify_work.invoice_id,invoice);
 const peer={...f.peer,wallet:{...f.peer.wallet,publicKey:f.issuer.publicKey,sign:bytes=>f.issuer.wallet.sign(bytes).toString('hex')},base:{writable:false,view:f.base.view}};
 const client=new MayhemFeature(peer,{});t.after(()=>client.stop());
 let policyReads=0;
 client.requestService=async(service,value)=>{
  const authorization=f.feature._verifyServiceRequest(service,value,{admin:f.admin.publicKey,transport:f.issuer.publicKey});assert.ok(authorization);
  policyReads++;return await f.feature._handleService(service,authorization.payload,authorization);
 };
 peer.protocol={instance:{features:{mayhem:client}}};
 let completedEvidence=null,completedPermit=null,issueWork=null;
 const bodyBound=16384;
 const server=http.createServer(async(req,res)=>{
  const reply=(value,status=200)=>{res.writeHead(status,{'content-type':'application/json','cache-control':'no-store'});res.end(JSON.stringify(value));};
  try {
   let raw='';for await(const chunk of req){raw+=chunk;assert.ok(Buffer.byteLength(raw)<=bodyBound);}const body=JSON.parse(raw);
   if(req.url==='/v1/proxy/admission-policy')return reply(await requestProxyAdmissionPolicy(peer,body));
   if(req.url==='/'){
    const p=c.verify_work.payment_reference,r=c.evidence_completion.receipt;let result;
    if(body.method==='eth_chainId')result='0x7a69';
    else if(body.method==='eth_getTransactionReceipt')result={transactionHash:p.transaction_hash,status:'0x1',blockNumber:'0x64',blockHash:r.block_hash,
     logs:[{address:p.token_contract,topics:[ERC20_TRANSFER_TOPIC,addressTopic(r.from_address),addressTopic(r.to_address)],data:'0x'+BigInt(invoice.amount_base_units).toString(16),logIndex:'0x3'}]};
    else result=body.params[0]==='finalized'?{number:'0x70'}:{number:'0x64',hash:r.block_hash,timestamp:'0x'+(BigInt(r.paid_at_ms)/1000n).toString(16)};
    return reply({jsonrpc:'2.0',id:body.id,result});
   }
   const phase=body.phase,action=req.url.split('/').at(-1);
   assert.equal(req.headers.authorization,`Bearer synthetic_${phase}_credential`);
   if(action==='pull'){
    assert.deepEqual(body.rails,['tap']);let work;
    if(phase==='verify')work=completedEvidence?null:c.verify_work;
    else if(completedPermit)work=null;
    else {
     assert.ok(completedEvidence);
     const receipts=[{payment_reference:c.verify_work.payment_reference,reference_assigned_at_ms:c.verify_work.reference_assigned_at_ms,receipt:completedEvidence.receipt}];
     const evidence={canonical_epoch:completedEvidence.canonical_epoch,evidence_commitment:await evidenceSetCommitment(c.verify_work,receipts),receipts};
     const permit={...c.issue_work.permit,...f.network,provider_pubkey:invoice.provider_pubkey,issuer_pubkey:invoice.issuer_pubkey,
      initial_operation_digest:invoice.initial_operation_digest,fee_policy_hash:invoice.fee_policy_hash,invoice_commitment:invoice.invoice_commitment,evidence_commitment:evidence.evidence_commitment};
     issueWork={...c.verify_work,phase:'issue',lease_token:h(32),payment_reference:null,reference_assigned_at_ms:null,evidence,permit};work=issueWork;
    }return reply({schema_version:1,purpose:PURPOSE,phase,work});
   }
   if(action==='evidence'){
    assert.equal(phase,'verify');const {canonical_epoch,evidence_commitment,receipt}=body;
    await validateEvidence({canonical_epoch,evidence_commitment,receipt},c.verify_work);completedEvidence=body;
   }else {assert.equal(action,'permit');assert.equal(phase,'issue');assert.deepEqual(body.permit,issueWork.permit);completedPermit=body;}
   reply({schema_version:1,purpose:PURPOSE,phase,accepted:true});
  }catch(e){reply({error:String(e.message)},500);}
 });
 await new Promise(r=>server.listen(0,'127.0.0.1',r));t.after(()=>new Promise(r=>server.close(r)));
 const origin=`http://127.0.0.1:${server.address().port}`;
 const options={coreOrigin:origin,network:f.network,feePolicyHash:invoice.fee_policy_hash,issuerPubkey:f.issuer.publicKey,rails:['tap'],allowLoopbackHttp:true,now};
 const api=phase=>new AdmissionApi({origin,credential:`synthetic_${phase}_credential`,phase,allowLoopbackHttp:true});
 const verifier=new AdmissionWorker({...options,phase:'verify',api:api('verify'),verifyReceipt:tapVerifier({origin,chainId:31337,tokenContract:invoice.collection.token_contract,allowLoopbackHttp:true})});
 const issuer=new AdmissionWorker({...options,phase:'issue',api:api('issue'),signPermit:bytes=>f.issuer.wallet.sign(bytes).toString('hex')});
 const nativeBalance=(await f.base.view.get('bal/existing-customer')).value,nativePayout=(await f.base.view.get('payout/epoch/542')).value;
 assert.equal((await verifier.runOnce()).status,'accepted');assert.equal((await issuer.runOnce()).status,'accepted');assert.equal(policyReads,2);
 envelope.admission={permit:completedPermit.permit,issuer_signature:completedPermit.issuer_signature};
 const key=await proxyRegistryFeatureKey(envelope),result=await f.controller.submit(key,envelope);
 assert.equal(result.ok,true);assert.equal(f.calls,1);assert.equal((await f.controller.submit(key,envelope)).duplicate,true);assert.equal(f.calls,1);
 assert.deepEqual((await f.base.view.get('bal/existing-customer')).value,nativeBalance);assert.deepEqual((await f.base.view.get('payout/epoch/542')).value,nativePayout);
 assert.equal(f.journal.list().length,0);
 if(process.env.PROXY_ADMISSION_FIXTURE_OUTPUT) fs.writeFileSync(process.env.PROXY_ADMISSION_FIXTURE_OUTPUT,JSON.stringify({
  schema_version:1,synthetic:true,site_queue:'loopback fixture; real SITE invoice/outbox acceptance is separate',
  verify_work:c.verify_work,evidence_completion:completedEvidence,issue_work:issueWork,permit_completion:completedPermit,
  canonical_result:result,canonical_appends:f.calls,policy_reads:policyReads,native_balance:nativeBalance,native_payout:nativePayout,model_calls:0,live_payments:0,
 },null,2)+'\n',{mode:0o600});
});

test('one worker keeps a single active lease request',async()=>{
 const c=fixture.cases[0];let release;
 const api={phase:'issue',post:async()=>await new Promise(r=>{release=r;})};
 const w=worker(c,'issue',{api});const pending=w.runOnce();
 await assert.rejects(w.runOnce(),/busy/);release({schema_version:1,purpose:PURPOSE,phase:'issue',work:null});assert.equal((await pending).status,'idle');

});

import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {generateKeyPairSync,randomBytes,randomUUID,sign,verify} from 'node:crypto';
import {TnkAdmissionRefund} from '../scripts/proxy-admission-refund-tnk.mjs';
import {RefundJournal} from '../scripts/proxy-admission-refund-common.mjs';
import {AdmissionRefundWorker} from '../scripts/proxy-admission-refund-worker.mjs';
import {digest,invoiceCommitment} from '../scripts/proxy-admission-wire.mjs';
import {proxyCanonicalSigningBytes} from '../contract/proxy-protocol.js';
import {testWallet,tnkRefundRuntime} from './helpers/proxy-admission-refund-tnk-runtime.mjs';
import {checkRecoveredExecution} from './helpers/proxy-admission-refund-recovery.mjs';
const fixtures=JSON.parse(fs.readFileSync(new URL('./fixtures/proxy-admission-worker-v1.json',import.meta.url)));
const h=()=>randomBytes(32).toString('hex');
const key=()=>{const k=generateKeyPairSync('ed25519');return {...k,hex:k.publicKey.export({format:'der',type:'spki'}).subarray(-32).toString('hex')};};
async function fixture(t){
 const original=structuredClone(fixtures.cases.find(c=>c.rail==='tnk')),review=key(),executor=key(),issuer=key(),id=randomUUID();
 const wallet=await testWallet(),returnWallet=await testWallet(),payer=await testWallet();
 const invoice={...original.verify_work.invoice,provider_pubkey:h(),entitlement_id:h(),issuer_pubkey:issuer.hex,
  network:{...original.verify_work.invoice.network,network_id:'919'},collection:{...original.verify_work.invoice.collection,destination:wallet.address}};
 invoice.invoice_commitment=await invoiceCommitment(id,invoice);
 const reference={network:'testnet1',transaction_hash:h()};
 const receipt={...original.evidence_completion.receipt,...reference,amount_base_units:String(BigInt(invoice.amount_base_units)+10n),from_address:payer.address,to_address:wallet.address};
 receipt.physical_key=`tnk/testnet1/${reference.transaction_hash}`;
 const evidence={receipt,canonical_epoch:100,evidence_commitment:await digest('mayhem/proxy/admission-evidence/v1',{invoice_commitment:invoice.invoice_commitment,payment_reference:reference,receipt})};
 const policy={enabled:true,policy_hash:h(),authorizers:[review.hex],executors:[executor.hex],reasons:['excess'],rails:['tnk'],max_authorization_ms:60000,lease_ms:60000};
 const now=Date.now(),body={schema_version:1,purpose:'proxy_admission_refund',refund_id:randomUUID(),invoice_id:id,payment_id:randomUUID(),
  invoice_commitment:invoice.invoice_commitment,evidence_commitment:evidence.evidence_commitment,physical_key:receipt.physical_key,policy_hash:policy.policy_hash,
  authorizer_pubkey:review.hex,reason:'excess',amount_base_units:'10',destination:{rail:'tnk',network:'testnet1',address:returnWallet.address},
  authorization_method:'operator_review',return_evidence_hash:h(),approved_at_ms:now-1,expires_at_ms:now+59000};
 const authorization={body,signature:sign(null,proxyCanonicalSigningBytes('mayhem/proxy/admission-refund-authorization/v1',body),review.privateKey).toString('hex')};
 const work={refund_id:body.refund_id,action:'prepare',lease_token:h(),lease_expires_at_ms:now+60000,authorization,
  authorization_digest:await digest('mayhem/proxy/admission-refund-authorization/v1',body),invoice,payment_reference:reference,payment_evidence:evidence,preparation:null,first_dispatch_at_ms:null};
 const grant={schema_version:1,purpose:'proxy_admission_refund',refund_id:body.refund_id,action:'dispatch',first_dispatch_at_ms:now};
 const root=fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(),'refund-tnk-journal-')));fs.chmodSync(root,0o700);
 const runtime=await tnkRefundRuntime({wallet,network:invoice.network});
 t.after(async()=>{await runtime.close();fs.rmSync(root,{recursive:true,force:true});});
 const options={msb:runtime.msb,network:invoice.network,networkName:'testnet1',canonicalFrontier:runtime.frontier,policy,key:executor.privateKey,journalRoot:root,finality:2,timeoutSeconds:1};
 return {work,grant,root,options,executor,review,runtime,adapter:()=>new TnkAdmissionRefund(options),
  resumed:p=>({...work,action:'reconcile',preparation:p,first_dispatch_at_ms:grant.first_dispatch_at_ms})};
}
const signal=()=>AbortSignal.timeout(5000);
test('signed recovery resumes the original TNK payload without a second transfer',async t=>{
 const f=await fixture(t);await checkRecoveredExecution(f);assert.equal(f.runtime.state.broadcasts.length,1);assert.equal(f.runtime.state.scans,0);
});
test('real TNK payload is retained before dispatch and final delivery is canonical, exact and repeatable after ledger growth',async t=>{
 const f=await fixture(t),adapter=f.adapter(),p=await adapter.prepare(f.work,signal());
 const journal=new RefundJournal(f.root,f.work.authorization_digest),stored=journal.get('request');
 assert.equal(f.runtime.state.broadcasts.length,0);assert.equal(stored.transaction_hash,p.body.reference.transaction_hash);
 assert.equal(fs.statSync(journal.file('request')).mode&0o777,0o600);
 const result=await adapter.execute(f.work,f.grant,signal());
 assert.equal(result.body.receipt.to_address,f.work.authorization.body.destination.address);
 assert.notEqual(result.body.receipt.to_address,f.work.payment_evidence.receipt.from_address,'return authority must not be inferred from sender');
 assert.equal(result.body.receipt.amount_base_units,'10');assert.equal(f.runtime.state.broadcasts.length,1);assert.equal(f.runtime.state.scans,0);
 assert(verify(null,proxyCanonicalSigningBytes('mayhem/proxy/admission-refund-delivery/v1',result.body),f.executor.publicKey,Buffer.from(result.signature,'hex')));
 for(let i=0;i<30;i++)await f.runtime.view.put(`more/${i}`,Buffer.from('other'));
 assert.deepEqual(await f.adapter().execute(f.resumed(p),{...f.grant,action:'reconcile'},signal()),result);
 assert.equal(f.runtime.state.broadcasts.length,1);assert.equal(f.runtime.state.scans,0);
});
test('loss before acceptance repeats identical signed payload, while loss after acceptance only verifies original',async t=>{
 for(const fault of ['dropBefore','dropAfter']){
  const f=await fixture(t),p=await f.adapter().prepare(f.work,signal());f.runtime.state[fault]=true;
  await assert.rejects(f.adapter().execute(f.work,f.grant,signal()),/simulated loss/);
  const result=await f.adapter().execute(f.resumed(p),{...f.grant,action:'reconcile'},signal());
  assert.equal(result.body.receipt.transaction_hash,p.body.reference.transaction_hash);
  assert.equal(f.runtime.state.broadcasts.length,fault==='dropBefore'?2:1);
  for(const sent of f.runtime.state.broadcasts)assert.deepEqual(sent,f.runtime.state.broadcasts[0]);
 }
});
test('pending finality never rebroadcasts and resolves when the same canonical entry matures',async t=>{
 const f=await fixture(t),p=await f.adapter().prepare(f.work,signal());f.runtime.state.pad=0;
 await assert.rejects(f.adapter().execute(f.work,f.grant,signal()),e=>e.code==='awaiting_finality');
 await assert.rejects(f.adapter().execute(f.resumed(p),{...f.grant,action:'reconcile'},signal()),e=>e.code==='awaiting_finality');
 assert.equal(f.runtime.state.broadcasts.length,1);
 for(let i=0;i<3;i++)await f.runtime.view.put(`later/${i}`,Buffer.from('padding'));
 assert.equal((await f.adapter().execute(f.resumed(p),{...f.grant,action:'reconcile'},signal())).body.receipt.finalized,true);
 assert.equal(f.runtime.state.broadcasts.length,1);
});
test('foreign canonical hash, stale authority, expired lease and changed custody reject without a send',async t=>{
 for(const fault of ['tree','stale','lease','custody']){
  const f=await fixture(t),p=await f.adapter().prepare(f.work,signal());
  if(fault==='lease')f.work.lease_expires_at_ms=1;
  if(fault==='custody')f.options.msb.wallet=await testWallet();
  if(fault==='tree')f.options.canonicalFrontier=async()=>({...await f.runtime.frontier(),tree_hash:h()});
  if(fault==='stale')f.options.canonicalFrontier=async()=>({...await f.runtime.frontier(),observed_at_ms:Date.now()-16000});
  await assert.rejects(f.adapter().execute(f.resumed(p),{...f.grant,action:'reconcile'},signal()));
  assert.equal(f.runtime.state.broadcasts.length,0);
 }
});
test('missing, modified or unprotected retained payload cannot be regenerated after dispatch',async t=>{
 for(const fault of ['missing','amount','signature','mode']){
  const f=await fixture(t),p=await f.adapter().prepare(f.work,signal()),journal=new RefundJournal(f.root,f.work.authorization_digest);
  if(fault==='missing')fs.unlinkSync(journal.file('request'));
  if(fault==='mode')fs.chmodSync(journal.file('request'),0o644);
  if(fault==='amount'||fault==='signature'){
   const value=journal.get('request');value.payload.tro[fault==='amount'?'am':'is']='00'.repeat(fault==='amount'?16:64);fs.writeFileSync(journal.file('request'),JSON.stringify(value));
  }
  await assert.rejects(f.adapter().execute(f.resumed(p),{...f.grant,action:'reconcile'},signal()));assert.equal(f.runtime.state.broadcasts.length,0);
 }
});
test('aborted unresolved transport retains its single permit and never accumulates pending sends',async t=>{
 const f=await fixture(t),adapter=f.adapter();await adapter.prepare(f.work,signal());
 let release,started;const ready=new Promise(resolve=>started=resolve);
 f.runtime.msb.broadcastPartialTransaction=()=>{started();return new Promise(resolve=>release=resolve);};
 const controller=new AbortController(),pending=adapter.execute(f.work,f.grant,controller.signal);
 await ready;controller.abort();await assert.rejects(pending);
 await assert.rejects(adapter.execute(f.work,f.grant,signal()),e=>e.code==='refund_transport_busy');
 release(false);await Promise.resolve();await Promise.resolve();
 assert.equal(adapter.pending,null);
});
test('completion ACK loss uses identical attestation through worker restart and grown canonical ledger',async t=>{
 const f=await fixture(t);let p=null,first=true,proof=null,pulls=0;
 const api={async post(action,body){
  if(action==='pull')return {work:pulls++===0?f.work:f.resumed(p)};
  if(action==='dispatch'){p=body.preparation;return {...f.grant,action:pulls===1?'dispatch':'reconcile'};}
  if(action==='defer')return {state:'reconcile'};
  if(action==='delivered'){
   if(first){first=false;proof=body.delivery;await f.runtime.view.put('grew',Buffer.from('metadata'));throw Error('ACK lost');}
   assert.deepEqual(body.delivery,proof);return {accepted:true};
  }throw Error('unexpected action');
 }};
 const worker=()=>new AdmissionRefundWorker({api,adapters:{tnk:f.adapter()},timeoutMs:5000});
 assert.equal((await worker().once()).outcome,'retry');assert.equal((await worker().once()).outcome,'delivered');assert.equal(f.runtime.state.broadcasts.length,1);
});
test('protected encrypted custody restores the existing address; rejects wrong receiver, key type and file permissions',async t=>{
 const {readRefundTnkWallet}=await import('../scripts/proxy-admission-refund-tnk-runtime.mjs');
 const {default:Wallet}=await import('trac-wallet');
 const f=await fixture(t),password=Buffer.from('public-test-passphrase'),file=path.join(f.root,'custody.pem'),pass=path.join(f.root,'password');
 fs.writeFileSync(pass,password,{mode:0o600});
 const pair=key(),address=Wallet.encodeBech32m('testtrac',Buffer.from(pair.hex,'hex'));
 fs.writeFileSync(file,pair.privateKey.export({format:'pem',type:'pkcs8',cipher:'aes-256-cbc',passphrase:password}),{mode:0o600});
 const config={format:'encrypted_pkcs8',key_file:file,password_file:pass,receiver:address,network_name:'testnet1'};
 const loaded=await readRefundTnkWallet(config);assert.equal(loaded.address,address);loaded.secretKey.fill(0);
 await assert.rejects(readRefundTnkWallet({...config,receiver:f.runtime.msb.wallet.address}));
 fs.chmodSync(file,0o644);await assert.rejects(readRefundTnkWallet(config),/owner-only/);fs.chmodSync(file,0o600);
 const wrong=generateKeyPairSync('ec',{namedCurve:'secp256k1'});
 fs.writeFileSync(file,wrong.privateKey.export({format:'pem',type:'pkcs8',cipher:'aes-256-cbc',passphrase:password}));
 await assert.rejects(readRefundTnkWallet(config),/Ed25519/);
 f.runtime.msb.wallet.exportToFile(file,password);
 const native=await readRefundTnkWallet({...config,format:'trac_wallet',receiver:f.runtime.msb.wallet.address});
 assert.equal(native.address,f.runtime.msb.wallet.address);native.secretKey.fill(0);
});
test('canonical return lookup remains enabled when admissions are disabled, but rejects nonce replay or stale/foreign snapshots',async t=>{
 const {canonicalRefundMsbReader}=await import('../scripts/proxy-admission-refund-tnk-runtime.mjs');
 const f=await fixture(t),nonces=[];let last,mode='normal';
 const read=canonicalRefundMsbReader({coreOrigin:'https://example.invalid',network:f.work.invoice.network,fetcher:async(url,options)=>{
  assert.equal(url,'https://example.invalid/v1/proxy/admission-policy');assert.equal(options.redirect,'error');
  const q=JSON.parse(options.body);nonces.push(q.request_nonce);assert.equal(q.msb_frontier,true);
  if(mode==='replay')return Response.json(last);
  last={ok:true,schema_version:1,lane:'proxy',requester:h(),request_nonce:q.request_nonce,context:{...f.work.invoice.network,epoch:100},
   proof:{view_key:h(),tree_hash:h(),signed_length:1,fork:0},registry_enabled:false,fee_policy_hash:h(),active_issuers:[h()],max_permit_epochs:10,
   msb_frontier:true,msb_snapshot:await f.runtime.frontier()};
  if(mode==='stale')last.msb_snapshot.observed_at_ms-=16000;
  if(mode==='foreign')last.msb_snapshot.msb_bootstrap=h();
  return Response.json(last);
 }});
 assert.equal((await read(signal())).network_id,'919');
 for(const fault of ['replay','stale','foreign']){mode=fault;await assert.rejects(read(signal()));}
 assert.equal(new Set(nonces).size,nonces.length);
});
test('a changed canonical fork after delivered return never reuses the old completion or sends replacement',async t=>{
 const f=await fixture(t),p=await f.adapter().prepare(f.work,signal());await f.adapter().execute(f.work,f.grant,signal());
 const base=f.runtime.msb.state.base.view,core=base.core;
 base.core={key:core.key,fork:core.fork+1,treeHash:core.treeHash.bind(core)};
 await assert.rejects(f.adapter().execute(f.resumed(p),{...f.grant,action:'reconcile'},signal()),/prefix changed/);
 assert.equal(f.runtime.state.broadcasts.length,1);
});
test('a treasury shortage or expired authorization is durably deferred before any dispatch',async t=>{
 for(const expired of [false,true]){
  const f=await fixture(t);let deferred,dispatches=0;
  if(expired)f.options.now=()=>f.work.authorization.body.expires_at_ms+1;
  else f.runtime.msb.state.getNodeEntry=async()=>({balance:Buffer.alloc(16)});
  const api={async post(action,body){
   if(action==='pull')return {work:f.work};
   if(action==='dispatch'){dispatches++;throw Error('must not dispatch');}
   if(action==='defer'){deferred=body;return {state:body.review?'review':'reserved'};}
   throw Error('unexpected action');
  }};
  const outcome=await new AdmissionRefundWorker({api,adapters:{tnk:f.adapter()},timeoutMs:5000}).once();
  assert.equal(outcome.outcome,expired?'review':'retry');assert.equal(deferred.reason,expired?'refund_authorization_expired':'refund_treasury_short');
  assert.equal(dispatches,0);assert.equal(f.runtime.state.broadcasts.length,0);
 }
});
test('canonical identity changing during retained-prefix verification rejects completion',async t=>{
 const f=await fixture(t),p=await f.adapter().prepare(f.work,signal());await f.adapter().execute(f.work,f.grant,signal());
 const base=f.runtime.msb.state.base.view,core=base.core;
 // Current observation hashes once through the authority's snapshot and once
 // through the local core. The next local hash checks the retained prefix.
 let reads=0;const replacement={key:core.key,fork:core.fork,async treeHash(length){
  const hash=await core.treeHash(length);if(++reads===2)replacement.fork++;return hash;
 }};base.core=replacement;
 await assert.rejects(f.adapter().execute(f.resumed(p),{...f.grant,action:'reconcile'},signal()),/retained prefix differs/);
 assert.equal(f.runtime.state.broadcasts.length,1);
});

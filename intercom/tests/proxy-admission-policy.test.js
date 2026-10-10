import test from 'node:test';
import assert from 'node:assert/strict';
import b4a from 'b4a';
import MayhemFeature from '../features/mayhem/index.js';
import { CONTRACT_VERSION } from '../contract/contract.js';
import { createProxyCanonicalSnapshot } from '../features/mayhem/proxy-canonical-view.js';
import { readProxyAdmissionPolicy, validateProxyAdmissionPolicyRequest, PROXY_ADMISSION_POLICY_SERVICE } from '../features/mayhem/proxy-admission-policy.js';
import { createServer, requestProxyAdmissionPolicy } from '../src/rpc.js';
import { familyAdminFixture } from './helpers/proxy-family-admin-fixture.mjs';
import { createTnkDiscoveryFixture, testTnkAddress } from './helpers/proxy-admission-tnk-fixture.mjs';
import { createAdmissionMsbReader } from '../features/mayhem/proxy-admission-msb.js';
import { canonicalAdmissionMsbReader } from '../scripts/proxy-admission-worker.mjs';
const h=n=>n.toString(16).padStart(64,'0');
const hex=v=>b4a.toString(v,'hex');

test('admission policy reads one real signed canonical key with no ledger writes or private facts',async t=>{
 const f=await familyAdminFixture(t),snapshot=createProxyCanonicalSnapshot(f.peer,CONTRACT_VERSION),keys=[],before=f.base.local.length;
 const value=await readProxyAdmissionPolicy({request:{requester:f.issuer.publicKey,request_nonce:h(1)},withCanonicalSnapshot:body=>snapshot(s=>body({...s,read:key=>{keys.push(key);return s.read(key);}}))});
 assert.deepEqual(keys,['proxy/v1/config']); assert.equal(value.registry_enabled,true);
 assert.deepEqual(value.active_issuers,[f.issuer.publicKey]); assert.equal(value.max_permit_epochs,20);
 assert.equal(value.context.epoch,100); assert.ok(value.proof.signed_length>0);
 assert.equal(f.base.local.length,before);
 for(const key of ['invoice_commitment','evidence_commitment','accepted_amount','provider_pubkey','balances','private_key']) assert.equal(value[key],undefined);
});

test('policy disabled, issuer removal and unavailable canonical source remain explicit',async t=>{
 const f=await familyAdminFixture(t),snapshot=createProxyCanonicalSnapshot(f.peer,CONTRACT_VERSION),request={requester:f.issuer.publicKey,request_nonce:h(2)};
 await f.base.append({type:'seed',entries:[['proxy/v1/config',{...f.config,enabled:false,active_issuers:[h(4)]}]]});await f.base.update();
 const response=await readProxyAdmissionPolicy({request,withCanonicalSnapshot:snapshot});
 assert.equal(response.registry_enabled,false);assert.deepEqual(response.active_issuers,[h(4)]);
 for(const bad of [{...request,request_nonce:'bad'},{...request,provider_pubkey:'invalid'},{request_nonce:h(1)}]) assert.throws(()=>validateProxyAdmissionPolicyRequest(bad));
 await assert.rejects(readProxyAdmissionPolicy({request,withCanonicalSnapshot:async()=>{throw new Error('unsigned source');}}),/unsigned source/);
});

test('policy uses four permits and timed out reads keep their permit until cleanup',async t=>{
 const callbacks=[];t.mock.method(globalThis,'setTimeout',f=>{callbacks.push(f);return 0;});t.mock.method(globalThis,'clearTimeout',()=>{});
 let release;const wait=new Promise(r=>{release=r;});const blocked=async()=>{await wait;throw new Error('cleaned');};
 const request={requester:h(1),request_nonce:h(2)},reads=Array.from({length:4},()=>readProxyAdmissionPolicy({request,withCanonicalSnapshot:blocked}));
 const rejects=Promise.all(reads.map(p=>assert.rejects(p,/expired/)));callbacks.forEach(f=>f());await rejects;
 await assert.rejects(readProxyAdmissionPolicy({request,withCanonicalSnapshot:blocked}),/capacity/);
 release();await new Promise(r=>setImmediate(r));
 await assert.rejects(readProxyAdmissionPolicy({request,withCanonicalSnapshot:blocked}),/cleaned/);
});

test('actual RPC uses authenticated policy relay; altered request, stale nonce and wrong network fail closed',async t=>{
 const f=await familyAdminFixture(t),admin=f.feature;
 const msb=await createTnkDiscoveryFixture({hash:h(91),destination:testTnkAddress(h(92)),networkId:f.network.network_id,msbBootstrap:f.network.msb_bootstrap});
 t.after(()=>msb.close());let msbReads=0;
 const readMsb=createAdmissionMsbReader(msb.msb);
 f.peer.proxyAdmissionMsbSnapshot=async context=>{msbReads++;return readMsb(context);};
 const peer={...f.peer,wallet:{...f.peer.wallet,publicKey:f.issuer.publicKey,sign:bytes=>hex(f.issuer.wallet.sign(b4a.from(bytes)))},base:{writable:false,view:f.base.view}};
 const client=new MayhemFeature(peer,{});t.after(()=>client.stop());
 let previous,mode='normal';const nonces=[];
 client.requestService=async(service,envelope)=>{
  assert.equal(service,PROXY_ADMISSION_POLICY_SERVICE);
  const check=value=>admin._verifyServiceRequest(service,value,{admin:f.admin.publicKey,transport:f.issuer.publicKey});
  assert.equal(check({...envelope,payload:{...envelope.payload,request_nonce:h(99)}}),null);
  const authorization=check(envelope);assert.ok(authorization);nonces.push(authorization.payload.request_nonce);
  if(mode==='replay')return structuredClone(previous);
  previous=await admin._handleService(service,authorization.payload,authorization);
  if(mode==='network')previous.context.network_id='999';
  if(mode==='msb_network')previous.msb_snapshot.network_id='999';
  if(mode==='msb_stale')previous.msb_snapshot.observed_at_ms-=20000;
  return { ...structuredClone(previous), relayed:true, request_id:h(98) };
 };
 peer.protocol={instance:{features:{mayhem:client}}};
 const server=createServer(peer);await new Promise(r=>server.listen(0,'127.0.0.1',r));t.after(()=>new Promise(r=>server.close(r)));
 const query={request_nonce:h(1)},before=f.base.local.length;
 const response=await fetch(`http://127.0.0.1:${server.address().port}/v1/proxy/admission-policy`,{method:'POST',body:JSON.stringify(query),headers:{'content-type':'application/json'}});
 assert.equal(response.status,200);const body=await response.json();assert.equal(body.request_nonce,query.request_nonce);assert.deepEqual(body.active_issuers,[f.issuer.publicKey]);
  assert.equal(msbReads,0);assert.equal(body.msb_snapshot,undefined);
 assert.equal(Object.hasOwn(body,'relayed'),false);assert.equal(Object.hasOwn(body,'request_id'),false);
 await requestProxyAdmissionPolicy(peer,query);assert.equal(new Set(nonces).size,2);assert.ok(nonces.every(n=>n!==query.request_nonce));
 const readFrontier=canonicalAdmissionMsbReader({coreOrigin:`http://127.0.0.1:${server.address().port}`,network:f.network,
  feePolicyHash:f.config.fee_policy_hash,issuerPubkey:f.issuer.publicKey,allowLoopbackHttp:true});
 const proof=await readFrontier(AbortSignal.timeout(3000));
 assert.equal(proof.view_key,msb.view.core.key.toString('hex'));assert.equal(msbReads,1);
 const frontierQuery={...query,msb_frontier:true};
 mode='replay';await assert.rejects(requestProxyAdmissionPolicy(peer,frontierQuery),/does not match/);
 mode='msb_network';await assert.rejects(requestProxyAdmissionPolicy(peer,frontierQuery),/foreign/);
 mode='msb_stale';await assert.rejects(requestProxyAdmissionPolicy(peer,frontierQuery),/stale/);
 mode='normal';
 const enrollment=await requestProxyAdmissionPolicy(peer,{...query,provider_pubkey:h(71)});
 assert.equal(enrollment.provider_pubkey,h(71));assert.equal(enrollment.enrollment.entitlement_id,null);
 const recovery={entitlement_id:h(72),invoice_commitment:h(73),evidence_commitment:h(74)};
 const inspection=await requestProxyAdmissionPolicy(peer,{...query,provider_pubkey:h(71),recovery});
 assert.deepEqual(inspection.recovery,recovery);assert.equal(inspection.recovery_state.entitlement_used,null);
 assert.equal(inspection.recovery_state.admission_revoked,false);
 mode='replay';await assert.rejects(requestProxyAdmissionPolicy(peer,query),/does not match/);
 mode='network';await assert.rejects(requestProxyAdmissionPolicy(peer,query),/does not match/);
 await assert.rejects(requestProxyAdmissionPolicy({},query),/not ready/);
 await assert.rejects(requestProxyAdmissionPolicy(peer,{...query,issuer:h(1)}),/Invalid/);
 await assert.rejects(requestProxyAdmissionPolicy(peer,{...query,msb_frontier:false}),/Invalid/);
 await assert.rejects(requestProxyAdmissionPolicy(peer,{...frontierQuery,provider_pubkey:h(71)}),/Invalid/);
 assert.equal(f.base.local.length,before);
 assert.equal(msb.view.core.length,11);
});

test('permit recovery reads exact unused/consumed/revoked/superseded keys on one signed snapshot without writes', async t => {
 const f=await familyAdminFixture(t),snapshot=createProxyCanonicalSnapshot(f.peer,CONTRACT_VERSION);
 const recovery={entitlement_id:h(81),invoice_commitment:h(82),evidence_commitment:h(83)};
 const request={requester:f.issuer.publicKey,request_nonce:h(84),provider_pubkey:h(85),recovery};
 const keys=[];const read=()=>readProxyAdmissionPolicy({request,withCanonicalSnapshot:fn=>snapshot(s=>fn({...s,read:key=>{keys.push(key);return s.read(key);}}))});
 const first=await read();assert.deepEqual(first.recovery_state,{entitlement_used:null,invoice_used:null,evidence_used:null,admission_revoked:false,generation:null});
 assert.equal(keys.length,8);assert.equal(new Set(keys).size,8);assert.equal(first.context.epoch,100);
 const owner={provider_pubkey:h(91),entitlement_id:h(92)},generation={revision:3,permit_digest:h(93)};
 await f.base.append({type:'seed',entries:[
  [`proxy/v1/admission-used/invoice/${recovery.invoice_commitment}`,owner],
  [`proxy/v1/admission-used/evidence/${recovery.evidence_commitment}`,owner],
  [`proxy/v1/admission-revoked/${recovery.entitlement_id}`,{revoked:true}],
  [`proxy/v1/admission-generation/${recovery.entitlement_id}`,generation],
 ]});await f.base.update();const before=f.base.local.length;
 const next=await read();assert.equal(next.enrollment.entitlement_id,null);
 assert.deepEqual(next.recovery_state,{entitlement_used:null,invoice_used:owner,evidence_used:owner,admission_revoked:true,generation});
 assert.equal(f.base.local.length,before);assert.equal(next.proof.signed_length>first.proof.signed_length,true);
 for(const bad of [{...request,provider_pubkey:undefined},{...request,recovery:{...recovery,path:'private'}},{...request,recovery:{...recovery,entitlement_id:'invalid'}}]) assert.throws(()=>validateProxyAdmissionPolicyRequest(bad));
 await f.base.append({type:'seed',entries:[[`proxy/v1/admission-generation/${recovery.entitlement_id}`,{revision:0,permit_digest:h(93)}]]});await f.base.update();
 await assert.rejects(read(),/invalid admission generation/);
});


test('collector can read another public provider admission, never private payment evidence or writes', async t => {
 const f=await familyAdminFixture(t),snapshot=createProxyCanonicalSnapshot(f.peer,CONTRACT_VERSION);
 const provider=h(71),entitlement=h(72),request={requester:f.issuer.publicKey,request_nonce:h(73),provider_pubkey:provider};
 const observed=[];const read=()=>readProxyAdmissionPolicy({request,withCanonicalSnapshot:fn=>snapshot(s=>fn({...s,read:key=>{observed.push(key);return s.read(key);}}))});
 assert.deepEqual((await read()).enrollment,{provider_pubkey:provider,entitlement_id:null,provider_revoked:false,admission_revoked:false});
 await f.base.append({type:'seed',entries:[[`proxy/v1/provider/${provider}`,{entitlement:{id:entitlement}}],
  [`proxy/v1/admission-used/entitlement/${entitlement}`,{provider_pubkey:provider,entitlement_id:entitlement}]]});await f.base.update();
 const before=f.base.local.length;
 assert.equal((await read()).enrollment.entitlement_id,entitlement);
 await f.base.append({type:'seed',entries:[[`proxy/v1/admission-revoked/${entitlement}`,{revoked:true}]]});await f.base.update();
 const revoked=await read();assert.equal(revoked.enrollment.admission_revoked,true);
 assert.equal(f.base.local.length,before+1);assert.ok(observed.every(key=>key.startsWith('proxy/v1/')));
 assert.equal(Object.hasOwn(revoked,'payments'),false);
 await f.base.append({type:'seed',entries:[[`proxy/v1/admission-used/entitlement/${entitlement}`,{provider_pubkey:h(99),entitlement_id:entitlement}]]});await f.base.update();
 await assert.rejects(read(),/ownership differs/);
});

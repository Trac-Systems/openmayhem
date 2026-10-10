// Ephemeral signed financial state and real loopback RPC for Rust integration.
// No outside peer, money transfer, model request or production configuration.
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import readline from 'node:readline';
import b4a from 'b4a';
import Corestore from 'corestore';
import Hyperbee from 'hyperbee';
import MayhemFeature from '../../features/mayhem/index.js';
import { createProxyCanonicalSnapshot } from '../../features/mayhem/proxy-canonical-view.js';
import { createServer } from '../../src/rpc.js';
import { CONTRACT_VERSION } from '../../contract/contract.js';
import { proxyReceiptFixture } from './proxy-finance.js';
import { closure } from './proxy-closure.js';
import { prepareProxyClose, prepareProxyExpiry } from '../../contract/proxy-closure.js';
import { proxyBuyerReceiptSigningBytes, proxyProviderReceiptSigningBytes, proxyBuyerClosureSigningBytes, proxyProviderClosureSigningBytes, proxyBuyerExpirySigningBytes } from '../../contract/proxy-finance.js';
import { proxyUsageFeatureKey, proxyReservationFeatureKey } from '../../contract/proxy-reservations.js';
import { proxyCloseFeatureKey, proxyExpireFeatureKey } from '../../contract/proxy-closure.js';
import { execute, signProviderKyb } from './contract.js';
import { PROXY_OPERATOR_STATE_SERVICE } from '../../features/mayhem/proxy-operator-state.js';
const execution=process.argv[4]?JSON.parse(process.argv[4]):null;
const deferred=Boolean(execution)||process.argv[5]==='unreserved';
const f=await proxyReceiptFixture(process.argv[2]??'tnk',process.argv[3]??'llm',execution,deferred,process.argv[5]==='expiry');
let reserved=!deferred;
const root=await fs.mkdtemp(path.join(os.tmpdir(),'proxy-finance-rpc-'));
const store=new Corestore(root);
const view=new Hyperbee(store.get({name:'financial'}),{keyEncoding:'utf-8',valueEncoding:'json',extension:false});
await view.ready();
let written=new Set();
async function sync() {
  const batch=view.batch();
  try {
    const keys=new Set(f.storage.values.keys());
    for(const key of written)if(!keys.has(key))await batch.del(key);
    for(const [key,value] of f.storage.values)await batch.put(key,value);
    await batch.flush();written=keys;
  } finally {await batch.close();}
}
await sync();
const wallet=(identity)=>({publicKey:identity.publicKey,sign:bytes=>b4a.toString(identity.wallet.sign(bytes),'hex'),verify:f.peer.wallet.verify});
const peer={...f.peer,wallet:wallet(f.admin),base:{writable:true,isIndexer:true,
  key:b4a.from(f.network.subnet_bootstrap,'hex'),view,_applyState:{view}}};
const admin=new MayhemFeature(peer,{withProxyCanonicalSnapshot:createProxyCanonicalSnapshot(peer,CONTRACT_VERSION)});admin.key='mayhem';
const local={...peer,wallet:wallet(f.provider)};
const participant=new MayhemFeature(local,{});participant.key='mayhem';
const buyerLocal={...peer,wallet:wallet(f.buyer)};
const buyerParticipant=new MayhemFeature(buyerLocal,{});buyerParticipant.key='mayhem';
buyerParticipant._adminKey=async()=>f.admin.publicKey;
participant._adminKey=async()=>f.admin.publicKey;
let mutation=null,calls=0,operatorCalls=0,operatorUnavailable=false;
let submissions=0,publications=0,pending=null,publicationMode=null,publicationTail=Promise.resolve();
// Test transport uses the actual RPC, receipt validator/accounting planner and
// canonical signed-view service. The remote relay/indexer transport is simulated;
// its durable append journal has separate real-Autobase integration coverage.
async function applyReceipt(key,value) {
  const reserve=value.op==='proxy_spend_reserve',waiver=value.op==='proxy_close_reservation',expires=value.op==='proxy_expire_reservation';
  if(key!==await (reserve?proxyReservationFeatureKey(value):expires?proxyExpireFeatureKey(value):waiver?proxyCloseFeatureKey(value):proxyUsageFeatureKey(value)))throw new Error('fixture publication key differs');
  const plan=reserve?await f.prepare(value):expires?await prepareProxyExpiry(f.ledger,value,f.context,f.peer.wallet.verify):waiver?await prepareProxyClose(f.ledger,value,f.context,f.peer.wallet.verify):await f.finalize(value);
  if(plan.writes.length) {await f.apply(plan);await sync();publications++;}
  if(reserve)reserved=true;
  return plan.result;
}
participant.relay=buyerParticipant.relay=(key,value)=>{
  const job=publicationTail.then(async()=>{
    submissions++;
    if(publicationMode==='pending') {pending={key,value};return {ok:true,pending:true};}
    const result=await applyReceipt(key,value);
    if(publicationMode==='lost_ack')throw new Error('fixture lost acknowledgment after canonical application');
    return result;
  });
  publicationTail=job.catch(()=>{});return job;
};
function requestsFor(actor) { return async(service,request)=>{
  calls++;
  if(service===PROXY_OPERATOR_STATE_SERVICE) {
    operatorCalls++;
    if(operatorUnavailable)throw new Error('fixture canonical operator service unavailable');
  }
  if(mutation==='delay')await new Promise(resolve=>setTimeout(resolve,250));
  const verified=admin._verifyServiceRequest(service,request,{admin:f.admin.publicKey,transport:actor.publicKey});
  if(!verified)throw new Error('fixture signature rejected');
  const result=await admin._handleService(service,verified.payload,verified);
  if(mutation==='nonce')result.request_nonce='0'.repeat(64);
  if(mutation==='network')result.context.network_id='wrong-network';
  if(mutation==='hold')result.session.max_spend_au='0';
  if(mutation==='signature')result.accepted.authorization.buyer_sig='0'.repeat(128);
  if(mutation==='unknown')result.unexpected='invalid';
  // Match the actual remote service envelope, not only direct admin dispatch.
  return { ...result, relayed:true, request_id:'a'.repeat(64) };
}; }
participant.requestService=requestsFor(f.provider);
buyerParticipant.requestService=requestsFor(f.buyer);
buyerLocal.protocol={instance:{features:{mayhem:buyerParticipant}}};
local.protocol={instance:{features:{mayhem:participant}}};
const server=createServer(local);
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
const buyerServer=createServer(buyerLocal);
await new Promise(resolve=>buyerServer.listen(0,'127.0.0.1',resolve));
console.log(JSON.stringify({url:`http://127.0.0.1:${server.address().port}/v1`,identity:f.network,
  buyer_url:`http://127.0.0.1:${buyerServer.address().port}/v1`,buyer:f.buyer.publicKey,requester:f.provider.publicKey,
  policy:f.settlementPolicy,authorization:f.authorize(f.terms).authorization}));
try {
  for await(const command of readline.createInterface({input:process.stdin})) {
    if(command==='stop')break;
    if(command.startsWith('{')) {
      const request=JSON.parse(command);
      if(request.set_offer && Object.keys(request).length===1) {
        const result=await f.submit(await f.envelope({kind:'set_offer',offer:request.set_offer}));
        if(result.ok!==true)throw new Error('fixture offer update rejected');
        await sync();
        console.log(JSON.stringify({done:'set_offer',offer:request.set_offer}));continue;
      }
      if(request.sign_expiry) {
        console.log(JSON.stringify({buyer_sig:b4a.toString(f.buyer.wallet.sign(proxyBuyerExpirySigningBytes(request.sign_expiry)),'hex')}));continue;
      }
      if(Number.isSafeInteger(request.epoch)&&request.epoch>=0) {
        f.context.epoch=request.epoch;
        await f.storage.put('epoch/apply/state',{epoch:request.epoch,updated_epoch:request.epoch,pending_epoch:null});await sync();
        console.log(JSON.stringify({done:'epoch',epoch:request.epoch}));continue;
      }
      if(request.sign_waiver) {
        const body=request.sign_waiver;
        console.log(JSON.stringify({provider_sig:b4a.toString(f.provider.wallet.sign(proxyProviderClosureSigningBytes(body)),'hex'),
          buyer_sig:b4a.toString(f.buyer.wallet.sign(proxyBuyerClosureSigningBytes(body)),'hex')}));continue;
      }
      if(request.sign_receipt) {
        const body=request.sign_receipt;
        console.log(JSON.stringify({provider_sig:b4a.toString(f.provider.wallet.sign(proxyProviderReceiptSigningBytes(body)),'hex'),
          buyer_sig:b4a.toString(f.buyer.wallet.sign(proxyBuyerReceiptSigningBytes(body)),'hex')}));continue;
      }
      if(request.bind_lease && !reserved && Object.keys(request).length===1 && /^[0-9a-f]{64}$/.test(request.bind_lease)) {
        f.terms.capacity_lease=request.bind_lease;
        console.log(JSON.stringify({done:'bind_lease',policy:f.settlementPolicy,authorization:f.authorize(f.terms).authorization}));continue;
      }
      if(reserved||Object.keys(request).join(',')!=='reserve'||!/^[0-9a-f]{64}$/.test(request.reserve))throw new Error('invalid fixture reservation');
      f.terms.capacity_lease=request.reserve;
      await f.apply(await f.prepare(f.authorize(f.terms)));await sync();reserved=true;
      console.log(JSON.stringify({done:'reserve',authorization:f.authorize(f.terms).authorization}));continue;
    }
    if(command==='final') {await f.apply(await f.finalize(await f.receipt()));await sync();}
    else if(command==='close') {await f.apply(await prepareProxyClose(f.ledger,await closure(f),f.context,f.peer.wallet.verify));await sync();}
    else if(command==='foreign')participant.peer.wallet=wallet(f.buyer); // Expected local actor/transport remains provider.
    else if(command==='reset')mutation=null;
    else if(command==='operator_verify'||command==='operator_revoke') {
      // Explicit test-only native enrollment; the KYB mutation/signature and
      // canonical read path are real. Never expose synthetic legal fields over
      // the helper protocol or treat them as inference-integrity evidence.
      const providerKey=`prov/${f.provider.publicKey}`;
      await f.storage.put(providerKey,{...await f.read(providerKey),provider:f.provider.publicKey});
      const value=command==='operator_verify'?{op:'set_provider_kyb',provider:f.provider.publicKey,
        legal_name:'Local Test Operator',jurisdiction:'DE',proof_hash:'9'.repeat(64),kyb_ref:'LOCAL-TEST-ONLY',
        verified_at:1788000000,schema_version:1}:{op:'revoke_provider_kyb',provider:f.provider.publicKey};
      if(command==='operator_verify')value.admin_sig=signProviderKyb(f.admin.wallet,value);
      const log=console.log;console.log=()=>{};
      let result;
      try {result=await execute(f.contract,f.storage,command==='operator_verify'?'setProviderKyb':'revokeProviderKyb',value,f.admin.publicKey,88);}
      finally {console.log=log;}
      if(result?.ok!==true)throw new Error(`fixture canonical operator command failed: ${result?.message}`);
      await sync();
    }
    else if(command==='operator_inactive') {await f.storage.put(`prov/${f.provider.publicKey}`,{...await f.read(`prov/${f.provider.publicKey}`),provider:f.provider.publicKey,status:'banned'});await sync();}
    else if(command==='operator_absent') {await f.storage.del(`kyb/${f.provider.publicKey}`);await sync();}
    else if(command==='operator_unknown') {await f.storage.put(`kyb/${f.provider.publicKey}`,{provider:f.provider.publicKey,status:'self_reported'});await sync();}
    else if(command==='operator_unavailable')operatorUnavailable=true;
    else if(command==='operator_available')operatorUnavailable=false;
    else if(command==='ephemeral_test_wallet_seeds') {
      // Ephemeral fixture process only, through its private test stdin/stdout;
      // never exposed by RPC or a production wallet/signing service.
      console.log(JSON.stringify({provider:Array.from(f.provider.wallet.secretKey.subarray(0,32)),
        buyer:Array.from(f.buyer.wallet.secretKey.subarray(0,32))}));continue;
    }
    else if(command==='status'){}
    else if(command==='state') {console.log(JSON.stringify({summary:await f.read(f.summaryKey),balance:await f.read(f.balanceKey),billing:await f.read(f.ledger.receiptBillingKey(f.terms.billing_id))}));continue;}
    else if(command==='publish_pending')publicationMode='pending';
    else if(command==='publish_lost_ack')publicationMode='lost_ack';
    else if(command==='flush_publication') {if(pending){await applyReceipt(pending.key,pending.value);pending=null;}publicationMode=null;}
    else if(['nonce','network','hold','signature','unknown','delay'].includes(command))mutation=command;
    else throw new Error('unknown fixture command');
    console.log(JSON.stringify({done:command,calls,operator_calls:operatorCalls,submissions,publications}));
  }
} finally {
  await participant.stop();await buyerParticipant.stop();await admin.stop();server.closeAllConnections();buyerServer.closeAllConnections();
  await new Promise(resolve=>buyerServer.close(resolve));
  await new Promise(resolve=>server.close(resolve));await view.close();await store.close();
  await fs.rm(root,{recursive:true,force:true});
}

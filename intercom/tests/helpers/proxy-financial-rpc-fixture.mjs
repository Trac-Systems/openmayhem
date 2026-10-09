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
import { prepareProxyClose } from '../../contract/proxy-closure.js';
import { proxyBuyerReceiptSigningBytes, proxyProviderReceiptSigningBytes, proxyBuyerClosureSigningBytes, proxyProviderClosureSigningBytes } from '../../contract/proxy-finance.js';
import { proxyUsageFeatureKey } from '../../contract/proxy-reservations.js';
import { proxyCloseFeatureKey } from '../../contract/proxy-closure.js';
const execution=process.argv[4]?JSON.parse(process.argv[4]):null;
const f=await proxyReceiptFixture(process.argv[2]??'tnk',process.argv[3]??'llm',execution,Boolean(execution));
let reserved=!execution;
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
participant._adminKey=async()=>f.admin.publicKey;
let mutation=null,calls=0;
let submissions=0,publications=0,pending=null,publicationMode=null,publicationTail=Promise.resolve();
// Test transport uses the actual RPC, receipt validator/accounting planner and
// canonical signed-view service. The remote relay/indexer transport is simulated;
// its durable append journal has separate real-Autobase integration coverage.
async function applyReceipt(key,value) {
  const waiver=value.op==='proxy_close_reservation';
  if(key!==await (waiver?proxyCloseFeatureKey(value):proxyUsageFeatureKey(value)))throw new Error('fixture publication key differs');
  const plan=waiver?await prepareProxyClose(f.ledger,value,f.context,f.peer.wallet.verify):await f.finalize(value);
  if(plan.writes.length) {await f.apply(plan);await sync();publications++;}
  return plan.result;
}
participant.relay=(key,value)=>{
  const job=publicationTail.then(async()=>{
    submissions++;
    if(publicationMode==='pending') {pending={key,value};return {ok:true,pending:true};}
    const result=await applyReceipt(key,value);
    if(publicationMode==='lost_ack')throw new Error('fixture lost acknowledgment after canonical application');
    return result;
  });
  publicationTail=job.catch(()=>{});return job;
};
participant.requestService=async(service,request)=>{
  calls++;
  if(mutation==='delay')await new Promise(resolve=>setTimeout(resolve,250));
  const verified=admin._verifyServiceRequest(service,request,{admin:f.admin.publicKey,transport:f.provider.publicKey});
  if(!verified)throw new Error('fixture signature rejected');
  const result=await admin._handleService(service,verified.payload,verified);
  if(mutation==='nonce')result.request_nonce='0'.repeat(64);
  if(mutation==='network')result.context.network_id='wrong-network';
  if(mutation==='hold')result.session.max_spend_au='0';
  if(mutation==='signature')result.accepted.authorization.buyer_sig='0'.repeat(128);
  if(mutation==='unknown')result.unexpected='invalid';
  return result;
};
local.protocol={instance:{features:{mayhem:participant}}};
const server=createServer(local);
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
console.log(JSON.stringify({url:`http://127.0.0.1:${server.address().port}/v1`,identity:f.network,
  requester:f.provider.publicKey,authorization:f.authorize(f.terms).authorization}));
try {
  for await(const command of readline.createInterface({input:process.stdin})) {
    if(command==='stop')break;
    if(command.startsWith('{')) {
      const request=JSON.parse(command);
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
      if(reserved||Object.keys(request).join(',')!=='reserve'||!/^[0-9a-f]{64}$/.test(request.reserve))throw new Error('invalid fixture reservation');
      f.terms.capacity_lease=request.reserve;
      await f.apply(await f.prepare(f.authorize(f.terms)));await sync();reserved=true;
      console.log(JSON.stringify({done:'reserve',authorization:f.authorize(f.terms).authorization}));continue;
    }
    if(command==='final') {await f.apply(await f.finalize(await f.receipt()));await sync();}
    else if(command==='close') {await f.apply(await prepareProxyClose(f.ledger,await closure(f),f.context,f.peer.wallet.verify));await sync();}
    else if(command==='foreign')participant.peer.wallet=wallet(f.buyer); // Expected local actor/transport remains provider.
    else if(command==='reset')mutation=null;
    else if(command==='status'){}
    else if(command==='publish_pending')publicationMode='pending';
    else if(command==='publish_lost_ack')publicationMode='lost_ack';
    else if(command==='flush_publication') {if(pending){await applyReceipt(pending.key,pending.value);pending=null;}publicationMode=null;}
    else if(['nonce','network','hold','signature','unknown','delay'].includes(command))mutation=command;
    else throw new Error('unknown fixture command');
    console.log(JSON.stringify({done:command,calls,submissions,publications}));
  }
} finally {
  await participant.stop();await admin.stop();server.closeAllConnections();
  await new Promise(resolve=>server.close(resolve));await view.close();await store.close();
  await fs.rm(root,{recursive:true,force:true});
}

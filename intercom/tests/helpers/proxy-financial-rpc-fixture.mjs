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
const f=await proxyReceiptFixture(process.argv[2]??'tnk',process.argv[3]??'llm');
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
participant.requestService=async(service,request)=>{
  calls++;
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
    if(command==='final') {await f.apply(await f.finalize(await f.receipt()));await sync();}
    else if(command==='close') {await f.apply(await prepareProxyClose(f.ledger,await closure(f),f.context,f.peer.wallet.verify));await sync();}
    else if(command==='foreign')participant.peer.wallet=wallet(f.buyer); // Expected local actor/transport remains provider.
    else if(command==='reset')mutation=null;
    else if(['nonce','network','hold','signature','unknown'].includes(command))mutation=command;
    else throw new Error('unknown fixture command');
    console.log(JSON.stringify({done:command,calls}));
  }
} finally {
  await participant.stop();await admin.stop();server.closeAllConnections();
  await new Promise(resolve=>server.close(resolve));await view.close();await store.close();
  await fs.rm(root,{recursive:true,force:true});
}

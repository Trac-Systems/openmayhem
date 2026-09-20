import assert from 'node:assert/strict';
import test from 'node:test';
import fs from 'node:fs';import os from 'node:os';import path from 'node:path';
import Autobase from 'autobase';import Corestore from 'corestore';import Hyperbee from 'hyperbee';
import b4a from 'b4a';
import PeerWallet from 'trac-wallet';
import MayhemProtocol from '../contract/protocol.js';
import {TxOperation,TxCheck} from 'trac-peer/src/operations/tx/index.js';
import {safeEncodeApplyOperation} from 'trac-msb/src/utils/protobuf/operationHelpers.js';
import {FEE} from 'trac-msb/src/core/state/utils/transaction.js';
import Wallet from 'trac-peer/src/wallet.js';
import { FeatureOperation, FeatureCheck } from 'trac-peer/src/operations/feature/index.js';
import { canonicalReplayContext, consumeCanonicalReplayContext, canonicalReplayView } from 'trac-peer/src/base/canonical-replay.js';
import {adminWriterDiagnostics,appliedViewProof} from '../src/rpc.js';
import MayhemContract from '../contract/contract.js';
import Contract23 from '../contract/history/v23.js';import Contract24 from '../contract/history/v24.js';import Contract25 from '../contract/history/v25.js';import Contract26 from '../contract/history/v26.js';
import { MemoryStorage } from './helpers/contract.js';
const delay=ms=>new Promise(r=>setTimeout(r,ms));
const protocol={featMaxBytes:()=>1e6};
const handler=(wallet,contract,canonicalView)=>new FeatureOperation(new FeatureCheck(),{wallet,protocolInstance:protocol,contractInstance:contract,canonicalView});
async function collect(view){const values={};for await(const row of view.createReadStream())values[row.key]=row.value;return new MemoryStorage(values);}
async function fixture(version,kind='feature'){
 const dir=fs.mkdtempSync(path.join(os.tmpdir(),'mayhem-canonical-replay-'));const wallet=new Wallet();await wallet.ready;await wallet.generateKeyPair();
 const toAddress=key=>PeerWallet.encodeBech32mSafe('trac',b4a.from(key,'hex'));
 const config={bootstrap:'43'.repeat(32),maxMsbSignedLength:1e9,maxMsbSignedLengthFutureDelta:100000,maxMsbApplyOperationBytes:4096};let msbEntry;
 const msbClient={networkId:918,bootstrapHex:'44'.repeat(32),getTxvHex:async()=> '45'.repeat(32),getFee:()=>FEE,pubKeyHexToAddress:toAddress,addressToPubKeyHex:a=>b4a.toString(PeerWallet.decodeBech32mSafe(a),'hex'),getSignedLength:()=>100,waitForSignedLengthAtLeast:async()=>{},getSignedAtLength:async()=>msbEntry};
 const peer={wallet,writerLocalKey:'42'.repeat(32),config,msbClient};const proto=new MayhemProtocol(peer,{},config);
 const Implementation=version===23?Contract23:version===24?Contract24:version===25?Contract25:Contract26;const old=new Implementation(proto,{});let store,base,lastNode;
 const applyHandler=(contract,view)=>kind==='feature'?handler(wallet,contract,view):new TxOperation(new TxCheck(),{wallet,protocolInstance:proto,contractInstance:contract,canonicalView:view,msbClient,config});
 async function open(){store=new Corestore(dir);base=new Autobase(store,null,{ackInterval:0,valueEncoding:'json',open:s=>new Hyperbee(s.get('view'),{extension:false,keyEncoding:'utf-8',valueEncoding:'json'}),async apply(nodes,view){const batch=view.batch();for(const node of nodes){if(node.value?.type==='seed'){await batch.put('admin',wallet.publicKey);continue;}if(node.value){lastNode=node;await applyHandler(old,view).handle(node.value,batch,base,node);}}await batch.flush();await batch.close();}});await base.ready();peer.base=base;}
 await open();await base.append({type:'seed'});await base.update();
 let op;
 if(kind==='feature'){
  const value={op:'rate_oracle',tnk_usd_au:'50000000000000000',source:'gate-spot',ts:3600},key='mayhem_'+await old.rateFeatureKey(value),nonce='history-'+version;
  op={type:'feature',key,value:{dispatch:{type:'mayhem_feature',contract_version:version,key,address:wallet.publicKey,value,nonce,hash:wallet.sign(JSON.stringify(value)+nonce)}}};
 }else{
  const original=proto.versionedTransactionObject;proto.versionedTransactionObject=d=>({...d,value:{...d.value,contract_version:version}});
  const paid=await proto.preparePaidTransaction({type:'setParams',value:{op:'set_params',submitted_at:0,effective_at:86400,values:{price_min_bps:version>=25?2500:1}}});proto.versionedTransactionObject=original;
  const txo=Object.fromEntries(Object.entries(paid.payload.txo).map(([k,v])=>[k,b4a.from(v,'hex')]));txo.va=b4a.from(toAddress(wallet.publicKey));msbEntry={value:safeEncodeApplyOperation({type:12,address:b4a.from(paid.payload.address),txo})};
  op={type:'tx',key:paid.surrogate.tx,value:{dispatch:paid.dispatch,ipk:wallet.publicKey,wp:wallet.publicKey,msbsl:100}};
 }
 await base.append(op);await base.update();
 for(let i=0;i<100&&base.view.core.signedLength<base.view.core.length;i++){await base.update();await delay(10);}
 const nodeLength=lastNode.length;if(kind==='feature')assert((await base.view.get('fr/'+op.value.dispatch.hash)).value.ok);else assert.equal((await base.view.get('txi/0')).value.err,null);const expected=await collect(base.view);const expectedHash=(await base.view.core.treeHash()).toString('hex');assert.equal((await appliedViewProof({base},base.view.core.signedLength)).tree_hash,expectedHash);const diagnostics=adminWriterDiagnostics({base});assert(diagnostics.apply_state.views.find(v=>v.name==='view').core_length>=base.view.core.signedLength,JSON.stringify({signed:base.view.core.signedLength,diagnostics}));
 await base.close();await store.close();await open();
 const node={value:op,from:base.local,length:nodeLength,indexed:true,optimistic:false};
 async function replay(){const replayStore=new Corestore(path.join(dir,'replay'));const view=new Hyperbee(replayStore.get({name:'view'}),{extension:false,keyEncoding:'utf-8',valueEncoding:'json'});await view.ready();await view.put('admin',wallet.publicKey);const batch=view.batch();const current=new MayhemContract(proto,{});await applyHandler(current,base.view).handle(op,batch,base,node);await batch.flush();await batch.close();assert.equal((await view.core.treeHash()).toString('hex'),expectedHash,'Full authenticated tree must match historical writer');const once=view.core.length;const duplicate=view.batch();await applyHandler(current,base.view).handle(op,duplicate,base,node);await duplicate.flush();await duplicate.close();assert.equal(view.core.length,once);const state=await collect(view);await view.close();await replayStore.close();return state;}
 return {wallet,op,node,proto,replay,get view(){return base.view;},expected,before:()=>new MemoryStorage({admin:wallet.publicKey}),async close(){await base.close();await store.close();fs.rmSync(dir,{recursive:true,force:true});}};
}
for(const version of [23,24,25,26])for(const kind of ['feature','tx'])test(`persisted signed v${version} ${kind} history preserves full tree under27 exactly once`,async()=>{
 const f=await fixture(version,kind);try{const replayed=await f.replay();assert.equal(replayed.snapshotBytes(),f.expected.snapshotBytes());if(kind==='tx')assert.equal((await replayed.get('params/price_min_bps')).value.pending.value,version>=25?2500:1,'Historical pricing bound must not be reinterpreted');}finally{await f.close();}
});
test('fresh, unbound, mutated and wrong-version historical Feature calls remain rejected',async()=>{
 const f=await fixture(24);try{const c=new MayhemContract({},{});
 await assert.rejects(c.execute(f.op,f.before()),/expected CONTRACT_VERSION 27, got 24/);
 await assert.rejects(c.execute(f.op,f.before(),{replay:true}),/expected CONTRACT_VERSION 27, got 24/);
 const fresh=structuredClone(f.op);fresh.value.dispatch.nonce='fresh';fresh.value.dispatch.hash=f.wallet.sign(JSON.stringify(fresh.value.dispatch.value)+'fresh');
 const unsignedNode={...f.node,value:fresh};await assert.rejects(handler(f.wallet,c,f.view).handle(fresh,f.before(),{},unsignedNode),/expected CONTRACT_VERSION 27, got 24/);
 const bad=structuredClone(f.op);bad.value.dispatch.contract_version=23;await assert.rejects(handler(f.wallet,c,f.view).handle(bad,f.before(),{},{...f.node,value:bad}),/expected CONTRACT_VERSION 27, got 23/);
 const storage=f.before(),token=await canonicalReplayContext(f.op,storage,f.node,f.view);assert(token);assert.equal(consumeCanonicalReplayContext(token,f.op,f.before()),false);assert.equal(consumeCanonicalReplayContext(token,f.op,storage),false,'failed use consumes capability');
 const token2=await canonicalReplayContext(f.op,storage,f.node,f.view);f.op.value.dispatch.key+='tampered';assert.equal(consumeCanonicalReplayContext(token2,f.op,storage),false);
 }finally{await f.close();}
});

test('ordinary historical TX requires exact canonical index and original signed bytes',async()=>{
 const f=await fixture(24,'tx');try{const c=new MayhemContract(f.proto,{}),storage=f.before();await assert.rejects(c.execute(f.op,storage),/expected CONTRACT_VERSION 27, got 24/);
 const noIndex={core:f.view.core,checkout:length=>{const view=f.view.checkout(length);return {close:()=>view.close(),get:async key=>key.startsWith('tx/')?null:view.get(key)};}};
 assert.equal(await canonicalReplayContext(f.op,storage,f.node,noIndex),null);
 const badRecord={core:f.view.core,checkout:length=>{const view=f.view.checkout(length);return {close:()=>view.close(),get:async key=>{const row=await view.get(key);return key.startsWith('txi/')?{...row,value:{...row.value,ipk:'00'.repeat(32)}}:row;}};}};
 assert.equal(await canonicalReplayContext(f.op,storage,f.node,badRecord),null);
 const changed=structuredClone(f.op);changed.value.dispatch.value.values.price_min_bps=2;assert.equal(await canonicalReplayContext(changed,storage,{...f.node,value:changed},f.view),null);
 assert.equal(await canonicalReplayContext(f.op,storage,{...f.node,optimistic:true},f.view),null);
 }finally{await f.close();}
});

for (const version of [23, 24, 25, 26]) test(`persisted remote v${version} Feature reads signed default view ahead of apply batch`, async () => {
 const dir=fs.mkdtempSync(path.join(os.tmpdir(),'mayhem-remote-history-'));
 const wallet=new Wallet();await wallet.ready;await wallet.generateKeyPair();
 const old=new (version===23?Contract23:version===24?Contract24:version===25?Contract25:Contract26)(protocol,{});
 const opened=new Set(), streams=[];
 async function open(name,key,contract,broken=false){
  const store=new Corestore(path.join(dir,name));let base;
  const state={store,contract,error:null,observations:[]};
  base=new Autobase(store,key,{ackInterval:0,fastForward:false,valueEncoding:'json',
   open:s=>new Hyperbee(s.get('view'),{extension:false,keyEncoding:'utf-8',valueEncoding:'json'}),
   async apply(nodes,view){
    const batch=view.batch(),canonical=broken?base.view:canonicalReplayView(base);
    try{
     if(canonical)await canonical.ready();
     for(const node of nodes){
      if(node.value?.type==='seed'){await batch.put('admin',wallet.publicKey);continue;}
      if(node.value?.type!=='feature')continue;
      state.observations.push({remote:!b4a.equals(node.from.key,base.local.key),nodeLength:node.length,inputSignedLength:node.from.signedLength,publicLength:base.view.core.signedLength,canonicalLength:canonical.core.signedLength,applyLength:view.core.length});
      await handler(wallet,contract,canonical).handle(node.value,batch,base,node);
     }
     await batch.flush();
    }finally{await batch.close();if(canonical&&!broken)await canonical.close();}
   }});
  state.base=base;base.on('error',e=>{state.error=e;});await base.ready();opened.add(state);return state;
 }
 async function close(state){await state.base.close();await state.store.close();opened.delete(state);}
 function connect(left,right){const a=left.store.replicate(true),b=right.store.replicate(false);a.pipe(b).pipe(a);const close=()=>{a.destroy();b.destroy();};streams.push(close);return close;}
 async function until(check){for(let i=0;i<250;i++){if(await check())return;await delay(10);}assert.fail('Remote replay did not reach its expected terminal state');}
 try{
  const writer=await open('writer',null,old);await writer.base.append({type:'seed'});await writer.base.update();
  // Both followers persist the earlier prefix, then go offline before the old
  // writer accepts the Feature. Reopening them invokes real remote Autobase
  // apply nodes; no synthetic node.from=base.local or borrowed writer view.
  for(const name of ['broken','fixed']){
   const follower=await open(name,writer.base.key,old);const disconnect=connect(writer,follower);
   await until(async()=>!!await follower.base.view.get('admin'));disconnect();await close(follower);
  }
  const value={op:'rate_oracle',tnk_usd_au:'50000000000000000',source:'gate-spot',ts:3600},key='mayhem_'+await old.rateFeatureKey(value),nonce='remote-'+version;
  const op={type:'feature',key,value:{dispatch:{type:'mayhem_feature',contract_version:version,key,address:wallet.publicKey,value,nonce,hash:wallet.sign(JSON.stringify(value)+nonce)}}};
  await writer.base.append(op);await writer.base.update();
  const expected=(await writer.base.view.core.treeHash()).toString('hex');
  const broken=await open('broken',writer.base.key,new MayhemContract(protocol,{}),true);const disconnectBroken=connect(writer,broken);
  await until(()=>broken.error);assert.match(broken.error.message,new RegExp('expected CONTRACT_VERSION 27, got '+version));
  assert(broken.observations.some(x=>x.remote&&x.inputSignedLength>=x.nodeLength&&x.publicLength===x.applyLength));
  disconnectBroken();await close(broken);
  const current=new MayhemContract(protocol,{}),fixed=await open('fixed',writer.base.key,current);connect(writer,fixed);
  await until(async()=>fixed.error||!!await fixed.base.view.get('fr/'+op.value.dispatch.hash));assert.equal(fixed.error,null);
  assert(fixed.observations.some(x=>x.remote&&x.inputSignedLength>=x.nodeLength&&x.canonicalLength>x.publicLength&&x.publicLength===x.applyLength));
  assert.equal((await fixed.base.view.core.treeHash()).toString('hex'),expected,'Real follower must reproduce the complete historical signed tree');
  assert.equal((await collect(fixed.base.view)).snapshotBytes(),(await collect(writer.base.view)).snapshotBytes());
  assert.equal(current._mayhemReplayStatus.completed,1);
  await writer.base.append(op);await writer.base.update();await delay(50);await fixed.base.update();
  assert.equal(current._mayhemReplayStatus.completed,1,'Repeated signed input must not apply twice');
  assert.equal((await fixed.base.view.core.treeHash()).toString('hex'),expected);
 }finally{for(const close of streams)close();for(const state of opened)await close(state);fs.rmSync(dir,{recursive:true,force:true});}
});

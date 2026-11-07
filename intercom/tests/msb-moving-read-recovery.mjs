import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { pathToFileURL, fileURLToPath } from 'node:url';
import { mkdtemp, rm } from 'node:fs/promises';
import { EventEmitter } from 'node:events';
import os from 'node:os';
import path from 'node:path';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const localRequire=createRequire(root+'/package.json');
const req=createRequire(localRequire.resolve('trac-msb/package.json'));
const Autobase=req('autobase'), Corestore=req('corestore'), Hyperbee=req('hyperbee');
const {MsbClient}=await import(pathToFileURL(root+'/trac/trac-peer/src/msbClient.js'));
const recoveryPath=process.env.RECOVERY_SOURCE || root+'/trac/msb/src/core/network/services/ReadRecovery.js';
const {createReadRecovery}=await import(pathToFileURL(recoveryPath));
const dir=await mkdtemp(path.join(os.tmpdir(),'msb-moving-recovery-')); const nodes=[],links=[],logs=[];
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
const until=async(fn,label,limit=10000)=>{const start=Date.now();while(Date.now()-start<limit){if(await fn())return;await sleep(20)}throw Error(label)};
async function open(name,key){const store=new Corestore(path.join(dir,name));const base=new Autobase(store,key,{valueEncoding:'json',ackInterval:0,
 open:s=>new Hyperbee(s.get('view'),{extension:false,keyEncoding:'utf-8',valueEncoding:'json'}),
 apply:async(entries,view)=>{const batch=view.batch();for(const {value} of entries)await batch.put(value.key,value.value);await batch.flush();await batch.close()}});
 await base.ready();const n={store,base,name};nodes.push(n);return n;}
function link(a,b){const x=a.store.replicate(true),y=b.store.replicate(false);x.on('error',()=>{});y.on('error',()=>{});x.pipe(y).pipe(x);links.push(x,y);return {x,y,close:()=>{x.destroy();y.destroy()}};}
let client,recover,blockedRequests=0,swarm,resolved=0,records=20;
try{
 const a=await open('writer'),b=await open('follower',a.base.key),c=await open('stale-replica',a.base.key),d=await open('good-replica',a.base.key);
 let ab=link(b,a),ac=link(c,a),ad=link(d,a);
 await a.base.append(Array.from({length:20},(_,i)=>({key:`tx-${i}`,value:i})));
 await until(()=>[b,c,d].every(n=>n.base.view.core.signedLength===a.base.view.core.signedLength),'initial catchup');
 ab.close();ac.close();await sleep(50); const bc=link(b,c);const old=b.base.view.core.signedLength;
 await a.base.append(Array.from({length:20},(_,i)=>({key:`tx-${i+20}`,value:i+20})));records+=20;
 await until(()=>d.base.view.core.signedLength===a.base.view.core.signedLength,'alternate has records');
 const firstTarget=a.base.view.core.signedLength;
 const goodKey=ad.y.noiseStream.remotePublicKey.toString('hex');
 const hooked=new WeakSet();const hook=p=>{if(hooked.has(p))return;hooked.add(p);const original=p.onrequest;p.onrequest=function(m){if(p.remotePublicKey?.toString('hex')!==goodKey && m.block){blockedRequests++;if(m.upgrade)return original.call(this,{...m,block:null});return;}return original.call(this,m)}};
 for(const core of [a.base.core,a.base.local,a.base.view.core]){for(const p of core.peers)hook(p);core.on('peer-add',hook)}
 ab=link(b,a);await until(()=>!!ab.y.noiseStream.remotePublicKey,'bad link handshake');
 const sockets=new Set();
 const wrap=l=>{const socket=Object.assign(new EventEmitter(),{remotePublicKey:l.x.noiseStream.remotePublicKey,destroyed:false,destroy(){this.destroyed=true;l.close();sockets.delete(this);this.emit('close')}});sockets.add(socket);return socket};
 await until(()=>!!bc.x.noiseStream.remotePublicKey,'stale link handshake');wrap(ab);wrap(bc);
 let repairs=0;const alternate={publicKey:Buffer.alloc(32,207),proven:true};
 swarm={connections:sockets,peers:new Map([['alternate',alternate]]),maxPeers:2,maxClientConnections:2,connecting:0,
 joinPeer(){repairs++;link(b,d)},leavePeer(){}};
 recover=createReadRecovery({base:b.base,swarm,topic:Buffer.alloc(32,7),repairMs:100,retireMs:6000,log:s=>logs.push(s),discovery:{refresh:async()=>true}});
 client=new MsbClient({state:{base:b.base},network:{recoverRead:recover},config:{},ready:async()=>{}},{readSliceMs:30,confirmationStallMs:100,log:s=>logs.push(s)});
 await client.ready();
 const pending=Promise.all(Array.from({length:4},()=>client.waitForSignedLengthAtLeast(firstTarget,{pollMs:10}).then(()=>resolved++)));pending.catch(()=>{});
 const failures=[];
 for(let attempt=0;attempt<4 && resolved===0;attempt++){
   await b.base.core.update({wait:true});await b.base.forceFastForward();
   const failure=b.base.fastForwardLastFailure;assert.equal(failure?.code,'REQUEST_TIMEOUT');
   failures.push(structuredClone(failure));console.log(JSON.stringify({attempt,index:failure.index,stage:failure.stage,resolved,blockedRequests,repairs}));
   if(repairs)break;
   assert.equal(resolved,0,'no completion while blocks are withheld');
   await a.base.append(Array.from({length:20},(_,i)=>({key:`tx-${i+records}`,value:i+records})));records+=20;
   await until(()=>d.base.view.core.signedLength===a.base.view.core.signedLength,'alternate tracks moving target');
 }
 await until(()=>resolved===4,'same-process moving-target recovery');await pending;
 await until(()=>b.base.view.core.signedLength===a.base.view.core.signedLength,'all final confirmations');
 assert.ok(new Set(failures.map(f=>f.index)).size>=2,'actual failed targets moved');assert.ok(blockedRequests>0);assert.equal(repairs,1);
 const anchor=a.base.view.core.signedLength;assert.deepEqual(await b.base.view.core.treeHash(anchor),await a.base.view.core.treeHash(anchor));
 for(let i=0;i<records;i++)assert.equal((await b.base.view.get(`tx-${i}`)).value,i);
 const result={old,anchor,records,blockedRequests,repairs,waitersResolved:resolved,failures,matchingStateAndTree:true,sameProcess:true,events:logs.filter(l=>l.includes('replace_stalled')).map(l=>JSON.parse(l.slice(l.indexOf('{'))))};
 console.log(JSON.stringify(result));console.log(JSON.stringify({complete:true,records,repairs,waitersResolved:resolved,matchingStateAndTree:true}));
}finally{recover?.close();await client?.close();for(const s of links)s.destroy();for(const n of nodes){await n.base.close();await n.store.close()}await rm(dir,{recursive:true,force:true})}

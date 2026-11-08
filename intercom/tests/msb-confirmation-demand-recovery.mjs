import assert from 'node:assert/strict';
import {createRequire} from 'node:module';
import {pathToFileURL,fileURLToPath} from 'node:url';
import {mkdtemp,rm} from 'node:fs/promises';
import {EventEmitter} from 'node:events';
import os from 'node:os';
import path from 'node:path';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
const req=createRequire(createRequire(root+'/package.json').resolve('trac-msb/package.json'));
const Autobase=req('autobase'),Corestore=req('corestore'),Hyperbee=req('hyperbee');
const {MsbClient}=await import(pathToFileURL(root+'/trac/trac-peer/src/msbClient.js'));
const {createReadRecovery}=await import(pathToFileURL(process.env.RECOVERY_SOURCE||root+'/trac/msb/src/core/network/services/ReadRecovery.js'));
const dir=await mkdtemp(path.join(os.tmpdir(),'msb-demand-recovery-')),nodes=[],links=[],events=[];
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
const until=async(fn,label)=>{const t=Date.now();while(Date.now()-t<10000){if(await fn())return;await sleep(20)}throw Error(label)};
async function open(name,key){const store=new Corestore(path.join(dir,name));const base=new Autobase(store,key,{valueEncoding:'json',ackInterval:0,open:s=>new Hyperbee(s.get('view'),{extension:false,keyEncoding:'utf-8',valueEncoding:'json'}),apply:async(entries,view)=>{const batch=view.batch();for(const {value}of entries)await batch.put(value.key,value.value);await batch.flush();await batch.close()}});await base.ready();const n={name,store,base};nodes.push(n);return n;}
function link(a,b){const x=a.store.replicate(true),y=b.store.replicate(false);x.on('error',()=>{});y.on('error',()=>{});x.pipe(y).pipe(x);links.push(x,y);return{x,y,close(){x.destroy();y.destroy()}};}
let client,recover,resolved=0,pending;
try{
 const a=await open('writer'),b=await open('follower',a.base.key),c=await open('old-replica',a.base.key),d=await open('good-replica',a.base.key),e=await open('pinned-old-replica',a.base.key);
 const ab=link(b,a),ac=link(c,a),ad=link(d,a),ae=link(e,a);
 await a.base.append(Array.from({length:20},(_,i)=>({key:`tx-${i}`,value:i})));
 await until(()=>[b,c,d,e].every(n=>n.base.view.core.signedLength===a.base.view.core.signedLength),'initial catchup');
 ab.close();ac.close();ae.close();await sleep(60);
 const bc=link(b,c),be=link(b,e);await until(()=>bc.x.noiseStream.remotePublicKey&&be.x.noiseStream.remotePublicKey,'old replica handshakes');
 await a.base.append(Array.from({length:20},(_,i)=>({key:`tx-${i+20}`,value:i+20})));
 await until(()=>d.base.view.core.signedLength===a.base.view.core.signedLength,'alternate confirmed data');
 const target=a.base.view.core.signedLength,old=b.base.view.core.signedLength;assert.ok(target>old);
 assert.equal(b.base.fastForwardLastFailure,undefined,'no timed-out block read');
 const sockets=new Set(),peers=new Map();let replacements=0,dials=0;
 function wrap(l,pinned=false){const key=l.x.noiseStream.remotePublicKey;const s=Object.assign(new EventEmitter(),{remotePublicKey:key,destroyed:false,destroy(){this.destroyed=true;l.close();sockets.delete(this);replacements++;this.emit('close')}});sockets.add(s);peers.set(key.toString('hex'),{publicKey:key,proven:true,explicit:pinned});return s;}
 const oldSocket=wrap(bc),pinnedSocket=wrap(be,true),alternate={publicKey:Buffer.alloc(32,207),proven:true};peers.set(alternate.publicKey.toString('hex'),alternate);
 const swarm={connections:sockets,peers,maxPeers:1,maxClientConnections:Infinity,maxParallel:3,connecting:0,joinPeer(){dials++;link(b,d)},leavePeer(){}};
 recover=createReadRecovery({base:b.base,swarm,topic:Buffer.alloc(32,7),repairMs:30,retireMs:200,log:s=>events.push(JSON.parse(s.slice(s.indexOf('{')))),discovery:{refresh:async()=>true}});
 client=new MsbClient({state:{base:b.base},network:{recoverRead:recover},config:{},ready:async()=>{}},{readSliceMs:20,confirmationStallMs:40,log:()=>{}});await client.ready();
 pending=Promise.all(Array.from({length:4},()=>client.waitForSignedLengthAtLeast(target,{pollMs:10}).then(()=>resolved++)));pending.catch(()=>{});
 await sleep(150);assert.equal(resolved,0,'waiters remain blocked without authenticated evidence');assert.equal(replacements,0,'local observation interval is respected');
 await until(()=>resolved===4,'same-process no-timeout confirmation recovery');await pending;
 await until(()=>b.base.view.core.signedLength===target,'final signed view');
 assert.equal(replacements,1);assert.equal(dials,1);assert.equal(oldSocket.destroyed,true);assert.equal(pinnedSocket.destroyed,false);
 assert.equal(b.base.fastForwardLastFailure,undefined,'recovery did not depend on a block timeout');
 assert.deepEqual(await b.base.view.core.treeHash(target),await a.base.view.core.treeHash(target));
 for(let i=0;i<40;i++)assert.equal((await b.base.view.get(`tx-${i}`)).value,i);
 console.log(JSON.stringify({complete:true,old,target,records:40,waitersResolved:resolved,replacements,dials,ordinaryPeerCap:1,initialConnections:2,pinnedPreserved:true,noFailedRead:true,sameProcess:true,matchingStateAndTree:true,events:events.filter(e=>e.event!=='msb_replication_recovery')}));
}finally{recover?.close();await client?.close();for(const s of links)s.destroy();for(const n of nodes){await n.base.close();await n.store.close()}await pending?.catch(()=>{});await rm(dir,{recursive:true,force:true})}

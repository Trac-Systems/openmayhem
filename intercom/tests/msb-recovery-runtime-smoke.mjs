const assert={equal(a,b){if(a!==b)throw Error('assertion failed: '+a+' !== '+b)},ok(v){if(!v)throw Error('assertion failed')}};
import b4a from 'b4a';
import {EventEmitter} from 'events';
import {createReadRecovery} from '../trac/msb/src/core/network/services/ReadRecovery.js';
import {ConfirmationWatch} from '../trac/msb/src/core/state/ConfirmationWatch.js';
import {MsbClient} from '../trac/trac-peer/src/msbClient.js';
let now=0,refreshes=0,replaced=0,dials=0,resolved=0;
const core=Object.assign(new EventEmitter(),{signedLength:10,length:10,peers:[]});
const base={view:{core},core:{peers:[]}};
const sockets=new Set(),peers=new Map();
for(const n of [1,2,3]){const publicKey=b4a.alloc(32,n);peers.set(b4a.toString(publicKey,'hex'),{publicKey,proven:true,explicit:n===2});if(n===3)continue;const s=Object.assign(new EventEmitter(),{remotePublicKey:publicKey,destroyed:false,destroy(){this.destroyed=true;replaced++;sockets.delete(this);this.emit('close')}});sockets.add(s);core.peers.push(s)}
const swarm={connections:sockets,peers,maxPeers:1,maxClientConnections:1,maxParallel:1,connecting:0,joinPeer(){dials++;core.signedLength=20;core.length=20;core.emit('append')},leavePeer(){}};
const recovery=createReadRecovery({base,swarm,topic:b4a.alloc(32,9),repairMs:30,retireMs:120,now:()=>now,log:()=>{},discovery:{refresh:async()=>{refreshes++;return true}}});
const client=new MsbClient({state:{base},network:{recoverRead:recovery},config:{},ready:async()=>{}},{readSliceMs:10,confirmationStallMs:20,now:()=>now,log:()=>{}});await client.ready();
const waits=Promise.all(Array.from({length:4},()=>client.waitForSignedLengthAtLeast(20,{pollMs:10}).then(()=>resolved++)));
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
now=30;await sleep(30);assert.equal(resolved,0);assert.equal(replaced,0);
now=200;await sleep(40);await waits;assert.equal(resolved,4);assert.equal(replaced,1);assert.equal(dials,1);assert.equal([...sockets][0].remotePublicKey[0],2);assert.ok(refreshes<=2);
let recoveryEvents=0;const wbase={view:{core:{signedLength:1,length:2}},isFastForwarding:()=>true};const watch=new ConfirmationWatch(wbase,{timeout:20,now:()=>now,log:()=>{},onStall(){recoveryEvents++}});await watch.check();now=240;await watch.check();assert.equal(recoveryEvents,1);watch.stop();recovery.close();await client.close();
console.log(JSON.stringify({complete:true,portableRecovery:true,waiters:resolved,replaced,dials,pinnedPreserved:true,watchRecovery:true}));
if(globalThis.Bare){Bare.emit('beforeExit',0);Bare.exit(Bare.exitCode??0)}

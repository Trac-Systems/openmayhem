import test from 'brittle';
import b4a from 'b4a';
import { EventEmitter } from 'events';
import { createReadRecovery } from '../../../src/core/network/services/ReadRecovery.js';
const turn = () => new Promise(r => setImmediate(r));
const key = n => b4a.alloc(32, n);
function fixture({ refresh = async () => false, limit = 2 } = {}) {
    let time = 0, refreshes = 0;
    const calls = [], logs = [], base = { view: { core: { signedLength: 10, length: 10, peers: [] } } };
    const swarm = { connections: new Set(), peers: new Map(), connecting: 0, maxPeers: limit, maxClientConnections: limit,
        joinPeer(k) { calls.push(b4a.toString(k, 'hex')); }, leavePeer() {} };
    const recover = createReadRecovery({ base, swarm, topic: key(9), repairMs: 10, retireMs: 40,
        now: () => time, log: x => logs.push(x), discovery: { refresh() { refreshes++; return refresh(); } } });
    const peer = n => { const p = { publicKey: key(n), proven: true }; swarm.peers.set(key(n).toString('hex'), p); return p; };
    const connect = n => {
        const c = Object.assign(new EventEmitter(), { remotePublicKey: key(n), destroyed: false,
            destroy() { this.destroyed = true; swarm.connections.delete(this); this.emit('close'); } });
        swarm.connections.add(c); base.view.core.peers.push({remotePublicKey: key(n)}); return c;
    };
    const demand = target => recover({ code: 'MSB_CONFIRMATION_STALLED', signedLength: base.view.core.signedLength,
        msbsl: target ?? base.view.core.signedLength + 1, stalledMs: 999999 });
    return { base, swarm, recover, peer, connect, demand, calls, logs, refreshes: () => refreshes, tick: (ms = 10) => { time += ms; } };
}
test('shared recovery bounds bursts and discovery and preserves pins and bans', async t => {
    const f = fixture({limit:3,refresh:()=>new Promise(()=>{})});f.peer(1).banned=true;f.peer(2).explicit=true;f.peer(3);
    for(let i=0;i<100;i++)f.demand();await turn();t.is(f.calls.length,1);t.is(f.refreshes(),1);
    f.tick();f.demand();await turn();t.is(f.calls.length,2);t.is(f.refreshes(),1);
    t.ok(f.calls.every(x=>x===key(3).toString('hex')));f.recover.close();f.tick();t.is(f.demand(),false);
});
test('idle connected nodes never retire; caller elapsed cannot bypass local observation', async t => {
    const f=fixture();f.peer(3);const a=f.connect(1),b=f.connect(2);f.recover();f.tick(100);f.recover();
    t.is(a.destroyed,false);f.demand();f.tick(20);f.demand();t.is(a.destroyed,false);
    f.tick(20);f.demand();await turn();t.is(a.destroyed,true);t.is(b.destroyed,false);t.is(f.calls.length,1);
    t.is(f.base.view.core.signedLength,10,'never manufactures confirmation');
});
test('confirmation progress, expired demand and invalid targets cannot retire', async t => {
    for(const mode of ['progress','expired','wrong-height','reached','not-participant','new-socket','pin']){
        const f=fixture();f.peer(3);let a=f.connect(1);f.connect(2);f.peer(2).explicit=true;
        const e={code:'MSB_CONFIRMATION_STALLED',signedLength:10,msbsl:11};
        if(mode==='wrong-height')e.signedLength=9;if(mode==='reached')e.msbsl=10;
        if(mode==='not-participant')f.base.view.core.peers=[];if(mode==='pin')f.peer(1).explicit=true;
        for(let i=0;i<100;i++)f.recover(e);f.tick(mode==='expired'?100:40);
        if(mode==='progress')f.base.view.core.signedLength=11;
        if(mode==='new-socket'){a.destroy();a=f.connect(1);}
        f.recover(mode==='expired'?undefined:e);await turn();t.is(a.destroyed,false,mode);t.is(f.calls.length,0,mode);
    }
});
test('one connection or no alternate replica prevents retirement', async t => {
    for(const limit of [1,2]){const f=fixture({limit});const a=f.connect(1);if(limit===2)f.connect(2);else f.peer(3);
        f.demand();f.tick(40);f.demand();await turn();t.is(a.destroyed,false);t.is(f.calls.length,0);}
});
test('shutdown cancels pending replacement and late discovery without dialing', async t => {
    let finish;const f=fixture({refresh:()=>new Promise(r=>finish=r)});f.peer(3);const a=f.connect(1);f.connect(2);a.destroy=()=>{};
    f.demand();await turn();f.tick(40);f.demand();t.is(a.listenerCount('close'),1);
    f.recover.close();a.emit('close');finish(false);await turn();t.is(a.listenerCount('close'),0);t.is(f.calls.length,0);
    t.not(f.logs.some(x=>x.includes('msb_discovery_failed')),true);
});
test('connection and parallel limits and suspended swarms are respected', async t => {
    const f=fixture({limit:1});f.peer(1);f.swarm.connecting=1;f.demand();t.is(f.calls.length,0);
    f.tick();f.swarm.connecting=0;f.swarm.suspended=true;t.is(f.demand(),false);t.is(f.calls.length,0);
    const g=fixture();g.peer(3);const a=g.connect(1);g.connect(2);g.swarm.maxParallel=1;g.swarm.connecting=1;
    g.demand();g.tick(40);g.demand();await turn();t.is(a.destroyed,false);
});
test('moving requested targets do not reset confirmed nonprogress', async t => {
    const f=fixture();f.peer(3);const a=f.connect(1);f.connect(2);f.demand(11);f.tick(20);f.demand(12);f.tick(20);f.demand(13);
    await turn();t.is(a.destroyed,true);t.is(f.calls.length,1);t.is(f.base.view.core.signedLength,10);
});
test('large peer cache has bounded scans and connection replacement stays rate limited', async t => {
    const f=fixture();let visited=0;f.swarm.peers=new Map(Array.from({length:10000},(_,n)=>[n,{get publicKey(){visited++;return key(3)},proven:true,banned:true}]));
    f.connect(1);f.connect(2);f.demand();t.ok(visited<=768);
    const g=fixture();g.peer(3);g.connect(1);const b=g.connect(2);g.demand();g.tick(40);g.demand();await turn();
    g.connect(4);g.tick(10);g.demand();await turn();t.is(b.destroyed,false);t.is(g.calls.length,1);
    g.base.view.core.signedLength++;g.tick(10);g.demand();g.tick(30);g.demand();t.is(b.destroyed,false);
});
test('one-for-one replacement works above the ordinary cap without disturbing pins', async t => {
    const f=fixture();f.peer(4);const a=f.connect(1);const pin=f.connect(2);f.peer(2).explicit=true;f.connect(3);
    f.demand();f.tick(40);f.demand();await turn();t.is(a.destroyed,true);t.is(pin.destroyed,false);t.is(f.calls.length,1);
});
test('a changed slot or explicit pin prevents asynchronous replacement dial', async t => {
    const f=fixture();const replacement=f.peer(4);const a=f.connect(1);f.connect(2);f.connect(3);f.demand();f.tick(40);
    a.destroy=function(){this.destroyed=true;f.swarm.connections.delete(this);f.connect(5);replacement.explicit=true;this.emit('close')};
    f.demand();await turn();t.is(f.calls.length,0);t.is(replacement.explicit,true);
});
test('standalone MSB pending work recovers without a dependency diagnostic', async t => {
    for(const mode of ['forwarding','system-head','unconfirmed']){
        const f=fixture();f.peer(3);const a=f.connect(1);f.connect(2);
        f.base.core={peers:[{remotePublicKey:key(1)}],length:10,core:{state:{length:mode==='system-head'?11:10}}};
        f.base.isFastForwarding=()=>mode==='forwarding';f.base.view.core.length=mode==='unconfirmed'?11:10;
        f.recover();f.tick(40);f.recover();await turn();t.is(a.destroyed,true,mode);t.is(f.calls.length,1);
    }
});

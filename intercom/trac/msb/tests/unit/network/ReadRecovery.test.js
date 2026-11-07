import test from 'brittle';
import b4a from 'b4a';
import { EventEmitter } from 'events';
import { createReadRecovery } from '../../../src/core/network/services/ReadRecovery.js';

const turn = () => new Promise(resolve => setImmediate(resolve));
const key = n => b4a.alloc(32, n);
function fixture({ refresh = async () => false, limit = 3 } = {}) {
    let time = 0, refreshes = 0;
    const calls = [], logs = [], base = { view: { core: { signedLength: 10 } } };
    const swarm = { connections: new Set(), peers: new Map(), connecting: 0,
        maxPeers: limit, maxClientConnections: limit,
        joinPeer(k) { calls.push(b4a.toString(k, 'hex')); }, leavePeer() {} };
    const recover = createReadRecovery({ base, swarm, topic: key(9), repairMs: 10, retireMs: 40,
        now: () => time, log: x => logs.push(x), discovery: { refresh() { refreshes++; return refresh(); } } });
    const peer = n => { const p = { publicKey: key(n), proven: true }; swarm.peers.set(key(n).toString('hex'), p); return p; };
    const connect = n => {
        const c = Object.assign(new EventEmitter(), { remotePublicKey: key(n), destroyed: false,
            destroy() { this.destroyed = true; swarm.connections.delete(this); this.emit('close'); } });
        swarm.connections.add(c); return c;
    };
    const fail = (n, rx = 0, index = 9) => { base.fastForwardLastFailure = { at: time, code: 'REQUEST_TIMEOUT',
        coreKey: key(8).toString('hex'), index, peers: [{ remoteKey: key(n).toString('hex'), dataRx: rx }] }; };
    return { base, swarm, recover, peer, connect, fail, calls, logs, refreshes: () => refreshes,
        tick: (ms = 10) => { time += ms; } };
}

test('shared recovery coalesces bursts, preserves pins/bans and bounds hung discovery', async t => {
    const f = fixture({ refresh: () => new Promise(() => {}) });
    f.peer(1).banned = true; f.peer(2).explicit = true; f.peer(3);
    for (let i = 0; i < 100; i++) f.recover();
    await turn(); t.is(f.calls.length, 1); t.is(f.refreshes(), 1);
    f.tick(); f.recover(); await turn();
    t.is(f.calls.length, 2); t.is(f.refreshes(), 1, 'one outstanding refresh');
    t.ok(f.calls.every(x => x === key(3).toString('hex')));
    f.recover.close(); f.tick(); t.is(f.recover(), false);
});

test('full connections alone cannot cause replacement; repeated failed block can replace one', async t => {
    const f = fixture({ limit: 2 }); f.peer(3); const c = f.connect(1); const other = f.connect(2);
    f.recover(); f.tick(100); f.recover(); t.is(c.destroyed, false);
    f.tick(); f.fail(1); f.recover(); f.tick(20); f.fail(1); f.recover();
    t.is(c.destroyed, false, 'not sustained long enough');
    f.tick(20); f.fail(1); f.recover(); await turn();
    t.is(c.destroyed, true); t.is(other.destroyed, false);
    t.is(f.calls.length, 1); t.is(f.calls[0], key(3).toString('hex'));
    t.is(c.listenerCount('close'), 0);
});

test('confirmation progress and stale or single errors cannot retire', async t => {
    for (const mode of ['confirmation', 'single', 'stale']) {
        const f = fixture({ limit: 2 }); f.peer(3); const c = f.connect(1); f.connect(2);
        f.fail(1); f.recover(); f.tick(40);
        if (mode === 'confirmation') { f.base.view.core.signedLength++; f.fail(1); }
        if (mode === 'stale') f.tick(21);
        f.recover(); t.is(c.destroyed, false, mode);
    }
});

test('no alternate replica or single remaining connection prevents retirement', async t => {
    for (const limit of [1, 2]) {
        const f = fixture({ limit }); const c = f.connect(1);
        if (limit === 2) f.connect(2); else f.peer(3);
        f.fail(1); f.recover(); f.tick(100); f.fail(1); f.recover();
        t.is(c.destroyed, false); t.is(f.calls.length, 0);
    }
});

test('shutdown cancels pending replacement and late discovery; no new dial', async t => {
    let finish; const f = fixture({ limit: 2, refresh: () => new Promise(r => { finish = r; }) });
    f.peer(3); const c = f.connect(1); f.connect(2); c.destroy = () => {};
    f.fail(1); f.recover(); await turn(); f.tick(40); f.fail(1); f.recover();
    t.is(c.listenerCount('close'), 1);
    f.recover.close(); c.emit('close'); finish(false); await turn();
    t.is(c.listenerCount('close'), 0); t.is(f.calls.length, 0);
    t.not(f.logs.some(x => x.includes('msb_discovery_failed')), true);
});

test('connecting slots and suspended swarms are respected', t => {
    const f = fixture({ limit: 1 }); f.peer(1); f.swarm.connecting = 1;
    f.recover(); t.is(f.calls.length, 0);
    f.tick(); f.swarm.connecting = 0; f.swarm.suspended = true;
    t.is(f.recover(), false); t.is(f.calls.length, 0);
});


test('moving tips and peer samples recover without treating unrelated data as confirmation', async t => {
    const f = fixture({ limit: 2 }); f.peer(3); const a = f.connect(1), b = f.connect(2);
    f.fail(1, 0, 9); f.recover();
    f.tick(20); f.fail(2, 5, 10); f.recover();
    f.tick(21); f.fail(1, 12, 11); f.recover(); await turn();
    t.is(a.destroyed, true); t.is(b.destroyed, false); t.is(f.calls.length, 1);
    t.is(f.base.view.core.signedLength, 10, 'never fabricates confirmation');
});

test('observation jitter is tolerated only with repeated fresh failures', async t => {
    const f = fixture({ limit: 2 }); f.peer(3); const a = f.connect(1); f.connect(2);
    f.fail(1); f.tick(1); f.recover();
    f.tick(20); f.fail(1, 0, 10); f.recover();
    f.tick(21); f.recover(); await turn();
    t.is(a.destroyed, true, 'two failures remain fresh despite initial polling delay');
});

test('new connections and explicit pins are never selected as stalled sockets', async t => {
    for (const mode of ['new', 'pinned']) {
        const f = fixture({ limit: 2 }); f.peer(3); let a = f.connect(1); f.connect(2);
        if (mode === 'pinned') f.peer(1).explicit = true;
        f.fail(1); f.recover(); f.tick(40);
        if (mode === 'new') { a.destroy(); a = f.connect(1); }
        f.fail(1, 0, 12); f.recover(); await turn();
        t.is(a.destroyed, false, mode); t.is(f.calls.length, 0, mode);
    }
});

test('large peer cache is scanned in bounded windows and retirement stays rate limited', async t => {
    const f = fixture({ limit: 2 });
    let visited = 0;
    f.swarm.peers = new Map(Array.from({length: 10000}, (_, n) => [n, {
        get publicKey() { visited++; return key(3); }, proven: true, banned: true
    }]));
    f.connect(1); f.connect(2); f.fail(1); f.recover();
    t.ok(visited <= 768, 'at most 256 candidates, with bounded field accesses');
    f.swarm.peers = new Map(); f.peer(3);
    // New fixture isolates the retirement counter from the synthetic cache iterator.
    const g = fixture({ limit: 2 }); g.peer(3); g.connect(1); const b = g.connect(2);
    g.fail(1); g.recover(); g.tick(40); g.fail(1); g.recover(); await turn();
    g.connect(4); g.tick(10); g.fail(2); g.recover(); await turn();
    t.is(b.destroyed, false, 'one socket at most per retirement interval');
    g.tick(10); g.base.view.core.signedLength++; g.fail(2); g.recover();
    g.tick(30); g.fail(2); g.recover(); t.is(b.destroyed, false, 'confirmed progress resets evidence');
});

test('live confirmation demand recovers without a fast-forward error above the ordinary connection cap', async t => {
    const f = fixture({ limit: 2 }); f.peer(4); const a = f.connect(1); f.connect(2); f.connect(3);
    f.base.core = { peers: [{remotePublicKey: key(1)}] };
    const event = () => ({code:'MSB_CONFIRMATION_STALLED', signedLength:10, msbsl:11, stalledMs:999999});
    f.recover(event()); await turn(); t.is(a.destroyed,false,'caller elapsed does not bypass local observation');
    f.tick(40); f.recover(event()); await turn();
    t.is(a.destroyed,true); t.is(f.calls.length,1,'one-for-one replacement dials above ordinary cap');
    t.is(f.base.view.core.signedLength,10,'never manufactures confirmation');
});

test('confirmation demand expires, respects pins and progress, and coalesces waiters', async t => {
    for (const mode of ['expired','pin','progress','wrong-height','reached','not-participant','new-socket']) {
        const f=fixture({limit:2});f.peer(3);let a=f.connect(1);f.connect(2);
        f.base.view.core.peers=[{remotePublicKey:key(1)}];
        const e={code:'MSB_CONFIRMATION_STALLED',signedLength:10,msbsl:11,stalledMs:999999};
        if(mode==='pin')f.peer(1).explicit=true;
        if(mode==='wrong-height')e.signedLength=9;
        if(mode==='reached')e.msbsl=10;
        if(mode==='not-participant')f.base.view.core.peers=[];
        for(let i=0;i<100;i++)f.recover(e);
        f.tick(mode==='expired'?100:40);
        if(mode==='progress')f.base.view.core.signedLength=11;
        if(mode==='new-socket'){a.destroy();a=f.connect(1);}
        f.recover(mode==='expired'?undefined:e);await turn();
        t.is(a.destroyed,false,mode);t.is(f.calls.length,0,mode);
    }
});

test('replacement cannot increase the pre-retirement footprint or clear another explicit pin', async t => {
    const f=fixture({limit:2});const replacement=f.peer(4);const a=f.connect(1);f.connect(2);f.connect(3);
    f.base.core={peers:[{remotePublicKey:key(1)}]};
    const e={code:'MSB_CONFIRMATION_STALLED',signedLength:10,msbsl:11};
    f.recover(e);f.tick(40);
    a.destroy=function(){this.destroyed=true;f.swarm.connections.delete(this);f.connect(5);replacement.explicit=true;this.emit('close')};
    f.recover(e);await turn();t.is(f.calls.length,0,'slot or pin changed before asynchronous dial');t.is(replacement.explicit,true);
});

test('standalone MSB fast-forward and known signed system head supply real pending work', async t => {
    for(const mode of ['forwarding','system-head','unconfirmed']){
        const f=fixture({limit:2});f.peer(3);const a=f.connect(1);f.connect(2);
        f.base.core={peers:[{remotePublicKey:key(1)}],length:10,core:{state:{length:mode==='system-head'?11:10}}};
        f.base.isFastForwarding=()=>mode==='forwarding';
        f.base.view.core.length=mode==='unconfirmed'?11:10;
        f.recover();f.tick(40);f.recover();await turn();t.is(a.destroyed,true,mode);t.is(f.calls.length,1);
    }
});

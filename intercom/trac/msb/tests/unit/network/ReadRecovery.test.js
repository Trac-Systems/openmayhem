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
    const peer = n => { const p = { publicKey: key(n), proven: true }; swarm.peers.set(n, p); return p; };
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

test('confirmation or replication progress resets retirement; stale errors cannot retire', async t => {
    for (const mode of ['confirmation', 'data', 'other-block', 'stale']) {
        const f = fixture({ limit: 2 }); f.peer(3); const c = f.connect(1); f.connect(2);
        f.fail(1); f.recover(); f.tick(40);
        if (mode === 'confirmation') { f.base.view.core.signedLength++; f.fail(1); }
        if (mode === 'data') f.fail(1, 1);
        if (mode === 'other-block') f.fail(1, 0, 10);
        if (mode === 'stale') f.tick(1);
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

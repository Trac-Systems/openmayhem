import test from 'brittle';
import b4a from 'b4a';
import { EventEmitter } from 'events';
import Hyperswarm from 'hyperswarm';
import { createReadRecovery } from '../../src/network/readRecovery.js';

const turn = () => new Promise(resolve => setImmediate(resolve));
const topic = b4a.alloc(32, 42);

// Pinned transport retry/queue code, with entirely local fake DHT sockets.
function fixture(t, { limit = 3, refresh = async () => false, rotate = false } = {}) {
  const dht = new EventEmitter(), dialed = [], logs = [];
  dht.createServer = () => ({ close: async () => {} });
  dht.destroy = async () => { for (const connection of dialed) connection.destroy(); };
  dht.connect = remotePublicKey => {
    const connection = Object.assign(new EventEmitter(), { remotePublicKey, destroy() {
      if (this.destroyed) return;
      this.destroyed = true; this.emit('close');
    } });
    dialed.push(connection); return connection;
  };
  const swarm = new Hyperswarm({ dht, keyPair: { publicKey: b4a.alloc(32, 99) }, maxPeers: limit, maxClientConnections: limit });
  t.teardown(() => swarm.destroy());
  const base = { view: { core: { signedLength: 123 } } };
  let time = 100000, refreshes = 0;
  const recover = createReadRecovery({ base, swarm, topic, label: 'MSB', rotate,
    discovery: { refresh() { refreshes++; return refresh(); } }, now: () => time, log: line => logs.push(line) });
  const peer = byte => {
    const p = swarm._upsertPeer(b4a.alloc(32, byte));
    p._topic(topic); p.attempts = 4; p.client = true;
    return p;
  };
  return { swarm, base, peer, recover, dialed, logs, next: () => { time += 30000; }, refreshes: () => refreshes };
}

test('exhausted known peers really dial again despite failed discovery; bans and existing pins survive', async t => {
  const f = fixture(t), stuck = f.peer(1), banned = f.peer(2), pinned = f.peer(3);
  banned.ban(true); pinned.explicit = true; f.swarm.explicitPeers.add(pinned);
  // Keep the unrelated pin backed off, to isolate this repair's dial.
  pinned.attempts = 5;
  f.swarm._requeue([stuck]);
  t.is(f.dialed.length, 0, 'ordinary retry gives up with the known peer still in the map');
  t.is(f.recover({ code: 'REQUEST_TIMEOUT' }), true);
  t.is(f.dialed.length, 1);
  t.alike(f.dialed[0].remotePublicKey, stuck.publicKey);
  t.is(stuck.explicit, false, 'no permanent new pin');
  t.is(pinned.explicit, true);
  t.is(banned.banned, true);
  t.is(f.recover(), false, 'cooldown');
  await turn();
  t.is(f.refreshes(), 1);
  t.ok(f.logs.some(line => line.includes('discovery_failed')));
});

test('a hung discovery does not suppress retries or accumulate refresh promises; offline candidates rotate', async t => {
  const f = fixture(t, { refresh: () => new Promise(() => {}) });
  f.peer(1); f.peer(2); f.peer(3);
  for (let i = 0; i < 3; i++) {
    f.recover(); await turn();
    t.is(f.dialed.length, i + 1);
    t.is(f.dialed[i].remotePublicKey[0], i + 1);
    f.dialed[i].destroy(); f.next();
  }
  t.is(f.refreshes(), 1);
  t.is(f.swarm.explicitPeers.size, 0);
  t.ok(f.logs.some(line => line.includes('"discoveryPending":true')));
});

test('MSB read recovery preserves connected legacy sessions and respects occupied/connecting slots', async t => {
  const f = fixture(t, { limit: 1 }); f.peer(1);
  const legacy = { protocolSession: { preferredProtocol: 'legacy' }, remotePublicKey: b4a.alloc(32, 2),
    destroy() { t.fail('a read must not retire a validator'); } };
  f.swarm.connections.add(legacy);
  f.recover(); t.is(f.dialed.length, 0);
  f.swarm.connections.delete(legacy); f.swarm.connecting = 1; f.next();
  f.recover(); t.is(f.dialed.length, 0);
  f.swarm.connecting = 0;
});

test('rotation waits for one retired socket to close, then dials a different known replica', async t => {
  const f = fixture(t, { limit: 1, rotate: true }); f.peer(1); f.peer(2);
  let destroys = 0;
  const old = Object.assign(new EventEmitter(), { remotePublicKey: b4a.alloc(32, 1), destroy() { destroys++; this.destroying = true; } });
  f.swarm.connections.add(old);
  f.recover(); await turn();
  t.is(destroys, 1); t.is(f.dialed.length, 0);
  f.next(); t.is(f.recover(), false, 'one pending retirement, even after cooldown');
  f.swarm.connections.delete(old); old.emit('close'); await turn();
  t.is(f.dialed.length, 1); t.is(f.dialed[0].remotePublicKey[0], 2);
  t.is(old.listenerCount('close'), 0);
  t.is(f.swarm.explicitPeers.size, 0);
});

test('late close/refresh completions do no recovery work after shutdown', async t => {
  let resolveRefresh;
  const f = fixture(t, { limit: 1, rotate: true, refresh: () => new Promise(resolve => { resolveRefresh = resolve; }) });
  f.peer(1);
  const old = Object.assign(new EventEmitter(), { destroy() {} }); f.swarm.connections.add(old);
  f.recover(); await turn();
  f.base.closing = true; f.swarm.connections.delete(old); old.emit('close'); resolveRefresh(false);
  await turn();
  t.is(f.dialed.length, 0); t.is(f.recover(), false);
  t.is(f.logs.length, 1);
});

test('refresh exceptions are visible and a suspended swarm is left alone', async t => {
  const f = fixture(t, { refresh: async () => { throw new Error('lookup failed'); } });
  f.peer(1); f.recover(); await turn();
  t.ok(f.logs.some(line => line.includes('lookup failed')));
  f.next(); f.swarm.suspended = true;
  t.is(f.recover(), false);
});

test('recovery supports a configured cooldown and contains transport and logging errors', async t => {
  let time = 0, joins = 0, leaves = 0, refreshes = 0;
  const logs = [];
  const swarm = { connections: new Set(), peers: new Map([['peer', {
    publicKey: b4a.alloc(32, 1), proven: true,
  }]]), joinPeer() { joins++; throw new Error('dial failed'); }, leavePeer() { leaves++; },
  status(value) { t.alike(value, topic); return { async refresh() { refreshes++; } }; } };
  const recover = createReadRecovery({ base: {}, swarm, topic, label: 'SUBNET', repairMs: 10,
    now: () => time, log: line => logs.push(line) });
  t.is(recover(), false);
  t.is(joins, 1); t.is(leaves, 1);
  t.ok(logs.some(line => line.includes('dial failed')));
  time = 9; t.is(recover(), false); t.is(joins, 1);
  await turn(); t.is(refreshes, 1);
  time = 10; t.is(recover(), false); t.is(joins, 2); t.is(leaves, 2);
  const brokenLog = createReadRecovery({ base: {}, swarm, topic, label: 'MSB', log() { throw new Error('log failed'); } });
  t.is(brokenLog(), false, 'logger and dial exceptions do not escape');
  t.is(createReadRecovery({ base: {}, label: 'MSB' })(), false, 'absent swarm is inactive');
  await turn();
});

test('rotation leaves a partial connection set intact and candidate filtering respects peer ownership', async t => {
  const f = fixture(t, { limit: 3, rotate: true });
  const connected = f.peer(1), queued = f.peer(2), unrelated = f.peer(3), self = f.peer(5);
  queued.queued = true; unrelated.topics = []; self.publicKey = b4a.alloc(32, 99);
  const live = { remotePublicKey: connected.publicKey, destroy() { t.fail('partial set must stay connected'); } };
  f.swarm.connections.add(live);
  f.recover(); t.is(f.dialed.length, 0);
  const eligible = f.peer(4);
  f.next(); f.recover();
  t.is(f.dialed.length, 1); t.alike(f.dialed[0].remotePublicKey, eligible.publicKey);
  t.is(self.explicit, false);
  f.swarm.connections.delete(live);
  await turn();
});

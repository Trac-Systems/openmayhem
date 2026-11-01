import test from 'brittle';
import b4a from 'b4a';
import { EventEmitter } from 'events';
import { mkdtempPortable, rmrfPortable } from '../helpers/tmpdir.js';
import { fakeIntervals } from '../helpers/fakeIntervals.js';
import os from 'os';
import path from 'path';
import crypto from 'crypto';
import Autobase from 'autobase';
import Corestore from 'corestore';
import Hyperbee from 'hyperbee';
import { MsbClient } from '../../src/msbClient.js';
import { Config, createConfig, ENV } from '../../src/index.js';
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const settle = async (promise, ms) => {
  let timer;
  try {
    return await Promise.race([promise.then(value => ({ value }), error => ({ error })),
      new Promise(resolve => { timer = setTimeout(() => resolve({ pending: true }), ms); })]);
  } finally { clearTimeout(timer); }
};
const keyOf = i => crypto.createHash('sha256').update(`tx${i}`).digest('hex');

// Followers adopt a signed view while disconnected from the skipped block range.
async function sparseFollowers(t) {
  const dir = await mkdtempPortable(path.join(os.tmpdir(), 'trac-peer-msb-read-'));
  const nodes = [], streams = [];
  t.teardown(async () => {
    try { for (const stream of streams) stream.destroy(); for (const node of nodes) { await node.base.close(); await node.store.close(); } }
    finally { await rmrfPortable(dir); }
  });
  const open = async (name, key) => {
    const store = new Corestore(path.join(dir, name));
    const base = new Autobase(store, key, { valueEncoding: 'json', ackInterval: 0,
      open: viewStore => new Hyperbee(viewStore.get('trac'), { extension: false, keyEncoding: 'utf-8', valueEncoding: 'json' }),
      apply: async (batchNodes, view) => {
        const batch = view.batch();
        for (const { value } of batchNodes) await batch.put(value.k, value.v);
        await batch.flush(); await batch.close();
      } });
    await base.ready();
    const node = { store, base, msb: { state: { base } } };
    nodes.push(node);
    return node;
  };
  const link = (x, y) => {
    const a = x.store.replicate(true), b = y.store.replicate(false);
    streams.push(a, b);
    a.on('error', () => {}); b.on('error', () => {}); a.pipe(b).pipe(a);
    return () => { a.destroy(); b.destroy(); };
  };
  const a = await open('a'), b = await open('b', a.base.key), c = await open('c', a.base.key);
  let next = 0;
  const write = async count => {
    for (let i = 0; i < count; i += 50) {
      await a.base.append(Array.from({ length: Math.min(50, count - i) }, () => ({ k: keyOf(next), v: next++ })));
    }
    await a.base.update();
  };
  const caughtUp = async () => {
    const length = a.base.view.core.signedLength;
    for (let i = 0; i < 400; i++) {
      await b.base.update(); await c.base.update();
      if (b.base.view.core.signedLength >= length && c.base.view.core.signedLength >= length) return;
      await sleep(25);
    }
    throw new Error('followers did not catch up');
  };
  let unlink = [link(a, b), link(a, c)];
  await write(500); await caughtUp();
  const from = b.base.view.core.signedLength;
  for (const stop of unlink) stop();
  await write(2000);
  const to = a.base.view.core.signedLength;
  unlink = [link(a, b), link(a, c)];
  await caughtUp();
  for (const stop of unlink) stop();
  const gapKey = keyOf(1000), gapLength = from + 1000;
  return { a, b, c, link, from, to, gapKey, gapLength };
}

test('MSB sparse reads keep waiting until a serving peer supplies the missing block', async t => {
  const f = await sparseFollowers(t), stalls = [], recovered = [], logs = [];
  t.is(await f.b.base.view.core.has(f.gapLength), false, 'skipped range is sparse');
  t.is(await f.c.base.view.core.has(f.gapLength), false, 'other follower cannot serve the range');
  const client = new MsbClient(f.b.msb, { readSliceMs: 40,
    onReadStall: event => recovered.push(event), log: line => logs.push(JSON.parse(line.slice('[trac-peer] '.length))) });
  client.on('read-stall', event => stalls.push(event));
  const core = f.b.base.view.core;
  await client.waitForSignedLengthAtLeast(f.gapLength);
  t.is(f.b.base.view.core, core, 'signed wait retains the view core session');
  f.link(f.b, f.c);
  const pending = client.getSignedAtLength(f.gapKey, f.gapLength);
  t.is((await settle(pending, 180)).pending, true, 'timeouts never resolve as missing entries');
  t.ok(stalls.length >= 2, 'each timed out slice emits a stall');
  t.is(stalls.length, recovered.length);
  t.is(stalls[0], recovered[0], 'listener and recovery receive the same event');
  t.is(stalls[0].code, 'MSB_APPLY_READ_STALLED');
  t.ok(stalls[0].waitingSeq >= f.from && stalls[0].waitingSeq < f.to);
  t.is(stalls[0].coreKey, b4a.toString(core.key, 'hex'));
  t.alike([logs[0].chain, logs[0].event, logs[0].key, logs[0].msbsl, logs[0].attempt],
    ['MSB', 'msb_read_stalled', f.gapKey, f.gapLength, 1]);
  f.link(f.a, f.b);
  const { value, error } = await settle(pending, 5000);
  t.is(error, undefined);
  t.is(value?.key, f.gapKey);
  t.is(value?.value, 1000, 'read returns the original entry once its blocks arrive');
  t.is(await client.getSignedAtLength(keyOf(400000), f.from), null, 'an actual absent entry still returns null');
});

test('MSB sparse reads reject when the base closes', async t => {
  const f = await sparseFollowers(t);
  const client = new MsbClient(f.b.msb, { readSliceMs: 30, log() {} });
  const pending = client.getSignedAtLength(f.gapKey, f.gapLength);
  t.is((await settle(pending, 60)).pending, true);
  const closing = f.b.base.close();
  const { error } = await settle(pending, 3000);
  await closing;
  t.ok(error, 'closing terminates the read');
});

function stub(get, options = {}) {
  let closed = 0, opened = 0;
  const lengths = [];
  const core = Object.assign(new EventEmitter(), { signedLength: 12, key: b4a.alloc(32, 7), peers: [{}, {}] });
  const base = { view: { core, checkout(length) {
    lengths.push(length); opened++;
    return { get, async close() { closed++; } };
  } } };
  const msb = { state: { base }, config: {}, network: {}, async ready() {} };
  const client = new MsbClient(msb, options);
  return { msb, base, core, client, lengths, closed: () => closed, opened: () => opened };
}

const timeout = () => Object.assign(new Error('read timed out'), { code: 'REQUEST_TIMEOUT' });

test('MSB timeouts retry the same signed checkout and close every attempt', async t => {
  let attempt = 0, time = 100;
  const logs = [], events = [], recovered = [];
  const expected = { key: 'tx', value: 'entry' };
  const f = stub(async (key, options) => {
    t.is(key, 'tx'); t.is(options.timeout, 25);
    t.is(f.closed(), attempt, 'previous checkout closes before retry');
    attempt++;
    if (attempt === 3) return expected;
    options.onwait(9);
    if (attempt === 2) options.onseq(9);
    time += 25;
    throw timeout();
  }, { readSliceMs: 25, now: () => time, log: line => logs.push(line), onReadStall: event => recovered.push(event) });
  f.client.on('read-stall', event => events.push(event));
  t.is(await f.client.getSignedAtLength('tx', 10), expected);
  t.alike(f.lengths, [10, 10, 10]); t.is(f.closed(), 3);
  t.alike(events.map(e => e.waitingSeq), [9, null], 'completed block callbacks clear the waiting sequence');
  t.alike(events.map(e => e.attempt), [1, 2]);
  t.alike(events.map(e => e.elapsedMs), [25, 50]);
  t.alike(events.map(e => e.signedLength), [12, 12]);
  t.alike(events.map(e => e.corePeers), [2, 2]);
  t.alike(recovered, events);
  t.is(logs.length, 2); t.ok(logs.every(line => line.startsWith('[trac-peer] ')));
});

test('MSB reads retain one-argument defaults and return null only for absent entries', async t => {
  const f = stub(async (_key, options) => { t.is(options.timeout, 30000); return null; });
  const client = new MsbClient(f.msb);
  t.is(await client.getSignedAtLength('absent', 12), null);
  t.is(f.opened(), 1); t.is(f.closed(), 1);
  t.is(client.recoverRead({}), false, 'recovery is optional before opening');
});

test('MSB non-timeout errors and shutdown timeouts propagate after closing the checkout', async t => {
  for (const state of ['active', 'closing', 'closed']) {
    const error = state === 'active' ? new Error('invalid read') : timeout();
    const f = stub(async () => { throw error; }, { log() { t.fail('no stall on a terminal error'); } });
    if (state !== 'active') f.base[state] = true;
    const result = await settle(f.client.getSignedAtLength('tx', 12), 1000);
    t.is(result.error, error, state); t.is(f.opened(), 1); t.is(f.closed(), 1);
  }
});

test('MSB reporting and checkout cleanup cannot turn a timeout into a skipped read', async t => {
  let attempts = 0, closed = 0;
  const f = stub(null, { log() { throw new Error('log failed'); }, onReadStall() { throw new Error('recovery failed'); } });
  f.client.on('read-stall', () => { throw new Error('listener failed'); });
  f.base.view.checkout = () => ({
    async get() { if (++attempts === 1) throw timeout(); return { value: 42 }; },
    async close() { closed++; throw new Error('close failed'); },
  });
  t.alike(await f.client.getSignedAtLength('tx', 12), { value: 42 });
  t.is(attempts, 2); t.is(closed, 2);
  const error = new Error('read failed');
  f.base.view.checkout = () => ({ async get() { throw error; }, async close() { throw new Error('close failed'); } });
  t.is((await settle(f.client.getSignedAtLength('tx', 12), 1000)).error, error);
});

test('MSB signed-length wait reports periodically and clears its timer and append listener', async t => {
  const timers = fakeIntervals(t), logs = [];
  let time = 0, done = false;
  const f = stub(null, { readSliceMs: 20, now: () => time, log: line => logs.push(JSON.parse(line.slice('[trac-peer] '.length))) });
  const pending = f.client.waitForSignedLengthAtLeast(14, { pollMs: 10000 }).then(() => { done = true; });
  t.is(timers.active.size, 1); t.is([...timers.active.values()][0].ms, 20); t.is(timers.unrefs, 1);
  time = 20; timers.tick();
  f.core.signedLength = 13; f.core.emit('append'); await Promise.resolve();
  time = 40; timers.tick();
  t.is(done, false, 'reporting does not end the wait');
  t.alike(logs.map(e => [e.chain, e.event, e.msbsl, e.signedLength, e.elapsedMs]),
    [['MSB', 'msb_wait_signed_length', 14, 12, 20], ['MSB', 'msb_wait_signed_length', 14, 13, 40]]);
  f.core.signedLength = 14; f.core.emit('append'); await pending;
  t.is(done, true); t.is(timers.active.size, 0); t.is(f.core.listenerCount('append'), 0);
  timers.tick(); t.is(logs.length, 2);
  await f.client.waitForSignedLengthAtLeast(14);
  t.is(timers.active.size, 0, 'already signed targets leave no reporting timer');
});

test('MSB signed-length wait preserves poll fallback and input validation', async t => {
  const f = stub(null, { log() {} });
  const pending = f.client.waitForSignedLengthAtLeast(13, { pollMs: 1 });
  f.core.signedLength = 13;
  await pending;
  t.is(f.core.listenerCount('append'), 0);
  for (const target of [-1, 1.5, NaN]) {
    t.ok((await settle(f.client.waitForSignedLengthAtLeast(target), 1000)).error);
  }
  t.ok((await settle(f.client.waitForSignedLengthAtLeast(12, { pollMs: 0 }), 1000)).error);
});

test('MSB opening installs default transport recovery only when no callback was supplied', async t => {
  const f = stub(async () => null, { log() {} });
  let joins = 0, leaves = 0, refreshes = 0;
  const topic = b4a.alloc(32, 42), key = b4a.alloc(32, 1);
  f.msb.config.channel = topic;
  f.msb.network.swarm = { connections: new Set(), peers: new Map([['peer', { publicKey: key, topics: [topic] }]]),
    joinPeer(value) { t.alike(value, key); joins++; }, leavePeer(value) { t.alike(value, key); leaves++; },
    status(value) { t.alike(value, topic); return { async refresh() { refreshes++; } }; } };
  await f.client.ready();
  t.is(f.client.recoverRead({ code: 'REQUEST_TIMEOUT' }), true);
  await new Promise(resolve => setImmediate(resolve));
  t.is(joins, 1); t.is(leaves, 1); t.is(refreshes, 1);
  let recovered;
  const custom = new MsbClient(f.msb, { onReadStall: event => { recovered = event; return true; } });
  await custom.ready();
  const event = { code: 'MSB_APPLY_READ_STALLED' };
  t.is(custom.recoverRead(event), true); t.is(recovered, event); t.is(joins, 1);
  const noNetwork = stub(async () => null);
  await noNetwork.client.ready(); t.is(noNetwork.client.recoverRead(event), false);
  await f.client.close(); await custom.close(); await noNetwork.client.close();
});

test('read and stall config defaults apply to every environment and older defaults objects', async t => {
  for (const env of Object.values(ENV)) {
    const config = createConfig(env);
    t.is(config.msbReadSliceMs, 30000); t.is(config.subnetStallTimeout, 60000);
  }
  const defaults = { ...createConfig(ENV.DEVELOPMENT) };
  delete defaults.msbReadSliceMs; delete defaults.subnetStallTimeout;
  const config = new Config({}, defaults);
  t.is(config.msbReadSliceMs, 30000); t.is(config.subnetStallTimeout, 60000);
  const custom = createConfig(ENV.DEVELOPMENT, { msbReadSliceMs: 1000, subnetStallTimeout: 0 });
  t.is(custom.msbReadSliceMs, 1000); t.is(custom.subnetStallTimeout, 0);
  t.is(new Config({}, { ...defaults, msbReadSliceMs: 2000 }).msbReadSliceMs, 2000);
  t.is(new Config({}, { ...defaults, subnetStallTimeout: 1 }).subnetStallTimeout, 1);
  t.is(createConfig(ENV.DEVELOPMENT, { msbReadSliceMs: Number.MAX_SAFE_INTEGER }).msbReadSliceMs, Number.MAX_SAFE_INTEGER);
  t.is(createConfig(ENV.DEVELOPMENT, { subnetStallTimeout: Number.MAX_SAFE_INTEGER }).subnetStallTimeout, Number.MAX_SAFE_INTEGER);
  for (const value of [null, -1, 0, 999, 1000.5, '30000', NaN, Infinity, Number.MAX_SAFE_INTEGER + 1]) {
    await t.exception(() => createConfig(ENV.DEVELOPMENT, { msbReadSliceMs: value }), /msbReadSliceMs/);
  }
  for (const value of [null, -1, 0.5, '60000', NaN, Infinity, Number.MAX_SAFE_INTEGER + 1]) {
    await t.exception(() => createConfig(ENV.DEVELOPMENT, { subnetStallTimeout: value }), /subnetStallTimeout/);
  }
});

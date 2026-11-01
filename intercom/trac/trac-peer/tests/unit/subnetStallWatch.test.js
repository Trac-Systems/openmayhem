import test from 'brittle';
import Autobase from 'autobase';
import Corestore from 'corestore';
import Hyperbee from 'hyperbee';
import FastForward from 'autobase/lib/fast-forward.js';
import { mkdtempPortable, rmrfPortable } from '../helpers/tmpdir.js';
import { fakeIntervals } from '../helpers/fakeIntervals.js';
import b4a from 'b4a';
import { Peer, createConfig, ENV, createReadRecovery, createSubnetStallWatch as exportedWatch } from '../../src/index.js';
import os from 'os';
import path from 'path';
import { createSubnetStallWatch } from '../../src/tasks/subnetStallWatch.js';

function fixture(t) {
  let time = 0, queued = 0;
  const repairs = [], logs = [];
  const base = { opened: true, indexedLength: 457518, core: { id: 'system', length: 457518,
    core: { state: { length: 458798 } } }, view: { core: { length: 100, signedLength: 100 } },
    _queueFastForward() { queued++; } };
  const watch = createSubnetStallWatch({ base, now: () => time,
    recover: error => repairs.push(error), log: line => logs.push(JSON.parse(line.slice('[trac-peer] '.length))) });
  t.teardown(() => watch.stop());
  return { base, watch, repairs, logs, queued: () => queued, time: value => { time = value; } };
}

test('signed system lead with no unconfirmed view tip triggers bounded repair independently of balance reads', t => {
  const f = fixture(t);
  t.is(f.watch.check(), false);
  f.time(60000 - 1); t.is(f.watch.check(), false);
  f.base.core.core.state.length += 100; // new signed heads must not restart the stall clock
  f.time(60000); t.is(f.watch.check(), true);
  t.is(f.queued(), 1);
  t.is(f.repairs[0].code, 'SUBNET_APPLY_STALLED');
  t.is(f.logs[0].appliedLength, 457518);
  t.is(f.logs[0].systemSignedLength, 458898);
  t.is(f.watch.check(), false);
  f.time(2 * 60000); t.is(f.watch.check(), true);
  t.is(f.queued(), 2);
});

test('progress, system migration, clock reversal and a cleared gap restart the stall clock', t => {
  const f = fixture(t);
  f.watch.check(); f.time(60000);
  f.base.indexedLength++; f.base.core.length++;
  t.is(f.watch.check(), false);
  f.time(2 * 60000); f.base.core.length++; // replay still progressing
  t.is(f.watch.check(), false);
  f.time(3 * 60000); f.base.core.id = 'migrated-system';
  t.is(f.watch.check(), false);
  f.time(0); t.is(f.watch.check(), false);
  f.base.core.core.state.length = f.base.core.length + 15;
  f.time(60000); t.is(f.watch.check(), false);
  f.base.core.core.state.length++;
  f.time(2 * 60000); t.is(f.watch.check(), false);
  f.time(3 * 60000); t.is(f.watch.check(), true);
  t.is(f.queued(), 1);
});

test('only ready, enabled, read-only subnet followers with a sufficient known signed lead are repaired', t => {
  for (const [key, value] of [['opened', false], ['closing', true], ['closed', true], ['writable', true],
    ['isIndexer', true], ['paused', true], ['_interrupting', true], ['fastForwardEnabled', false]]) {
    const f = fixture(t);
    f.watch.check(); f.base[key] = value; f.time(60000);
    t.is(f.watch.check(), false, key);
    t.is(f.queued(), 0, key); t.is(f.repairs.length, 0, key);
  }
  for (const signed of [undefined, NaN, Infinity, 457518, 457533]) {
    const f = fixture(t); f.base.core.core.state.length = signed;
    f.watch.check(); f.time(60000);
    t.is(f.watch.check(), false); t.is(f.queued(), 0);
  }
});

test('in-flight and prepared fast-forwards retain ownership while their transport can recover', t => {
  for (const key of ['fastForwarding', 'fastForwardTo']) {
    const f = fixture(t), pending = { length: 458798 };
    f.base[key] = pending; f.watch.check(); f.time(60000);
    t.is(f.watch.check(), true);
    t.is(f.repairs.length, 1); t.is(f.queued(), 0);
    t.is(f.base[key], pending);
    f.base[key] = null; f.time(2 * 60000);
    t.is(f.watch.check(), true); t.is(f.queued(), 1);
  }
});

test('subnet watch starts once, stops its timer, and restarts with a fresh baseline', t => {
  const timers = fakeIntervals(t), f = fixture(t);
  f.watch.start(); f.watch.start();
  t.is(timers.active.size, 1);
  t.is([...timers.active.values()][0].ms, 15000);
  t.is(timers.unrefs, 1);
  f.time(60000); timers.tick();
  t.is(f.queued(), 1);
  f.watch.stop(); f.time(120000); timers.tick();
  t.is(timers.active.size, 0);
  t.is(f.watch.check(), false); t.is(f.queued(), 1);
  f.watch.start(); f.time(180000); timers.tick();
  t.is(f.queued(), 2);
  f.watch.stop();
  t.is(timers.active.size, 0);
});

test('subnet watch uses configured stall and check intervals', t => {
  const timers = fakeIntervals(t), f = fixture(t);
  let time = 0;
  const watch = createSubnetStallWatch({ base: f.base, stallMs: 40, checkIntervalMs: 10,
    now: () => time, log() {} });
  t.teardown(() => watch.stop());
  watch.start();
  t.is([...timers.active.values()][0].ms, 10);
  time = 39; timers.tick(); t.is(f.queued(), 0);
  time = 40; timers.tick(); t.is(f.queued(), 1);
  watch.stop();
});

test('transport and queue failures remain observable and do not disable later checks', t => {
  const f = fixture(t), logs = [];
  const watch = createSubnetStallWatch({ base: f.base, now: () => time, log: line => logs.push(line),
    recover() { throw new Error('discovery unavailable'); } });
  t.teardown(() => watch.stop());
  let time = 0;
  watch.check(); time += 60000;
  t.is(watch.check(), true); t.is(f.queued(), 1);
  f.base._queueFastForward = () => { throw new Error('queue unavailable'); };
  time += 60000; t.is(watch.check(), true);
  t.ok(logs.some(line => line.includes('discovery unavailable')));
  t.ok(logs.some(line => line.includes('queue unavailable')));
  t.exception(() => createSubnetStallWatch({ base: {} }), /fast-forward hook unavailable/);
});

const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(predicate) {
  const deadline = Date.now() + 5000;
  while (!await predicate()) {
    if (Date.now() >= deadline) throw new Error('local replication timed out');
    await delay(5);
  }
}

test('pinned base recovers a silent failed FF with equal applied/total lengths and no new system append', async t => {
  t.is(Autobase.prototype.forceFastForward, undefined);
  t.is(Autobase.prototype.repair, undefined);
  const directory = await mkdtempPortable(path.join(os.tmpdir(), 'trac-peer-subnet-watch-'));
  const stores = [], bases = [], streams = [], warnings = [];
  let replayed = 0;
  const open = store => new Hyperbee(store.get('view'), { extension: false, keyEncoding: 'utf-8', valueEncoding: 'json' });
  const apply = async (nodes, view) => {
    const batch = view.batch();
    for (const { value } of nodes) if (value) await batch.put('balance', value.balance);
    await batch.flush(); await batch.close();
  };
  const make = async (name, bootstrap, reader = false) => {
    const store = new Corestore(path.join(directory, name)); stores.push(store);
    const base = new Autobase(store, bootstrap, { open, valueEncoding: 'json', ackInterval: 0,
      fastForward: !reader, apply: reader ? async (...args) => { replayed++; return apply(...args); } : apply });
    bases.push(base); base.on('warning', error => warnings.push(error)); await base.ready(); return base;
  };
  t.teardown(async () => {
    for (const stream of streams) stream.destroy();
    for (const base of bases) await base.close();
    for (const store of stores) await store.close();
    await rmrfPortable(directory);
  });
  const writer = await make('writer', null), reader = await make('reader', writer.key, true);
  const connect = () => {
    const left = writer.replicate(true), right = reader.replicate(false);
    for (const stream of [left, right]) { stream.on('error', () => {}); streams.push(stream); }
    left.pipe(right).pipe(left);
  };
  connect(); await writer.append({ balance: 1 });
  await until(() => reader.core.signedLength > 0 && reader.core.signedLength === writer.core.signedLength);
  await reader.update(); reader.paused = true;
  const before = reader.indexedLength, localKey = reader.local.key.toString('hex');
  for (let balance = 2; balance <= 31; balance++) await writer.append({ balance });
  await until(() => reader.core.core.state.length === writer.core.core.state.length);
  const target = writer.indexedLength, system = reader._viewStore.getSystemCore();
  await system.download({ start: 0, end: system.length }).done();
  const view = reader.store.get({ key: writer.view.core.key }); await view.ready();
  await view.download({ start: 0, end: writer.view.core.signedLength }).done();
  const indexer = reader.store.get({ key: writer.local.key }); await indexer.ready();
  await indexer.download({ start: 0, end: writer.local.length }).done();
  for (const stream of streams) stream.destroy();
  await delay(20);
  await view.clear(view.length - 1, view.length); // signed head survives, required view block doesn't
  reader.fastForwardEnabled = true; reader.paused = false;
  await reader._runFastForward(new FastForward(reader, reader.core.key, { timeout: 20 }));
  t.ok(reader.fastForwardFailedAt > 0);
  t.is(reader.indexedLength, before); t.is(reader.core.length, before);
  t.is(reader.view.core.length, reader.view.core.signedLength, 'no unconfirmed view tip');
  t.is(reader.core.core.state.length, target);
  t.is((await reader.view.get('balance')).value, 1, 'a successful stale read cannot trigger read recovery');
  t.is(warnings.length, 0, 'the upstream failure is silent');

  connect(); await view.get(view.length - 1, { timeout: 1000 });
  await delay(20);
  t.is(reader.indexedLength, before, 'restoring the missing block did not restart FF');
  let time = 0, repairs = 0;
  const watch = createSubnetStallWatch({ base: reader, now: () => time, log() {}, recover() { repairs++; } });
  t.teardown(() => watch.stop()); watch.check();
  time += 60000; watch.check();
  t.is(repairs, 1); t.is(reader.fastForwarding, null, 'upstream cooldown is preserved');
  reader.fastForwardFailedAt = Date.now() - 300001; // expire the real upstream cooldown without a five-minute test
  replayed = 0; time += 60000; watch.check();
  await until(() => reader.indexedLength === target);
  t.is(replayed, 0, 'recovery used verified FF, without replaying the contract');
  t.is(reader.core.length, target); t.is(reader.core.signedLength, target);
  t.is((await reader.view.get('balance')).value, 31);
  t.is(reader.local.key.toString('hex'), localKey); t.is(reader.writable, false);
  t.is(watch.check(), false);
  await view.close(); await indexer.close();
  await writer.append({ balance: 32 });
  await until(() => reader.core.signedLength === writer.core.signedLength);
  t.is((await reader.view.get('balance')).value, 32, 'normal replay continues after FF');
  await reader.close();
  const reopened = await make('reader', writer.key, true);
  t.is(reopened.indexedLength, writer.indexedLength);
  t.is(reopened.local.key.toString('hex'), localKey);
  t.is((await reopened.view.get('balance')).value, 32, 'same-store restart preserves recovery');
});

test('Peer starts its configured watch after replication and stops it before closing the base', async t => {
  const directory = await mkdtempPortable(path.join(os.tmpdir(), 'trac-peer-watch-lifecycle-'));
  const timers = fakeIntervals(t), originalNow = Date.now;
  let time = 0, queued = 0, replicated = false, readAttempts = 0;
  Date.now = () => time;
  const recovered = [], stalls = [], readStalls = [], timeouts = [];
  const base = { opened: false, indexedLength: 20,
    core: { id: 'system', length: 20, core: { state: { length: 36 } } },
    local: { key: b4a.alloc(32, 1) }, key: b4a.alloc(32, 2), view: { core: { signedLength: 20 } },
    async ready() { this.opened = true; },
    _queueFastForward() { queued++; },
    async close() { t.is(timers.active.size, 0, 'watch timer stops before base close'); this.closed = true; },
  };
  class TestPeer extends Peer {
    _boot() { this.base = base; }
    async _replicate() {
      t.is(this.base.opened, true);
      t.is(this.subnetStallWatch, null, 'replication finishes before watch starts');
      this.recoverSubnet = event => recovered.push(event);
      replicated = true;
    }
  }
  const msb = { async ready() {}, config: {}, state: { base: { view: {
    core: { signedLength: 12 }, checkout() { return {
      async get(_key, options) {
        timeouts.push(options.timeout);
        if (++readAttempts === 1) throw Object.assign(new Error('read timed out'), { code: 'REQUEST_TIMEOUT' });
        return { value: 42 };
      }, async close() {},
    }; },
  } } } };
  const makePeer = subnetStallTimeout => new TestPeer({
    config: createConfig(ENV.DEVELOPMENT, { storesDirectory: directory, storeName: 'peer',
      replicate: true, msbReadSliceMs: 1000, subnetStallTimeout }), msb,
    wallet: { async initKeyPair() {} }, protocol: class { async extendApi() {} }, contract: class {},
  });
  let peer, disabled;
  try {
    peer = makePeer(40);
    peer.on('subnet-stall', event => stalls.push(event));
    peer.on('msb-read-stall', event => readStalls.push(event));
    await peer.ready();
    t.is(replicated, true); t.is(timers.active.size, 1);
    time = 39; timers.tick(); t.is(queued, 0);
    time = 40; timers.tick();
    t.is(queued, 1); t.is(recovered.length, 1); t.is(stalls[0], recovered[0]);
    t.is(stalls[0].code, 'SUBNET_APPLY_STALLED');
    t.alike(await peer.msbClient.getSignedAtLength('tx', 12), { value: 42 });
    t.alike(timeouts, [1000, 1000]);
    t.is(readStalls.length, 1); t.is(readStalls[0].event, 'msb_read_stalled');
    await peer.close();
    t.is(timers.active.size, 0);
    time = 80; timers.tick(); t.is(peer.subnetStallWatch.check(), false); t.is(queued, 1);
    base.closed = false; replicated = false;
    disabled = makePeer(0);
    await disabled.ready();
    t.is(replicated, true); t.is(disabled.subnetStallWatch, null); t.is(timers.active.size, 0);
    await disabled.close();
    t.is(exportedWatch, createSubnetStallWatch); t.is(typeof createReadRecovery, 'function');
  } finally {
    Date.now = originalNow;
    for (const instance of [peer, disabled]) {
      if (!instance) continue;
      instance.subnetStallWatch?.stop();
      await instance.msbClient.close(); await instance.store.close();
    }
    await rmrfPortable(directory);
  }
});

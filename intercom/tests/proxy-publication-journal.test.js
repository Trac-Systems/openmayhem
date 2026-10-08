import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import Autobase from 'autobase';
import Corestore from 'corestore';
import Hyperbee from 'hyperbee';
import b4a from 'b4a';
import PeerWallet from 'trac-wallet';
import { spawn } from 'node:child_process';
import MayhemFeature from '../features/mayhem/index.js';
import { CONTRACT_VERSION } from '../contract/contract.js';
import { proxyRegistryFeatureKey } from '../contract/proxy-protocol.js';
import { proxyContractFixture } from './helpers/proxy.js';
import { createProxyCanonicalSnapshot } from '../features/mayhem/proxy-canonical-view.js';
import { createProxyPublicationTransport, installProxyPublicationController } from '../features/mayhem/proxy-publication-transport.js';
import { ProxyPublicationJournal, ProxyPublicationController } from '../features/mayhem/proxy-publication-journal.js';

const clone = value => JSON.parse(JSON.stringify(value));

async function fixture(t, { maxEntries = 16 } = {}) {
  const f = await proxyContractFixture();
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-proxy-pending-'));
  let store;
  let base;
  let feature;
  let controller;
  let journal;
  let transport;
  let dropFeature = false;
  let calls = 0;
  const openBase = async () => {
    store = new Corestore(path.join(root, 'store'));
    base = new Autobase(store, null, { ackInterval: 0, valueEncoding: 'json',
      open: views => new Hyperbee(views.get('view'), { extension: false, keyEncoding: 'utf-8', valueEncoding: 'json' }),
      apply: async (nodes, view) => {
        const batch = view.batch();
        try {
          for (const node of nodes) {
            if (node.value?.type === 'seed') {
              for (const [key, value] of node.value.entries) await batch.put(key, value);
            } else if (node.value?.type === 'feature' && !dropFeature) {
              await f.contract.execute(node.value, batch);
            }
          }
          await batch.flush();
        } finally { await batch.close(); }
      } });
    await base.ready();
    f.peer.base = base;
  };
  await openBase();
  const bootstrap = b4a.toString(base.key, 'hex');
  f.network.subnet_bootstrap = bootstrap; f.context.subnet_bootstrap = bootstrap;
  f.peer.config.bootstrap = bootstrap; f.config.subnet_bootstrap = bootstrap;
  await f.storage.put('proxy/v1/config', f.config);
  await base.append({ type: 'seed', entries: [...f.storage.values.entries()] });
  await base.update();
  f.peer.wallet = { publicKey: f.admin.publicKey,
    sign: message => b4a.toString(f.admin.wallet.sign(b4a.isBuffer(message) ? message : b4a.from(String(message))), 'hex'),
    verify: (signature, message, key) => PeerWallet.verify(
      b4a.isBuffer(signature) ? signature : b4a.from(signature, 'hex'),
      b4a.isBuffer(message) ? message : b4a.from(String(message)),
      b4a.isBuffer(key) ? key : b4a.from(key, 'hex')) };
  f.peer.protocol = { instance: { features: {}, featMaxBytes: () => 64000 } };
  f.peer.contract = { instance: f.contract };
  const directory = path.join(root, 'journal');
  const openController = async (overrides = {}) => {
    transport = createProxyPublicationTransport(f.peer, CONTRACT_VERSION);
    journal = await ProxyPublicationJournal.open({ directory, identity: transport.identity(), maxEntries });
    feature = new MayhemFeature(f.peer, { resultTimeoutMs: 25, resultPollMs: 1,
      withProxyCanonicalSnapshot: createProxyCanonicalSnapshot(f.peer, CONTRACT_VERSION) });
    feature.key = 'mayhem';
    controller = new ProxyPublicationController({ journal, ...transport,
      maxInFlight: Math.min(8, maxEntries),
      admit: (key, envelope, forward) => feature._admitProxyPublication(key, envelope, forward),
      append: async entry => { calls++; return await feature._submitFeature(entry.key, entry.envelope, { nonce: entry.nonce }); },
      result: (entry, result) => feature._featureResponse(entry.key, entry.hash, entry.result_key, result),
      ...overrides });
    return controller;
  };
  await openController();
  t.after(async () => {
    await controller.close(); await feature.stop(); await base.close(); await store.close();
    fs.rmSync(root, { recursive: true, force: true });
  });
  return { ...f, directory, root, openController,
    get base() { return base; }, get feature() { return feature; }, get journal() { return journal; },
    get controller() { return controller; }, get transport() { return transport; }, get calls() { return calls; },
    dropFeatures: () => { dropFeature = true; },
    reopen: async overrides => {
      await controller.close(); await feature.stop(); await base.close(); await store.close();
      await openBase(); await base.update(); await openController(overrides);
    } };
}

test('real signed publication is durable before append and concurrent duplicates append once', async t => {
  const f = await fixture(t);
  const original = f.controller.append;
  f.controller.append = async entry => {
    const disk = JSON.parse(fs.readFileSync(path.join(f.directory, 'pending.json'), 'utf8'));
    assert.deepEqual(disk.entries, [entry]);
    return await original(entry);
  };
  const envelope = await f.create(); const key = await proxyRegistryFeatureKey(envelope);
  const results = await Promise.all(Array.from({ length: 20 }, () => f.controller.submit(key, envelope)));
  assert.ok(results.every(result => result.ok === true));
  assert.equal(f.calls, 1);
  assert.equal(f.journal.list().length, 0);
  const before = f.base.local.length;
  assert.equal((await f.controller.submit(key, envelope)).duplicate, true);
  assert.equal(f.base.local.length, before);
  assert.deepEqual((await f.base.view.get('payout/epoch/542')).value, { status: 'prepared', native: true });
});

test('restart after durable prepare before append recovers the exact nonce', async t => {
  const f = await fixture(t);
  const envelope = await f.create(); const key = await proxyRegistryFeatureKey(envelope);
  const entry = await f.transport.prepare(key, envelope);
  await f.journal.put(entry);
  await f.reopen();
  const recovered = await f.transport.inspect(entry);
  assert.equal(recovered.state, 'absent', JSON.stringify(recovered));
  const result = await f.controller.submit(key, envelope);
  assert.equal(result.ok, true);
  assert.equal(result.hash, entry.hash);
  assert.equal(f.calls, 1);
});

test('lost append acknowledgment recovers the confirmed result across actual store restart', async t => {
  const f = await fixture(t);
  const original = f.controller.append;
  f.controller.append = async entry => { await original(entry); throw new Error('test lost ACK'); };
  const envelope = await f.create(); const key = await proxyRegistryFeatureKey(envelope);
  assert.equal((await f.controller.submit(key, envelope)).status, 'pending');
  assert.equal(f.journal.list().length, 1);
  const before = f.base.local.length;
  await f.reopen();
  assert.equal((await f.controller.submit(key, envelope)).ok, true);
  assert.equal(f.calls, 1);
  assert.equal(f.base.local.length, before);
  assert.equal(f.journal.list().length, 0);
});

test('missing canonical result does not imply absence when generic validation discarded a source block', async t => {
  const f = await fixture(t);
  f.dropFeatures();
  const envelope = await f.create(); const key = await proxyRegistryFeatureKey(envelope);
  assert.equal((await f.controller.submit(key, envelope)).status, 'pending');
  assert.equal(f.calls, 1);
  const entry = f.journal.get(key);
  const evidence = await f.transport.inspect(entry);
  assert.equal(evidence.state, 'pending');
  assert.notEqual(evidence.source?.found_index ?? entry.source.found_index, null, JSON.stringify(evidence));
  const before = f.base.local.length;
  await f.reopen();
  await f.controller.step(); await f.controller.step();
  assert.equal(f.calls, 1);
  assert.equal(f.base.local.length, before);
});

test('absence recovery walks only a bounded pending source interval and persists its cursor', async t => {
  const f = await fixture(t);
  const envelope = await f.create(); const key = await proxyRegistryFeatureKey(envelope);
  const entry = await f.transport.prepare(key, envelope);
  await f.journal.put(entry);
  for (let n = 0; n < 40; n++) await f.base.append({ type: 'native_unrelated', n });
  await f.base.update();
  const evidence = await f.transport.inspect(entry);
  assert.equal(evidence.state, 'pending');
  assert.ok(evidence.source, JSON.stringify(evidence));
  assert.equal(evidence.source.checked_length - entry.source.length, 32);
  await f.journal.advance(key, evidence.source);
  await f.reopen();
  assert.equal(f.journal.get(key).source.checked_length, evidence.source.checked_length);
  assert.equal((await f.controller.submit(key, envelope)).ok, true);
  assert.equal(f.calls, 1);
});

test('unpaid/forged registration never creates a pending entry or ledger append', async t => {
  const f = await fixture(t);
  const envelope = await f.create(); envelope.admission = null;
  const key = await proxyRegistryFeatureKey(envelope);
  const before = f.base.local.length;
  await assert.rejects(f.controller.submit(key, envelope), /admission/);
  assert.equal(f.journal.list().length, 0);
  assert.equal(f.base.local.length, before);
});

test('journal rejects wrong identity, another live writer, mutation and invalid persisted state', async t => {
  const f = await fixture(t);
  await assert.rejects(ProxyPublicationJournal.open({ directory: f.directory, identity: f.transport.identity() }), /writer lock/);
  const envelope = await f.create(); const key = await proxyRegistryFeatureKey(envelope);
  const entry = await f.transport.prepare(key, envelope);
  await f.journal.put(entry);
  const modified = clone(entry); modified.nonce = 'ab'.repeat(32);
  await assert.rejects(f.journal.put(modified), /cannot replace/);
  await f.controller.close();
  await assert.rejects(ProxyPublicationJournal.open({ directory: f.directory, identity: { ...f.transport.identity(), admin: 'ac'.repeat(32) } }), /identity/);
  fs.writeFileSync(path.join(f.directory, 'pending.json'), '{broken');
  await assert.rejects(ProxyPublicationJournal.open({ directory: f.directory, identity: f.transport.identity() }));
  assert.equal(fs.readFileSync(path.join(f.directory, 'pending.json'), 'utf8'), '{broken');
});

test('unflushed source work cannot be interpreted as permission to append again', async t => {
  const f = await fixture(t);
  const envelope = await f.create(); const key = await proxyRegistryFeatureKey(envelope);
  const entry = await f.transport.prepare(key, envelope);
  const original = f.base._appending;
  f.base._appending = [];
  try { assert.equal((await f.transport.inspect(entry)).state, 'pending'); }
  finally { f.base._appending = original; }
  const corrupt = clone(entry); corrupt.source.fork++;
  await assert.rejects(f.transport.inspect(corrupt), /source identity/);
});

test('public writer ingress requires durability even with canonical snapshot configured', async t => {
  const f = await fixture(t);
  const envelope = await f.create(); const key = await proxyRegistryFeatureKey(envelope);
  await assert.rejects(f.feature.submit(key, envelope), /recovery is not configured/);
  assert.equal(f.calls, 0);
  const controller = await installProxyPublicationController(f.feature, {
    directory: path.join(f.root, 'installed'), contractVersion: CONTRACT_VERSION,
  });
  try { assert.equal((await f.feature.submit(key, envelope)).ok, true); }
  finally { await controller.close(); }
});

test('policy revocation during durable journal write is rechecked before append', async t => {
  const f = await fixture(t);
  const save = f.journal.put.bind(f.journal);
  f.journal.put = async entry => {
    await save(entry);
    await f.base.append({ type: 'seed', entries: [['proxy/v1/config', { ...f.config, enabled: false }]] });
  };
  const envelope = await f.create(); const key = await proxyRegistryFeatureKey(envelope);
  await assert.rejects(f.controller.submit(key, envelope), /disabled/);
  assert.equal(f.calls, 0);
  assert.equal(f.journal.list().length, 0);
});

test('storage failure never publishes and leaves native writer available', async t => {
  const f = await fixture(t);
  f.journal._save = async () => { throw new Error('test disk failure'); };
  const envelope = await f.create(); const key = await proxyRegistryFeatureKey(envelope);
  await assert.rejects(f.controller.submit(key, envelope), /disk failure/);
  assert.equal(f.calls, 0);
  await assert.rejects(f.controller.submit(key, envelope), /storage failure/);
  await f.base.append({ type: 'seed', entries: [['native/after-disk-failure', true]] });
  assert.equal((await f.base.view.get('native/after-disk-failure')).value, true);
});

test('pending entries enforce per-provider and aggregate bounds and isolate returned objects', async t => {
  const f = await fixture(t, { maxEntries: 2 });
  const envelope = await f.create(); const key = await proxyRegistryFeatureKey(envelope);
  const entry = await f.transport.prepare(key, envelope);
  await f.journal.put(entry);
  const second = clone(entry); second.key += '2';
  await assert.rejects(f.journal.put(second), /scope is pending/);
  second.scope = `provider:${'bc'.repeat(32)}`;
  await f.journal.put(second);
  const third = clone(entry); third.key += '3'; third.scope = `provider:${'bd'.repeat(32)}`;
  await assert.rejects(f.journal.put(third), /capacity/);
  const copy = f.journal.get(key); copy.envelope.admission = null;
  assert.ok(f.journal.get(key).envelope.admission);
  assert.equal(f.journal.list().length, 2);
});

test('a proved pre-append size rejection is terminal rather than endlessly recovered', async t => {
  const f = await fixture(t);
  f.peer.protocol.instance.featMaxBytes = () => 1;
  const envelope = await f.create(); const key = await proxyRegistryFeatureKey(envelope);
  const before = f.base.local.length;
  const response = await f.controller.submit(key, envelope);
  assert.equal(response.status, 'rejected');
  assert.equal(response.accepted, false);
  assert.equal(f.journal.list().length, 0);
  await f.controller.step();
  assert.equal(f.base.local.length, before);
  assert.equal(f.calls, 1);
});

test('SIGKILL after durable prepare releases the OS lock and preserves exact recovery intent', async t => {
  const f = await fixture(t);
  const envelope = await f.create(); const key = await proxyRegistryFeatureKey(envelope);
  const entry = await f.transport.prepare(key, envelope);
  const identity = f.transport.identity();
  await f.controller.close();
  const module = new URL('../features/mayhem/proxy-publication-journal.js', import.meta.url).href;
  const child = spawn(process.execPath, ['--input-type=module', '-e', `
    import fs from 'node:fs';
    import { ProxyPublicationJournal } from ${JSON.stringify(module)};
    const { directory, identity, entry } = JSON.parse(fs.readFileSync(0, 'utf8'));
    const journal = await ProxyPublicationJournal.open({directory, identity});
    await journal.put(entry);
    process.stdout.write('durable');
    setInterval(() => {}, 1000);
  `], { stdio: ['pipe', 'pipe', 'pipe'] });
  t.after(() => { if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL'); });
  child.stdin.end(JSON.stringify({ directory: f.directory, identity, entry }));
  const exited = new Promise((resolve, reject) => { child.once('error', reject); child.once('exit', (code, signal) => resolve({ code, signal })); });
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => { child.kill('SIGKILL'); reject(new Error('Child preparation timed out')); }, 5000);
    let output = '';
    child.stdout.on('data', bytes => {
      output += bytes.toString();
      if (output.includes('durable')) { clearTimeout(timer); resolve(); }
    });
    child.once('exit', code => { clearTimeout(timer); if (!output.includes('durable')) reject(new Error(`Child exited before prepare: ${code}`)); });
    child.once('error', error => { clearTimeout(timer); reject(error); });
  });
  child.kill('SIGKILL');
  assert.equal((await exited).signal, 'SIGKILL');
  await f.openController();
  assert.deepEqual(f.journal.get(key), entry);
  const result = await f.controller.submit(key, envelope);
  assert.equal(result.ok, true);
  assert.equal(result.hash, entry.hash);
  assert.equal(f.calls, 1);
});

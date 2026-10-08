import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import Autobase from 'autobase';
import Corestore from 'corestore';
import Hyperbee from 'hyperbee';
import { CONTRACT_VERSION } from '../contract/contract.js';
import { createProxyCanonicalSnapshot } from '../features/mayhem/proxy-canonical-view.js';

const admin = 'ab'.repeat(32);

async function fixture(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-proxy-view-'));
  const store = new Corestore(dir);
  const base = new Autobase(store, null, { ackInterval: 0, valueEncoding: 'json',
    open: views => new Hyperbee(views.get('view'), { extension: false, keyEncoding: 'utf-8', valueEncoding: 'json' }),
    apply: async (nodes, view) => {
      const batch = view.batch();
      try {
        for (const node of nodes) for (const [key, value] of node.value?.entries ?? []) await batch.put(key, value);
        await batch.flush();
      } finally { await batch.close(); }
    } });
  t.after(async () => { await base.close(); await store.close(); fs.rmSync(dir, { recursive: true, force: true }); });
  await base.ready();
  const write = async entries => {
    await base.append({ entries });
    await base.update();
  };
  await write([['admin', admin], ['epoch/apply/state', { epoch: 100 }], ['proxy/v1/config', { enabled: true }]]);
  const peer = { base, wallet: { publicKey: admin }, config: { bootstrap: base.key },
    msbClient: { networkId: 918, bootstrapHex: 'ac'.repeat(32) } };
  const snapshot = createProxyCanonicalSnapshot(peer, CONTRACT_VERSION);
  return { base, peer, snapshot, write };
}

test('pins a real confirmed Autobase/Hyperbee view and validates its applied prefix', async t => {
  const f = await fixture(t);
  const startingLength = f.base.view.core.signedLength;
  await f.snapshot(async s => {
    await s.assertCurrent();
    assert.equal(s.proof.signed_length, startingLength);
    assert.match(s.proof.tree_hash, /^[0-9a-f]{64}$/);
    assert.equal(s.context.contract_version, CONTRACT_VERSION);
    assert.deepEqual(await s.read('proxy/v1/config'), { enabled: true });
    await s.assertCurrent();
  });
  assert.equal(f.base.view.core.signedLength, startingLength, 'reads cannot append ledger work');
});

test('unrelated native writes do not invalidate proxy admission or change pinned reads', async t => {
  const f = await fixture(t);
  await f.snapshot(async s => {
    assert.deepEqual(await s.read('proxy/v1/config'), { enabled: true });
    await f.write([['native/example', { unchangedForProxy: true }]]);
    await s.assertCurrent();
    assert.deepEqual(await s.read('proxy/v1/config'), { enabled: true });
  });
});

test('revocation/configuration changes invalidate a pinned registration before forwarding', async t => {
  const f = await fixture(t);
  await f.snapshot(async s => {
    assert.deepEqual(await s.read('proxy/v1/config'), { enabled: true });
    await f.write([['proxy/v1/config', { enabled: false }]]);
    await assert.rejects(s.assertCurrent(), /registry state changed/);
    assert.deepEqual(await s.read('proxy/v1/config'), { enabled: true });
  });
});

test('epoch/admin changes, sparse reader role and replaying writer fail closed', async t => {
  const f = await fixture(t);
  await f.snapshot(async s => {
    await f.write([['epoch/apply/state', { epoch: 101 }]]);
    await assert.rejects(s.assertCurrent(), /context changed/);
  });
  const base = f.peer.base;
  f.peer.base = { writable: true, isIndexer: false, view: base.view, _applyState: base._applyState };
  await assert.rejects(f.snapshot(async () => {}), /indexer is not ready/);
  f.peer.base = base;
  f.peer.contract = { instance: { _mayhemReplayStatus: { active: true } } };
  await assert.rejects(f.snapshot(async () => {}), /indexer is not ready/);
  f.peer.contract = null;
  await f.write([['admin', 'ad'.repeat(32)]]);
  await assert.rejects(f.snapshot(async () => {}), /not the canonical admin/);
});

test('mismatched canonical applied prefix and out-of-scope reads are rejected', async t => {
  const f = await fixture(t);
  const base = f.peer.base;
  const applyCore = base._applyState.view.core;
  f.peer.base = { writable: true, isIndexer: true, view: base.view,
    _applyState: { view: { core: { length: applyCore.length, treeHash: async () => Buffer.alloc(32, 1) } } } };
  await assert.rejects(f.snapshot(async () => {}), /differs from canonical applied prefix/);
  f.peer.base = base;
  await f.snapshot(async s => {
    await assert.rejects(s.read('bal/customer'), /invalid registry read key/);
    const config = await s.read('proxy/v1/config'); config.enabled = 'mutated by caller';
    assert.deepEqual(await s.read('proxy/v1/config'), { enabled: true });
  });
});

test('snapshot callback failure closes only its sessions and preserves native availability', async t => {
  const f = await fixture(t);
  await assert.rejects(f.snapshot(async () => { throw new Error('test callback failure'); }), /callback failure/);
  await f.write([['native/after-failure', 1]]);
  await f.snapshot(async s => assert.deepEqual(await s.read('proxy/v1/config'), { enabled: true }));
  assert.equal((await f.base.view.get('native/after-failure')).value, 1);
});

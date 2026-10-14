import assert from 'node:assert/strict';
import test from 'node:test';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import crypto from 'node:crypto';
import Autobase from 'autobase';
import Corestore from 'corestore';
import Hyperbee from 'hyperbee';
import Wallet from 'trac-peer/src/wallet.js';
import PeerWallet from 'trac-wallet';
import { handlerFor } from 'trac-peer/src/operations/index.js';
import { createConfig, ENV } from 'trac-peer/src/config/env.js';
import { createHash } from 'trac-peer/src/utils/types.js';
import { safeEncodeApplyOperation } from 'trac-msb/src/utils/protobuf/operationHelpers.js';
import MayhemContract from '../contract/contract.js';
import Contract24 from '../contract/history/v24.js';
import MayhemProtocol from '../contract/protocol.js';
import { proxyPublicationFeatureKey } from '../contract/proxy-publication.js';
import { replayCanonicalHistory, replayStorageTypes } from '../scripts/replay-canonical-history.mjs';
import { discoverView } from '../scripts/discover-canonical-history.mjs';

const hex = value => Buffer.from(value).toString('hex');
async function directoryHash(directory) {
  const result = {};
  async function walk(current) {
    for (const entry of await fs.readdir(current, { withFileTypes: true })) {
      const name = path.join(current, entry.name);
      if (entry.isDirectory()) await walk(name);
      else if (entry.isFile()) result[path.relative(directory, name)] = crypto.createHash('sha256').update(await fs.readFile(name)).digest('hex');
    }
  }
  await walk(directory);
  return result;
}

async function fixture(t) {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'mayhem-history-runner-'));
  t.after(() => fs.rm(directory, { recursive: true, force: true }));
  const wallet = new Wallet(); await wallet.ready; await wallet.generateKeyPair();
  const sourcePath = path.join(directory, 'source'), msbPath = path.join(directory, 'msb');
  const store = new Corestore(sourcePath), msbStore = new Corestore(msbPath);
  const msb = new Hyperbee(msbStore.get({ name: 'msb-view' }), { keyEncoding: 'ascii', valueEncoding: 'binary' });
  await msb.ready(); await msb.put('initial', Buffer.from('state'));
  const toAddress = key => PeerWallet.encodeBech32mSafe('trac', Buffer.from(key, 'hex'));
  const msbClient = { networkId: 918, bootstrapHex: '44'.repeat(32), getTxvHex: async () => '45'.repeat(32),
    pubKeyHexToAddress: toAddress, addressToPubKeyHex: address => hex(PeerWallet.decodeBech32mSafe(address)),
    getSignedLength: () => msb.core.signedLength, waitForSignedLengthAtLeast: async () => {},
    getSignedAtLength: async (key, length) => { const snapshot = msb.checkout(length); try { return await snapshot.get(key); } finally { await snapshot.close(); } } };
  const config = createConfig(ENV.DEVELOPMENT);
  const peer = { wallet, msbClient, config, writerLocalKey: '42'.repeat(32) };
  const protocol = new MayhemProtocol(peer, null, config);
  const old = new Contract24(protocol, config), current = new MayhemContract(protocol, config);
  let base = new Autobase(store, null, { fastForward: false, ackInterval: 0, valueEncoding: 'json',
    open: views => new Hyperbee(views.get('view'), { extension: false, keyEncoding: 'utf-8', valueEncoding: 'json' }),
    async apply(nodes, view, applyingBase) {
      const batch = view.batch();
      try {
        for (const node of nodes) {
          const version = node.value?.value?.dispatch?.contract_version ?? node.value?.value?.dispatch?.value?.contract_version;
          const handler = handlerFor(node, { wallet, config, protocolInstance: protocol,
            contractInstance: version === 24 ? old : current, msbClient });
          if (handler) await handler.handle(node.value, batch, applyingBase, node);
        }
        await batch.flush();
      } finally { await batch.close(); }
    } });
  await base.ready(); peer.base = base; config.bootstrap = base.key;
  await base.append({ type: 'addAdmin', key: wallet.publicKey });
  const value = { op: 'rate_oracle', tnk_usd_au: '50000000000000000', source: 'fixture', ts: 3600 };
  const key = 'mayhem_' + await old.rateFeatureKey(value), nonce = 'old';
  const feature = { type: 'feature', key, value: { dispatch: { type: 'mayhem_feature', contract_version: 24,
    key, address: wallet.publicKey, value, nonce, hash: wallet.sign(JSON.stringify(value) + nonce) } } };
  await base.append(feature);
  const versioned = protocol.versionedTransactionObject;
  protocol.versionedTransactionObject = d => ({ ...d, value: { ...d.value, contract_version: 24 } });
  const paid = await protocol.preparePaidTransaction({ type: 'setParams', value: { op: 'set_params', submitted_at: 0,
    effective_at: 86400, values: { price_min_bps: 1 } } });
  protocol.versionedTransactionObject = versioned;
  // Freeze the actual old string hash, independently of the new UTF-8 default.
  paid.surrogate.ch = await createHash(JSON.stringify(paid.dispatch), 'legacy');
  paid.surrogate.tx = await protocol.generateTx(918, paid.surrogate.txv, paid.surrogate.iw,
    paid.surrogate.ch, paid.surrogate.bs, paid.surrogate.mbs, paid.surrogate.nonce);
  paid.surrogate.signature = wallet.sign(Buffer.from(paid.surrogate.tx, 'hex'));
  Object.assign(paid.payload.txo, { ch: paid.surrogate.ch, tx: paid.surrogate.tx, is: paid.surrogate.signature });
  const txo = Object.fromEntries(Object.entries(paid.payload.txo).map(([k, v]) => [k, Buffer.from(v, 'hex')]));
  txo.va = Buffer.from(toAddress(wallet.publicKey));
  await msb.put(paid.surrogate.tx, safeEncodeApplyOperation({ type: 12, address: Buffer.from(paid.payload.address), txo }));
  await base.append({ type: 'tx', key: paid.surrogate.tx, value: { dispatch: paid.dispatch,
    ipk: wallet.publicKey, wp: wallet.publicKey, msbsl: msb.core.signedLength } });
  const policy = { op: 'proxy_policy', context: { network_id: '918', msb_bootstrap: msbClient.bootstrapHex,
    subnet_bootstrap: hex(base.key), contract_version: 30 }, revision: 1,
    action: { kind: 'set_family', family_id: 'fixture', label: 'Fixture', enabled: true } };
  const proxyKey = await proxyPublicationFeatureKey(policy), proxyNonce = 'proxy';
  await base.append({ type: 'feature', key: proxyKey, value: { dispatch: { type: 'mayhem_feature', contract_version: 30,
    key: proxyKey, address: wallet.publicKey, value: policy, nonce: proxyNonce,
    hash: wallet.sign(JSON.stringify(policy) + proxyNonce) } } });
  await base.append(feature); // Accepted historical duplicate must not write twice.
  await base.update();
  assert((await base.view.get('proxy/v1/family/fixture'))?.value.enabled);
  assert.equal((await base.view.get('txi/0')).value.err, null);
  const manifest = { schema_version: 1, captured_at: new Date().toISOString(),
    subnet: { store: sourcePath, bootstrap: hex(base.key), view_key: hex(base.view.core.key),
      signed_length: base.view.core.signedLength, fork: base.view.core.fork,
      tree_hash: hex(await base.view.core.treeHash(base.view.core.signedLength)) },
    msb: { store: msbPath, bootstrap: msbClient.bootstrapHex, view_key: hex(msb.core.key),
      signed_length: msb.core.signedLength, fork: msb.core.fork, tree_hash: hex(await msb.core.treeHash()),
      network_id: 918, address_prefix: 'trac' } };
  await base.close(); await store.close(); await msb.close(); await msbStore.close(); base = null;
  return { directory, manifest };
}

test('isolated full replay preserves real signed legacy TX, historical Feature and v30 proxy state', { timeout: 30000 }, async t => {
  const f = await fixture(t);
  const before = await directoryHash(f.manifest.subnet.store), msbBefore = await directoryHash(f.manifest.msb.store);
  const report = await replayCanonicalHistory({ manifest: f.manifest, output: path.join(f.directory, 'replay'), timeoutMs: 15000, stableMs: 0 });
  assert.equal(report.ok, true); assert.equal(report.fast_forward, false);
  assert(report.counts.operations.tx >= 1); // Autobase may undo/reapply speculative prefixes.
  assert(report.counts.contract_versions['24'] >= 2);
  assert(report.counts.contract_versions['30'] >= 1);
  assert(report.comparison.keys > 5);
  assert.deepEqual(await directoryHash(f.manifest.subnet.store), before, 'Source database bytes must be unchanged');
  assert.deepEqual(await directoryHash(f.manifest.msb.store), msbBefore, 'MSB database bytes must be unchanged');
  await assert.rejects(replayCanonicalHistory({ manifest: f.manifest, output: path.join(f.directory, 'replay'), stableMs: 0 }), /EEXIST/);
});

test('replay rejects an incorrect canonical root before accepting history', { timeout: 30000 }, async t => {
  const f = await fixture(t);
  f.manifest.subnet.tree_hash = '00'.repeat(32);
  await assert.rejects(replayCanonicalHistory({ manifest: f.manifest, output: path.join(f.directory, 'rejected'), timeoutMs: 15000, stableMs: 0 }), /Pinned signed tree hash mismatch/);
});

for (const binary of [false, true]) test(`signed metadata discovers ${binary ? 'MSB' : 'subnet'} view without replay`, { timeout: 30000 }, async t => {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'mayhem-view-discovery-'));
  t.after(() => fs.rm(directory, { recursive: true, force: true }));
  const types = replayStorageTypes(binary), store = new types.Corestore(path.join(directory, 'writer'));
  let applyCalls = 0;
  const base = new types.Autobase(store, null, { ackInterval: 0, valueEncoding: 'json',
    open: views => new types.Hyperbee(views.get(binary ? 'trac' : 'view'), { keyEncoding: 'utf-8', valueEncoding: 'json' }),
    async apply(nodes, view) { applyCalls++; const batch = view.batch();
      for (const node of nodes) if (node.value) await batch.put(node.value.key, node.value.value);
      await batch.flush(); await batch.close(); } });
  await base.ready();
  await base.append({ key: 'fixture', value: 1 });
  await base.append({ key: 'fixture', value: 2 });
  await base.update();
  const identity = { bootstrap: hex(base.key), signed_length: base.view.core.signedLength,
    tree_hash: hex(await base.view.core.treeHash(base.view.core.signedLength)) };
  for (let i = 0; i < 20; i++) await base.append({ key: `tail/${i}`, value: i });
  await base.update();
  const expectedKey = hex(base.view.core.key), calls = applyCalls;
  const sparse = new types.Corestore(path.join(directory, 'sparse-reader'));
  const a = store.replicate(true), b = sparse.replicate(false); a.pipe(b).pipe(a);
  const discovered = await discoverView(sparse, identity, { binary, timeoutMs: 10000 });
  assert.equal(discovered.view_key, expectedKey);
  assert.equal(discovered.tree_hash, identity.tree_hash);
  assert.equal(applyCalls, calls, 'Metadata discovery must not execute contract apply');
  const sparseView = sparse.get({ key: Buffer.from(expectedKey, 'hex'), writable: false }); await sparseView.ready();
  assert(sparseView.signedLength > identity.signed_length, 'Fixture must have a newer signed head');
  assert(sparseView.contiguousLength < sparseView.length, 'Historical prefix proof must not hydrate the entire view');
  await sparseView.close(); a.destroy(); b.destroy(); await sparse.close();
  await base.close(); await store.close();
});

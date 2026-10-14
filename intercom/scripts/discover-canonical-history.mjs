import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';
import { createRequire } from 'node:module';
import { inputStore, replayStorageTypes } from './replay-canonical-history.mjs';

const require = createRequire(import.meta.url);
const hex = value => Buffer.from(value).toString('hex');
const hex32 = value => typeof value === 'string' && /^[0-9a-f]{64}$/.test(value);

// This reads signed metadata only. It neither constructs Autobase nor applies
// operations. Oplog digests identify system cores; signed system Info records
// identify views and newer indexer writers after membership migrations.
export async function discoverView(store, identity, { binary = false, timeoutMs = 120000,
  onProgress = () => {} } = {}) {
  const types = replayStorageTypes(binary);
  const messages = types.messages ?? require('autobase/lib/messages.js');
  const encoding = types.encoding ?? require('compact-encoding');
  const deadline = Date.now() + timeoutMs;
  const queue = [identity.bootstrap, ...(identity.writer_keys ?? [])];
  const visitedWriters = new Set(), visitedSystems = new Set();
  const evidence = [];
  const sessions = [];
  const open = async (key, valueEncoding) => {
    const core = store.get({ key: Buffer.from(key, 'hex'), writable: false, ...(valueEncoding ? { valueEncoding } : {}) });
    sessions.push(core); await core.ready();
    while (core.signedLength === 0) {
      assert(Date.now() < deadline, `Timed out discovering signed core ${key}`);
      await core.update({ wait: false }); await delay(100);
    }
    return core;
  };
  try {
    while (queue.length) {
      assert(Date.now() < deadline, 'View discovery timed out');
      assert(visitedWriters.size < 1024, 'View discovery exceeded its writer bound');
      const writerKey = queue.shift();
      if (visitedWriters.has(writerKey)) continue;
      visitedWriters.add(writerKey);
      const writer = await open(writerKey, messages.OplogMessage);
      let index = writer.signedLength - 1, digest = null;
      // Indexer records normally carry a digest in every block. Limit backward
      // scans for a removed indexer or a supplied non-indexer writer.
      for (let scanned = 0; index >= 0 && scanned < 1024; scanned++, index--) {
        const block = await writer.get(index, { timeout: Math.max(1, deadline - Date.now()) });
        if (!block.digest) continue;
        const at = index - block.digest.pointer;
        assert(at >= 0 && at <= index, 'Invalid signed digest pointer');
        const pointed = block.digest.pointer === 0 ? block : await writer.get(at, { timeout: Math.max(1, deadline - Date.now()) });
        if (pointed.digest?.key) { digest = { key: hex(pointed.digest.key), at: at + 1 }; break; }
      }
      if (!digest || visitedSystems.has(digest.key)) continue;
      visitedSystems.add(digest.key);
      const system = await open(digest.key);
      const systemLength = system.signedLength;
      const bee = new types.Hyperbee(system.session({ writable: false }), { extension: false, keyEncoding: 'binary' });
      let info;
      try {
        const row = await bee.getBySeq(systemLength - 1, { timeout: Math.max(1, deadline - Date.now()) });
        assert(row?.value, 'Signed system Info record is missing');
        info = encoding.decode(messages.Info, row.value);
      } finally { await bee.close(); }
      evidence.push({ writer_key: writerKey, writer_signed_length: writer.signedLength,
        digest_at: digest.at, system_key: digest.key, system_signed_length: systemLength,
        system_tree_hash: hex(await system.treeHash(systemLength)) });
      for (const indexer of info.indexers ?? []) {
        const key = hex(indexer.key);
        if (!visitedWriters.has(key)) queue.push(key);
      }
      onProgress({ writer_key: writerKey, system_key: digest.key, views: info.views.map(v => ({ key: hex(v.key), length: v.length })) });
      for (let viewIndex = 0; viewIndex < info.views.length; viewIndex++) {
        if (identity.view_index !== undefined && identity.view_index !== viewIndex) continue;
        const item = info.views[viewIndex];
        const boundary = identity.signed_length ?? item.length;
        if (item.length < boundary || boundary < 1) continue;
        const view = await open(hex(item.key));
        if (view.signedLength < boundary) continue;
        // An upgraded sparse core knows the current signed head but may not
        // yet hold the Merkle nodes for an older prefix. Fetching its final
        // block acquires an authenticated proof for that exact prefix path.
        await view.get(boundary - 1, { timeout: Math.max(1, deadline - Date.now()) });
        const treeHash = hex(await view.treeHash(boundary));
        if (identity.tree_hash && identity.tree_hash !== treeHash) continue;
        if (identity.fork !== undefined && identity.fork !== view.fork) continue;
        return { ...identity, view_key: hex(item.key), signed_length: boundary, fork: view.fork,
          tree_hash: treeHash, discovery: { view_index: viewIndex, evidence } };
      }
    }
    throw new Error('No signed view matches the requested canonical boundary/root; supply a current signed indexer writer key if the bootstrap writer is retired');
  } finally {
    await Promise.allSettled(sessions.reverse().map(core => core.close()));
  }
}

export async function discoverCanonicalHistory({ manifest, output, network = false,
  resume = false, timeoutMs = 120000, onProgress = () => {} }) {
  assert.equal(manifest.schema_version, 1);
  for (const name of ['subnet', 'msb']) {
    const item = manifest[name]; assert(item && hex32(item.bootstrap), `Invalid ${name} bootstrap`);
    if (network) assert(hex32(item.channel_hex), `Missing ${name} channel_hex`);
    else assert(typeof item.store === 'string' && item.store.length, `Missing ${name} store`);
    if (item.signed_length !== undefined) assert(Number.isSafeInteger(item.signed_length) && item.signed_length > 0);
    if (item.tree_hash !== undefined) assert(hex32(item.tree_hash));
    assert((item.writer_keys ?? []).every(hex32));
  }
  output = path.resolve(output);
  if (resume) {
    assert(network, 'Only network discovery caches can be resumed');
    const entries = await fs.readdir(output, { withFileTypes: true });
    assert(entries.every(entry => !entry.isSymbolicLink() && ['subnet-input', 'msb-input', 'manifest.json', 'discovery-request.json'].includes(entry.name)),
      'Resume directory is not an isolated discovery cache');
    const prior = await fs.readFile(path.join(output, 'discovery-request.json'), 'utf8').catch(error => {
      if (error.code === 'ENOENT') return null; throw error;
    });
    if (prior !== null) assert.deepEqual(JSON.parse(prior), manifest, 'Discovery resume request changed');
  } else await fs.mkdir(output); // Replay output never supports resume.
  await fs.writeFile(path.join(output, 'discovery-request.json'), JSON.stringify(manifest, null, 2) + '\n');
  const resources = { stores: [], swarms: [] };
  let failure;
  try {
    const result = { ...manifest, captured_at: manifest.captured_at ?? new Date().toISOString() };
    for (const name of ['subnet', 'msb']) {
      const store = await inputStore(manifest[name], path.join(output, `${name}-input`), network, resources,
        error => { failure ??= error; }, name === 'msb');
      result[name] = await discoverView(store, manifest[name], { binary: name === 'msb', timeoutMs,
        onProgress: value => { if (failure) throw failure; onProgress({ network: name, ...value }); } });
      // A discovery cache is sparse. Network replay must continue to acquire
      // missing input/history blocks; this is not advertised as a full snapshot.
      delete result[name].store;
    }
    if (failure) throw failure;
    await fs.writeFile(path.join(output, 'manifest.json'), JSON.stringify(result, null, 2) + '\n');
    return result;
  } finally {
    await Promise.allSettled(resources.swarms.map(swarm => swarm.destroy()));
    await Promise.allSettled(resources.stores.reverse().map(store => store.close()));
  }
}

async function main() {
  const args = process.argv.slice(2), options = {};
  if (args.includes('--help')) {
    console.log('Usage: node intercom/scripts/discover-canonical-history.mjs --manifest FILE --output NEW_DIRECTORY [--network] [--resume] [--timeout-ms 120000]');
    return;
  }
  for (let i = 0; i < args.length; i++) {
    if (args[i] === '--network') options.network = true;
    else if (args[i] === '--resume') options.resume = true;
    else { assert(['--manifest', '--output', '--timeout-ms'].includes(args[i]), `Unknown option ${args[i]}`);
      assert(args[i + 1], `Missing value for ${args[i]}`); options[args[i].slice(2)] = args[++i]; }
  }
  assert(options.manifest && options.output, '--manifest and --output are required');
  const manifest = JSON.parse(await fs.readFile(options.manifest, 'utf8'));
  await discoverCanonicalHistory({ manifest, output: options.output, network: options.network, resume: options.resume,
    ...(options['timeout-ms'] ? { timeoutMs: Number(options['timeout-ms']) } : {}),
    onProgress: value => console.error(JSON.stringify(value)) });
  console.log(JSON.stringify({ manifest: path.resolve(options.output, 'manifest.json'), replay_verified: false }));
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch(error => { console.error(error.stack ?? error.message); process.exitCode = 1; });
}

import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import crypto from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';
import { setTimeout as delay } from 'node:timers/promises';
import Autobase from 'autobase';
import Corestore from 'corestore';
import Hyperbee from 'hyperbee';
import Hyperswarm from 'hyperswarm';
import PeerWallet from 'trac-wallet';
import Wallet from 'trac-peer/src/wallet.js';
import { handlerFor } from 'trac-peer/src/operations/index.js';
import { createConfig, ENV } from 'trac-peer/src/config/env.js';
import MayhemProtocol from '../contract/protocol.js';
import MayhemContract, { CONTRACT_VERSION } from '../contract/contract.js';
import { CONTRACT_CODE_PATHS } from '../src/release-identity.js';
import { adminWriterDiagnostics, appliedViewProof } from '../src/rpc.js';

const hex = value => Buffer.from(value).toString('hex');
const hex32 = value => typeof value === 'string' && /^[0-9a-f]{64}$/.test(value);
const root = fileURLToPath(new URL('../', import.meta.url));
const msbRequire = createRequire(import.meta.resolve('trac-msb'));
export const replayStorageTypes = binary => binary
  ? { Autobase: msbRequire('autobase'), Corestore: msbRequire('corestore'), Hyperbee: msbRequire('hyperbee'),
      messages: msbRequire('autobase/lib/messages.js'), encoding: msbRequire('compact-encoding') }
  : { Autobase, Corestore, Hyperbee };

function positive(value, name, minimum = 1) {
  assert(Number.isSafeInteger(value) && value >= minimum, `Invalid ${name}`);
  return value;
}

export function validateReplayManifest(manifest, network = false) {
  assert.equal(manifest.schema_version, 1, 'Unsupported replay manifest');
  assert(Number.isFinite(Date.parse(manifest.captured_at)), 'Missing capture timestamp');
  for (const name of ['subnet', 'msb']) {
    const item = manifest[name];
    assert(item && hex32(item.bootstrap) && hex32(item.view_key) && hex32(item.tree_hash), `Invalid ${name} identity`);
    positive(item.signed_length, `${name}.signed_length`);
    positive(item.fork, `${name}.fork`, 0);
    if (network) {
      assert(hex32(item.channel_hex), `Missing ${name}.channel_hex`);
      if (item.dht_bootstrap !== undefined) assert(Array.isArray(item.dht_bootstrap) && item.dht_bootstrap.every(x => typeof x === 'string'));
      if (item.peers !== undefined) assert(Array.isArray(item.peers) && item.peers.every(hex32));
    } else {
      assert(typeof item.store === 'string' && item.store.length > 0, `Missing ${name}.store`);
    }
  }
  positive(manifest.msb.network_id, 'msb.network_id');
  assert(typeof manifest.msb.address_prefix === 'string' && /^[a-z0-9]+$/.test(manifest.msb.address_prefix));
  for (const [key, value] of Object.entries(manifest.limits ?? {})) {
    assert(['maxMsbSignedLength', 'maxMsbSignedLengthFutureDelta', 'maxMsbApplyOperationBytes'].includes(key), `Unsupported replay limit ${key}`);
    positive(value, key, key === 'maxMsbSignedLengthFutureDelta' ? 0 : 1);
  }
  return manifest;
}

async function fingerprint() {
  const paths = [...new Set([...CONTRACT_CODE_PATHS,
    'trac/trac-peer/src/artifacts/protocol.js', 'trac/trac-peer/src/api.js',
    'trac/trac-peer/src/utils/types.js', 'trac/trac-peer/src/config/config.js',
    'src/writer-checkpoint-transport.js', 'scripts/replay-canonical-history.mjs',
    'trac/trac-peer/package.json', 'package-lock.json'])].sort();
  const files = {};
  for (const name of paths) {
    files[name] = crypto.createHash('sha256').update(await fs.readFile(path.join(root, name))).digest('hex');
    if (name.startsWith('trac/trac-peer/src/')) {
      const installed = fileURLToPath(import.meta.resolve(name.replace('trac/trac-peer/', 'trac-peer/')));
      const actual = crypto.createHash('sha256').update(await fs.readFile(installed)).digest('hex');
      assert.equal(actual, files[name], `Installed candidate differs from vendor source: ${name}`);
    }
  }
  return { contract_version: CONTRACT_VERSION, tx_content_hash: 'utf8', files,
    sha256: crypto.createHash('sha256').update(JSON.stringify(files)).digest('hex') };
}

function contains(parent, child) {
  const relative = path.relative(parent, child);
  return relative === '' || (!relative.startsWith(`..${path.sep}`) && relative !== '..' && !path.isAbsolute(relative));
}

export async function inputStore(item, directory, network, resources, fail, binary = false) {
  // Hypercore opens perform local bookkeeping writes even with writable:false.
  // Never open caller-owned storage: copy a coherent snapshot into this run.
  if (!network) await fs.cp(item.store, directory, { recursive: true, errorOnExist: true, force: false,
    filter: async source => {
      const relative = path.relative(item.store, source);
      if (relative === '') return true;
      // Copy only RocksDB storage files, never adjacent wallet/config files.
      if (relative.split(path.sep)[0] !== 'db') return false;
      const entry = await fs.lstat(source);
      assert(!entry.isSymbolicLink(), 'Snapshot storage must not contain symbolic links');
      return entry.isDirectory() || /^(?:[0-9]+\.(?:sst|log|blob)|CURRENT|IDENTITY|LOCK|LOG(?:\.old\..*)?|MANIFEST-[0-9]+|OPTIONS-[0-9]+)$/.test(path.basename(source));
    } });
  const Store = replayStorageTypes(binary).Corestore;
  const store = new Store(directory, { allowBackup: !network });
  resources.stores.push(store);
  try { await store.ready(); } catch (error) { throw new Error(`Input store could not open: ${error.message}`, { cause: error }); }
  if (network) {
    // Raw replication only. No Peer, registration channel, wallet file, MSB
    // services, updater, producer, or transaction API is started here.
    const swarm = new Hyperswarm({ ...(item.dht_bootstrap ? { bootstrap: item.dht_bootstrap } : {}) });
    resources.swarms.push(swarm);
    swarm.on('connection', connection => {
      connection.on('error', () => {});
      const stream = store.replicate(connection);
      stream.on('error', () => {});
    });
    swarm.on('error', fail);
    swarm.join(Buffer.from(item.channel_hex, 'hex'), { server: false, client: true });
    for (const key of item.peers ?? []) swarm.joinPeer(Buffer.from(key, 'hex'));
  }
  return store;
}

async function authenticatedView(store, item, binary, resources, deadline, failure) {
  const core = store.get({ key: Buffer.from(item.view_key, 'hex'), writable: false });
  try { await core.ready(); } catch (error) { throw new Error(`Input view could not open: ${error.message}`, { cause: error }); }
  while (core.signedLength < item.signed_length) {
    failure();
    assert(Date.now() < deadline, 'Timed out acquiring the pinned signed view');
    await core.update({ wait: false });
    await delay(100);
  }
  assert.equal(core.fork, item.fork, 'Pinned view fork changed');
  await core.get(item.signed_length - 1, { timeout: Math.max(1, deadline - Date.now()) });
  assert.equal(hex(await core.treeHash(item.signed_length)), item.tree_hash, 'Pinned signed tree hash mismatch');
  const Bee = replayStorageTypes(binary).Hyperbee;
  const view = new Bee(core, { extension: false,
    keyEncoding: binary ? 'ascii' : 'utf-8', valueEncoding: binary ? 'binary' : 'json' });
  resources.views.push(view);
  await view.ready();
  return view;
}

function pinnedCanonical(view, length, timeout) {
  return { core: { signedLength: length }, checkout(at) {
    assert.equal(at, length);
    const snapshot = view.checkout(length);
    return { get: key => snapshot.get(key, { timeout }), close: () => snapshot.close() };
  } };
}

function msbReader(view, identity, timeout, counts) {
  return {
    networkId: identity.network_id, bootstrapHex: identity.bootstrap,
    addressPrefix: identity.address_prefix,
    addressToPubKeyHex: address => hex(PeerWallet.decodeBech32mSafe(address)),
    pubKeyHexToAddress: key => PeerWallet.encodeBech32mSafe(identity.address_prefix, Buffer.from(key, 'hex')),
    getSignedLength: () => identity.signed_length,
    async waitForSignedLengthAtLeast(length) {
      assert(length <= identity.signed_length, `MSB dependency ${length} exceeds the pinned MSB boundary`);
    },
    async getSignedAtLength(key, length) {
      positive(length, 'MSB checkout length', 0);
      assert(length <= identity.signed_length, 'MSB checkout exceeds pinned boundary');
      const snapshot = view.checkout(length);
      try {
        const entry = await snapshot.get(key, { timeout });
        counts.msb_reads++;
        return entry;
      } finally { await snapshot.close(); }
    },
  };
}

export async function compareMaterializedState(base, canonical, boundary, expectedHash, timeout = 30000) {
  const proof = await appliedViewProof({ base }, boundary);
  assert(proof, 'Pinned prefix is not materialized');
  assert.equal(proof.tree_hash, expectedHash, 'Materialized prefix tree differs from canonical history');
  const actualBee = new Hyperbee(base._applyState.view.core.session({ writable: false }),
    { extension: false, keyEncoding: 'utf-8', valueEncoding: 'json' });
  const actual = actualBee.checkout(boundary), expected = canonical.checkout(boundary);
  const left = actual.createReadStream({}, { timeout }), right = expected.createReadStream({}, { timeout });
  let keys = 0;
  const digest = crypto.createHash('sha256');
  try {
    const a = left[Symbol.asyncIterator](), b = right[Symbol.asyncIterator]();
    while (true) {
      const [av, bv] = await Promise.all([a.next(), b.next()]);
      assert.equal(av.done, bv.done, 'Materialized and canonical key counts differ');
      if (av.done) break;
      assert.equal(av.value.key, bv.value.key, 'Materialized and canonical keys differ');
      assert.deepEqual(av.value.value, bv.value.value, `State differs at ${av.value.key}`);
      digest.update(JSON.stringify([av.value.key, av.value.value]) + '\n');
      keys++;
    }
    return { ...proof, keys, state_sha256: digest.digest('hex') };
  } finally {
    left.destroy(); right.destroy();
    await actual.close(); await expected.close(); await actualBee.close();
  }
}

function idle(peer) {
  const c = adminWriterDiagnostics(peer);
  return c.base.opened && !c.base.closing && !c.base.paused && c.base.caught_up
    // Autobase 7.6 retains the resolved _advancing Promise. The caller awaits
    // base.update(); non-null _advancing is not an active-work indicator.
    && !c.base.draining && c.base.appending_count === 0
    && c.apply_state && !c.apply_state.applying
    && !c.contract.execution_queued && !c.contract.replay.active;
}

export async function replayCanonicalHistory({ manifest, output, network = false,
  timeoutMs = 3600000, readTimeoutMs = 30000, stableMs = 60000, onProgress = () => {} }) {
  validateReplayManifest(manifest, network);
  positive(timeoutMs, 'timeoutMs'); positive(readTimeoutMs, 'readTimeoutMs'); positive(stableMs, 'stableMs', 0);
  output = path.resolve(output);
  // mkdir without recursive/exist_ok is intentional: an existing apply store
  // can never satisfy a genesis replay request.
  const parent = await fs.realpath(path.dirname(output));
  output = path.join(parent, path.basename(output));
  if (!network) for (const item of [manifest.subnet, manifest.msb]) {
    item.store = await fs.realpath(item.store);
    assert(!contains(item.store, output) && !contains(output, item.store), 'Output overlaps an input store');
  }
  await fs.mkdir(output);
  const resources = { stores: [], views: [], swarms: [], streams: [] };
  let base = null, failure = null;
  const fail = error => { failure ??= error; };
  const check = () => { if (failure) throw failure; };
  const deadline = Date.now() + timeoutMs;
  const counts = { apply_calls: 0, nodes: 0, operations: {}, contract_versions: {}, msb_reads: 0 };
  let progressAt = 0;
  const candidate = await fingerprint();
  await fs.writeFile(path.join(output, 'manifest.json'), JSON.stringify(manifest, null, 2) + '\n', { flag: 'wx' });
  try {
    const source = await inputStore(manifest.subnet, path.join(output, 'subnet-input'), network, resources, fail);
    const msbSource = await inputStore(manifest.msb, path.join(output, 'msb-input'), network, resources, fail, true);
    const canonical = await authenticatedView(source, manifest.subnet, false, resources, deadline, check);
    const msb = await authenticatedView(msbSource, manifest.msb, true, resources, deadline, check);
    let store = new Corestore(path.join(output, 'replayed'));
    resources.stores.push(store);
    const wallet = new Wallet(); await wallet.ready; // Verification only; no keypair is loaded or generated.
    const config = createConfig(ENV.DEVELOPMENT, { bootstrap: manifest.subnet.bootstrap,
      txContentHash: 'utf8', ...(manifest.limits ?? {}) });
    assert.equal(config.txContentHash, 'utf8', 'Replay must accept legacy and UTF-8 hashes');
    const peer = { config, wallet, base: null, msbClient: msbReader(msb, manifest.msb, readTimeoutMs, counts) };
    let protocol = new MayhemProtocol(peer, null, config);
    let contract = new MayhemContract(protocol, config);
    peer.protocol = { instance: protocol }; peer.contract = { instance: contract };
    const canonicalView = pinnedCanonical(canonical, manifest.subnet.signed_length, readTimeoutMs);
    const handlers = {
      fastForward: false, ackInterval: 0, valueEncoding: 'json',
      open: s => new Hyperbee(s.get('view'), { extension: false, keyEncoding: 'utf-8', valueEncoding: 'json' }),
      async apply(nodes, view, applyingBase) {
        counts.apply_calls++;
        const batch = view.batch();
        try {
          for (const node of nodes) {
            counts.nodes++;
            const type = node.value?.type ?? 'null';
            counts.operations[type] = (counts.operations[type] ?? 0) + 1;
            const version = node.value?.value?.dispatch?.contract_version ?? node.value?.value?.dispatch?.value?.contract_version;
            if (version !== undefined) counts.contract_versions[version] = (counts.contract_versions[version] ?? 0) + 1;
            const handler = handlerFor(node, { wallet, config, protocolInstance: protocol,
              contractInstance: contract, canonicalView, msbClient: peer.msbClient });
            if (handler) await handler.handle(node.value, batch, applyingBase, node);
          }
          await batch.flush();
        } finally { await batch.close(); }
      },
    };
    const openBase = async () => {
      base = new Autobase(store, Buffer.from(manifest.subnet.bootstrap, 'hex'), handlers);
      peer.base = base; protocol.base = base;
      base.on('error', fail); base.on('warning', fail);
      await base.ready();
    };
    await openBase();
    assert.equal(base.fastForwardEnabled, false);
    assert.equal(base._applyState.view.core.length, 0, 'Replay output did not start empty');
    const a = source.replicate(true), b = store.replicate(false);
    a.on('error', fail); b.on('error', fail);
    resources.streams.push(a, b); a.pipe(b).pipe(a);
    let stableSince = null, firstProof = null;
    const samples = [];
    while (true) {
      check();
      assert(Date.now() < deadline, `Replay timed out: ${JSON.stringify({ counts, consensus: adminWriterDiagnostics(peer) })}`);
      await base.update(); await delay(0); check();
      const length = base._applyState?.view?.core?.length ?? 0;
      if (Date.now() - progressAt >= 3000) {
        progressAt = Date.now();
        onProgress({ at: new Date().toISOString(), applied_length: length, boundary: manifest.subnet.signed_length, counts });
      }
      if (length >= manifest.subnet.signed_length && idle(peer)) {
        assert(counts.nodes > 0 && counts.apply_calls > 0, 'No historical inputs were executed');
        const proof = await appliedViewProof(peer, manifest.subnet.signed_length);
        assert.equal(proof.tree_hash, manifest.subnet.tree_hash, 'Materialized canonical prefix differs');
        if (stableSince === null) { stableSince = Date.now(); firstProof = proof; }
        assert.equal(proof.fork, firstProof.fork, 'Materialized fork changed during observation');
        samples.push({ at: new Date().toISOString(), proof });
        if (Date.now() - stableSince >= stableMs) break;
      } else { stableSince = null; samples.length = 0; }
      await delay(100);
    }
    const comparison = await compareMaterializedState(base, canonical, manifest.subnet.signed_length, manifest.subnet.tree_hash, readTimeoutMs);
    const consensus = adminWriterDiagnostics(peer);
    // Persist, close, and reopen the actual apply state without a replication
    // stream. This must reproduce the same complete prefix from local storage.
    for (const stream of resources.streams) stream.destroy();
    await base.close(); base = null; await store.close();
    store = new Corestore(path.join(output, 'replayed')); resources.stores.push(store);
    protocol = new MayhemProtocol(peer, null, config); contract = new MayhemContract(protocol, config);
    peer.protocol.instance = protocol; peer.contract.instance = contract;
    await openBase(); await base.update(); check();
    const reopened = await compareMaterializedState(base, canonical, manifest.subnet.signed_length, manifest.subnet.tree_hash, readTimeoutMs);
    assert.equal(reopened.state_sha256, comparison.state_sha256, 'State changed after reopening');
    assert.equal(reopened.keys, comparison.keys);
    assert.deepEqual(await fingerprint(), candidate, 'Candidate source changed during replay');
    const report = { schema_version: 1, ok: true, started_from_empty: true, fast_forward: false,
      source_mode: network ? 'authenticated-network-replication' : 'isolated-snapshot-copy',
      captured_at: manifest.captured_at, completed_at: new Date().toISOString(), candidate,
      canonical: manifest.subnet, msb: manifest.msb, counts, comparison, reopened,
      effective_limits: { tx_max_bytes: protocol.txMaxBytes(), feature_max_bytes: protocol.featMaxBytes(),
        maxMsbSignedLength: config.maxMsbSignedLength, maxMsbSignedLengthFutureDelta: config.maxMsbSignedLengthFutureDelta,
        maxMsbApplyOperationBytes: config.maxMsbApplyOperationBytes },
      observation_ms: Date.parse(samples.at(-1).at) - Date.parse(samples[0].at),
      samples: [samples[0], samples.at(-1)], consensus };
    await fs.writeFile(path.join(output, 'result.json'), JSON.stringify(report, null, 2) + '\n', { flag: 'wx' });
    return report;
  } catch (error) {
    await fs.writeFile(path.join(output, 'failure.json'), JSON.stringify({ ok: false, message: error.message, counts }, null, 2) + '\n');
    throw error;
  } finally {
    for (const stream of resources.streams) stream.destroy();
    await Promise.allSettled(resources.swarms.map(swarm => swarm.destroy()));
    if (base) await base.close();
    await Promise.allSettled(resources.views.reverse().map(view => view.close()));
    await Promise.allSettled(resources.stores.reverse().map(store => store.close()));
  }
}

async function main() {
  const args = process.argv.slice(2);
  if (args.includes('--help')) {
    console.log('Usage: node intercom/scripts/replay-canonical-history.mjs --manifest FILE --output NEW_DIRECTORY [--network] [--timeout-ms 3600000]');
    return;
  }
  const options = {};
  for (let i = 0; i < args.length; i++) {
    if (args[i] === '--network') options.network = true;
    else {
      assert(['--manifest', '--output', '--timeout-ms'].includes(args[i]), `Unknown option ${args[i]}`);
      assert(args[i + 1], `Missing value for ${args[i]}`);
      options[args[i].slice(2)] = args[++i];
    }
  }
  assert(options.manifest && options.output, '--manifest and --output are required');
  const manifest = JSON.parse(await fs.readFile(options.manifest, 'utf8'));
  const report = await replayCanonicalHistory({ manifest, output: options.output, network: options.network,
    ...(options['timeout-ms'] ? { timeoutMs: Number(options['timeout-ms']) } : {}),
    onProgress: value => console.error(JSON.stringify(value)) });
  console.log(JSON.stringify({ ok: report.ok, result: path.resolve(options.output, 'result.json'), comparison: report.comparison, counts: report.counts }));
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch(error => { console.error(error.stack ?? error.message); process.exitCode = 1; });
}

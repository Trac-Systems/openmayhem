// Opt-in LOCAL TEST catalog. No MSB client, swarm, DHT, provider or payment service.
// The only seeded records are local genesis/admin/epoch. Catalog rows are written
// by real signed publication admission and MayhemContract applied by Autobase.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import readline from 'node:readline';
import { pathToFileURL } from 'node:url';
import nativeFs from 'fs-native-extensions';
import Autobase from 'autobase';
import Corestore from 'corestore';
import Hyperbee from 'hyperbee';
import PeerWallet from 'trac-wallet';
import MayhemContract, { CONTRACT_VERSION } from '../../contract/contract.js';
import MayhemFeature from '../../features/mayhem/index.js';
import { createServer } from '../../src/rpc.js';
import { createProxyCanonicalSnapshot } from '../../features/mayhem/proxy-canonical-view.js';
import { createProxyPublicationTransport } from '../../features/mayhem/proxy-publication-transport.js';
import { ProxyPublicationJournal, ProxyPublicationController } from '../../features/mayhem/proxy-publication-journal.js';
import { proxyPublicationFeatureKey, validateProxyPublication } from '../../contract/proxy-publication.js';
import { proxyMarketId, proxyOperationDigest, proxyOperationSigningBytes, proxyAdmissionSigningBytes } from '../../contract/proxy-protocol.js';

const fail = () => { throw new Error('Local test catalog state is unavailable or unsafe; retain its original directory.'); };
const hex = bytes => Buffer.from(bytes).toString('hex');
const random = () => crypto.randomBytes(32).toString('hex');
const exists = file => { try { fs.lstatSync(file); return true; } catch (e) { if (e.code === 'ENOENT') return false; throw e; } };
function directory(name) {
  const stat = fs.lstatSync(name);
  if (!path.isAbsolute(name) || !stat.isDirectory() || stat.isSymbolicLink() || stat.uid !== process.getuid()
    || (stat.mode & 0o077) !== 0 || fs.realpathSync(name) !== name) fail();
}
function readPrivate(file, limit = 65536) {
  const fd = fs.openSync(file, fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW | fs.constants.O_NONBLOCK);
  try {
    const stat = fs.fstatSync(fd);
    if (!stat.isFile() || stat.nlink !== 1 || stat.uid !== process.getuid() || (stat.mode & 0o077) || stat.size > limit) fail();
    const bytes = Buffer.alloc(stat.size);
    if (fs.readSync(fd, bytes, 0, bytes.length, 0) !== bytes.length) fail();
    return JSON.parse(bytes.toString('utf8'));
  } finally { fs.closeSync(fd); }
}
function createPrivate(file, value) {
  const temp = `${file}.${random()}.tmp`;
  const fd = fs.openSync(temp, 'wx', 0o600);
  try { fs.writeFileSync(fd, JSON.stringify(value)); fs.fsyncSync(fd); } finally { fs.closeSync(fd); }
  // No replacement of an existing identity or interrupted authoritative state.
  try { fs.linkSync(temp, file); } finally { fs.unlinkSync(temp); }
  const parent = fs.openSync(path.dirname(file), 'r');
  try { fs.fsyncSync(parent); } finally { fs.closeSync(parent); }
}
async function keys(root) {
  const file = path.join(root, 'identities.json');
  if (!exists(file)) {
    if (fs.readdirSync(root).some(name => name !== 'owner.lock' && !/\.tmp$/.test(name))) fail();
    const identities = {};
    for (const role of ['admin', 'provider', 'issuer']) {
      const wallet = new PeerWallet(); await wallet.ready; await wallet.generateKeyPair();
      identities[role] = { publicKey: hex(wallet.publicKey), secretKey: hex(wallet.secretKey) };
    }
    createPrivate(file, { schema_version: 1, test_only: true, msb_bootstrap: random(), identities });
  }
  const value = readPrivate(file, 4096);
  if (value.schema_version !== 1 || value.test_only !== true || !/^[a-f0-9]{64}$/.test(value.msb_bootstrap)) fail();
  const wallets = {};
  for (const role of ['admin', 'provider', 'issuer']) {
    const key = value.identities?.[role];
    if (!/^[a-f0-9]{64}$/.test(key?.publicKey) || !/^[a-f0-9]{128}$/.test(key?.secretKey)) fail();
    const wallet = await PeerWallet.fromKeyPair({ publicKey: Buffer.from(key.publicKey, 'hex'), secretKey: Buffer.from(key.secretKey, 'hex') });
    const message = Buffer.from('mayhem/local-test-catalog/key-check/v1');
    if (!PeerWallet.verify(wallet.sign(message), message, wallet.publicKey)) fail();
    wallets[role] = wallet;
  }
  return { ...value, wallets };
}
async function planFor(root, network, wallets) {
  const file = path.join(root, 'plan.json');
  if (!exists(file)) {
    const wire = JSON.parse(fs.readFileSync(new URL('../../../crates/mayhem-proto/tests/fixtures/proxy-wire-v1.json', import.meta.url)));
    const row = structuredClone(wire.cases.find(row => row.name === 'llm'));
    const provider = hex(wallets.provider.publicKey);
    row.market.creator_pubkey = provider;
    row.market.slug = 'local-test-catalog';
    row.market.model.model_id = 'LOCAL TEST ONLY / catalog acceptance';
    row.membership.market_id = await proxyMarketId(row.market);
    row.membership.provider_pubkey = provider;
    row.offer.market_id = row.membership.market_id; row.offer.provider_pubkey = provider;
    const fee = random();
    const actions = [
      { kind: 'configure', config: { ...network, enabled: true, fee_policy_hash: fee, active_issuers: [hex(wallets.issuer.publicKey)],
        max_permit_epochs: 20, max_mutations_per_provider_epoch: 8, max_mutations_per_epoch: 16,
        max_active_memberships: 1, max_created_markets_per_provider_epoch: 1, max_offer_slots_per_membership: 2 } },
      { kind: 'set_family', family_id: row.market.model.family_id, label: 'Local test only', enabled: true },
      { kind: 'set_metering', policy_hash: row.market.metering.policy_hash, policy: { enabled: true, units: row.market.metering.units } },
      ...row.market.endpoints.map(endpoint => ({ kind: 'set_endpoint', contract_hash: endpoint.contract_hash,
        policy: { enabled: true, endpoint: endpoint.endpoint, family: row.market.family, max_context: 262144,
          ctx_brackets: ['le128k', 'le256k', 'le32k', 'le8k'], outcome_classes: [''] } })),
    ];
    const policies = actions.map((action, index) => ({ op: 'proxy_policy', context: network, revision: index + 1, action }));
    const operations = [];
    for (const action of [{ kind: 'create_market', market: row.market, membership: row.membership }, { kind: 'set_offer', offer: row.offer }]) {
      const intent = { schema_version: 1, lane: 'proxy', ...network, provider_pubkey: provider, sequence: operations.length + 1, action };
      const envelope = { op: 'proxy_registry', intent, provider_signature: hex(wallets.provider.sign(proxyOperationSigningBytes(intent))), admission: null };
      if (!operations.length) {
        const permit = { ...row.permit, ...network, provider_pubkey: provider, issuer_pubkey: hex(wallets.issuer.publicKey),
          fee_policy_hash: fee, entitlement_id: random(), invoice_commitment: random(), evidence_commitment: random(), nonce: random(),
          initial_operation_digest: await proxyOperationDigest(intent), valid_from_epoch: 100, expires_after_epoch: 119 };
        envelope.admission = { permit, issuer_signature: hex(wallets.issuer.sign(proxyAdmissionSigningBytes(permit))) };
      }
      operations.push(envelope);
    }
    createPrivate(file, { schema_version: 1, test_only: true, network, policies, operations });
  }
  const plan = readPrivate(file);
  if (plan.schema_version !== 1 || plan.test_only !== true || JSON.stringify(plan.network) !== JSON.stringify(network)
    || !Array.isArray(plan.policies) || plan.policies.length < 4 || plan.policies.length > 8 || plan.operations?.length !== 2) fail();
  for (const envelope of [...plan.policies, ...plan.operations]) validateProxyPublication(envelope);
  if (plan.operations.some(operation => operation.intent.provider_pubkey !== hex(wallets.provider.publicKey))) fail();
  return plan;
}

export async function openLocalCatalog(root) {
  process.umask(0o077);
  directory(root);
  const lock = fs.openSync(path.join(root, 'owner.lock'), fs.constants.O_CREAT | fs.constants.O_RDWR | fs.constants.O_NOFOLLOW, 0o600);
  const locked = fs.fstatSync(lock);
  if (!locked.isFile() || locked.nlink !== 1 || locked.uid !== process.getuid() || (locked.mode & 0o077) || !nativeFs.tryLock(lock)) { fs.closeSync(lock); fail(); }
  let store, base, feature, controller, server;
  const close = async () => {
    if (server?.listening) { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); }
    await controller?.close(); await feature?.stop(); await base?.close(); await store?.close(); fs.closeSync(lock);
  };
  try {
    const saved = await keys(root); const wallets = saved.wallets;
    const peer = { wallet: { publicKey: hex(wallets.admin.publicKey),
      sign: message => hex(wallets.admin.sign(Buffer.isBuffer(message) ? message : Buffer.from(String(message)))),
      verify: (signature, message, key) => PeerWallet.verify(Buffer.isBuffer(signature) ? signature : Buffer.from(signature, 'hex'),
        Buffer.isBuffer(message) ? message : Buffer.from(String(message)), Buffer.isBuffer(key) ? key : Buffer.from(key, 'hex')) },
      config: {}, msbClient: { networkId: 918, bootstrapHex: saved.msb_bootstrap },
      protocol: { instance: { features: {}, featMaxBytes: () => 64000 } } };
    const contract = new MayhemContract({ peer }, {}); peer.contract = { instance: contract };
    const storePath = path.join(root, 'store');
    if (exists(storePath)) directory(storePath);
    else { if (exists(path.join(root, 'network.json'))) fail(); fs.mkdirSync(storePath, { mode: 0o700 }); }
    store = new Corestore(storePath);
    base = new Autobase(store, null, { ackInterval: 0, valueEncoding: 'json',
      open: views => new Hyperbee(views.get('view'), { extension: false, keyEncoding: 'utf-8', valueEncoding: 'json' }),
      apply: async (nodes, view) => {
        const batch = view.batch();
        try {
          for (const node of nodes) {
            if (node.value?.type === 'local-test-genesis') {
              if (await batch.get('admin') || node.value.admin !== peer.wallet.publicKey) fail();
              await batch.put('admin', peer.wallet.publicKey);
              await batch.put('epoch/apply/state', { updated_epoch: 100, pending_epoch: null });
              await batch.put('local-test/catalog', { schema_version: 1, test_only: true });
            } else if (node.value?.type === 'feature') await contract.execute(node.value, batch);
            else fail();
          }
          await batch.flush();
        } finally { await batch.close(); }
      } });
    await base.ready(); await base.update(); peer.base = base; peer.config.bootstrap = hex(base.key);
    const network = { network_id: '918', msb_bootstrap: saved.msb_bootstrap, subnet_bootstrap: hex(base.key), contract_version: CONTRACT_VERSION };
    const networkPath = path.join(root, 'network.json');
    if (!exists(networkPath)) createPrivate(networkPath, { schema_version: 1, test_only: true, network });
    const retained = readPrivate(networkPath, 4096);
    if (retained.schema_version !== 1 || retained.test_only !== true || JSON.stringify(retained.network) !== JSON.stringify(network)) fail();
    if (!base.local.length) { await base.append({ type: 'local-test-genesis', admin: peer.wallet.publicKey }); await base.update(); }
    if ((await base.view.get('admin'))?.value !== peer.wallet.publicKey || (await base.view.get('local-test/catalog'))?.value?.test_only !== true) fail();
    if (!exists(path.join(root, 'plan.json')) && base.local.length !== 1) fail();
    const plan = await planFor(root, network, wallets);
    feature = new MayhemFeature(peer, { resultTimeoutMs: 1000, resultPollMs: 10,
      withProxyCanonicalSnapshot: createProxyCanonicalSnapshot(peer, CONTRACT_VERSION) }); feature.key = 'mayhem';
    peer.protocol.instance.features.mayhem = feature;
    const transport = createProxyPublicationTransport(peer, CONTRACT_VERSION);
    const journalPath = path.join(root, 'journal');
    if (exists(journalPath)) directory(journalPath);
    const journal = await ProxyPublicationJournal.open({ directory: journalPath, identity: transport.identity(), maxEntries: 8 });
    controller = new ProxyPublicationController({ journal, ...transport, maxInFlight: 1,
      admit: (key, envelope, forward) => feature._admitProxyPublication(key, envelope, forward),
      append: entry => feature._submitFeature(entry.key, entry.envelope, { nonce: entry.nonce }),
      result: (entry, result) => feature._featureResponse(entry.key, entry.hash, entry.result_key, result) });
    await controller.step(1);
    if (journal.list().length) fail();
    const publish = async envelope => {
      const result = await controller.submit(await proxyPublicationFeatureKey(envelope), envelope);
      if (result?.ok !== true || result.status === 'pending' || journal.list().length) fail();
    };
    // Recover from the exact canonical prefix, including a crash after append
    // but before the caller observed completion. Never issue a replacement permit.
    const head = (await base.view.get('proxy/v1/policy-head'))?.value;
    const revision = head?.revision ?? 0;
    if (!Number.isSafeInteger(revision) || revision < 0 || revision > plan.policies.length
      || revision && head.operation_key !== await proxyPublicationFeatureKey(plan.policies[revision - 1])) fail();
    for (const envelope of plan.policies.slice(revision)) await publish(envelope);
    const provider = (await base.view.get(`proxy/v1/provider/${hex(wallets.provider.publicKey)}`))?.value;
    const sequence = provider?.sequence ?? 0;
    if (!Number.isSafeInteger(sequence) || sequence < 0 || sequence > plan.operations.length
      || sequence && provider.operation_digest !== await proxyOperationDigest(plan.operations[sequence - 1].intent)) fail();
    for (const envelope of plan.operations.slice(sequence)) await publish(envelope);
    server = createServer(peer);
    const [handler] = server.listeners('request'); server.removeAllListeners('request');
    server.on('request', (request, response) => {
      if (request.method !== 'POST' || request.url !== '/v1/proxy/discovery') { response.writeHead(404); response.end(); return; }
      handler(request, response);
    });
    server.requestTimeout = 5000; server.headersTimeout = 5000; server.keepAliveTimeout = 1000; server.maxConnections = 16;
    await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
    return { close, base, plan, metadata: { schema_version: 1, test_only: true, network,
      rpc_url: `http://127.0.0.1:${server.address().port}/v1`, canonical_length: base.view.core.signedLength,
      provider_sequence: 2, policy_revision: plan.policies.length, admission: 'synthetic_local_test_permit', paid_execution: false } };
  } catch (error) { await close(); throw error; }
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  let local;
  try {
    if (process.argv.length !== 4 || process.argv[2] !== '--directory') fail();
    local = await openLocalCatalog(process.argv[3]);
    process.stdout.write(`${JSON.stringify(local.metadata)}\n`); // Public identities only.
    const lines = readline.createInterface({ input: process.stdin });
    const stop = () => lines.close(); process.once('SIGTERM', stop); process.once('SIGINT', stop);
    for await (const command of lines) { if (command !== 'stop') fail(); break; }
  } catch { process.stderr.write('Local test catalog stopped: configuration, startup or state validation failed. Original state retained.\n'); process.exitCode = 1; }
  finally { if (local) await local.close(); }
}

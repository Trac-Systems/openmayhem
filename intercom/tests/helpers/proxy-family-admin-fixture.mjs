import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import Autobase from 'autobase';
import Corestore from 'corestore';
import Hyperbee from 'hyperbee';
import b4a from 'b4a';
import PeerWallet from 'trac-wallet';
import MayhemFeature from '../../features/mayhem/index.js';
import { CONTRACT_VERSION } from '../../contract/contract.js';
import { proxyContractFixture } from './proxy.js';
import { createProxyCanonicalSnapshot } from '../../features/mayhem/proxy-canonical-view.js';
import { createProxyPublicationTransport } from '../../features/mayhem/proxy-publication-transport.js';
import { ProxyPublicationJournal, ProxyPublicationController } from '../../features/mayhem/proxy-publication-journal.js';
export async function familyAdminFixture(t, { maxEntries = 16 } = {}) {
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
    feature.proxyPublicationController = controller;
    f.peer.protocol.instance.features.mayhem = feature;
    return controller;
  };
  await openController();
  t.after(async () => {
    await controller.close(); await feature.stop(); await base.close(); await store.close();
    fs.rmSync(root, { recursive: true, force: true });
  });
  return { ...f, directory, root, openController,
    get base() { return base; }, get feature() { return feature; }, get journal() { return journal; },
    get controller() { feature.proxyPublicationController = controller;
    f.peer.protocol.instance.features.mayhem = feature;
    return controller; }, get transport() { return transport; }, get calls() { return calls; },
    dropFeatures: () => { dropFeature = true; },
    reopen: async overrides => {
      await controller.close(); await feature.stop(); await base.close(); await store.close();
      await openBase(); await base.update(); await openController(overrides);
    } };
}

// Rust integration-test child only. Real registry projections, signed discovery
// and loopback RPC; all wallets/stores are ephemeral and no remote peers start.
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import readline from 'node:readline';
import b4a from 'b4a';
import Corestore from 'corestore';
import Hyperbee from 'hyperbee';
import { proxyContractFixture } from './proxy.js';
import MayhemFeature from '../../features/mayhem/index.js';
import { createProxyDiscovery } from '../../features/mayhem/proxy-discovery.js';
import { createServer } from '../../src/rpc.js';
import { CONTRACT_VERSION } from '../../contract/contract.js';

const root = await fs.mkdtemp(path.join(os.tmpdir(), 'proxy-rust-rpc-'));
const registry = await proxyContractFixture();
assert.equal((await registry.submit(await registry.create())).ok, true);
assert.equal((await registry.submit(await registry.envelope({ kind: 'set_offer', offer: registry.offer }))).ok, true);
const store = new Corestore(root);
const view = new Hyperbee(store.get({ name: 'catalog' }), { keyEncoding: 'utf-8', valueEncoding: 'json', extension: false });
await view.ready();
const write = async rows => {
  const batch = view.batch();
  try { for (const [key, value] of rows) { if (value === null) await batch.del(key); else await batch.put(key, value); } await batch.flush(); }
  finally { await batch.close(); }
};
await write([...registry.storage.values]);
const extra = Array.from({ length: 110 }, (_, i) => [`proxy/v1/catalog/families/f${String(i).padStart(3, '0')}`, { enabled: true, label: `Family ${i}` }]);
await write(extra);
const peer = { ...registry.peer,
  wallet: { publicKey: registry.admin.publicKey, sign: b => registry.admin.wallet.sign(b), verify: (...args) => registry.admin.wallet.verify(...args) },
  base: { writable: true, isIndexer: true, key: b4a.from(registry.network.subnet_bootstrap, 'hex'), view, _applyState: { view } },
};
let now = 100000;
const feature = new MayhemFeature(peer, {}); feature.key = 'mayhem';
feature.proxyDiscovery = createProxyDiscovery(peer, CONTRACT_VERSION, { now: () => now });
peer.protocol = { instance: { features: { mayhem: feature } } };
const server = createServer(peer);
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
console.log(JSON.stringify({ url: `http://127.0.0.1:${server.address().port}/v1`, identity: registry.network, market_id: registry.membership.market_id }));
try {
  const lines = readline.createInterface({ input: process.stdin });
  for await (const command of lines) {
    if (command === 'change') {
      assert.equal((await registry.submit(await registry.envelope({ kind: 'withdraw_offer', market_id: registry.offer.market_id,
        endpoint: registry.offer.endpoint, ctx_bracket: registry.offer.ctx_bracket, outcome_class: registry.offer.outcome_class, revision: 2 }))).ok, true);
      await write([...registry.storage.values]);
      await write([[extra[0][0], null], [extra[1][0], { enabled: false, label: 'Family 1' }]]);
    } else if (command === 'expire') { now += 8 * 24 * 60 * 60 * 1000; }
    else if (command === 'stop') break;
    else throw new Error('Unknown fixture command');
    console.log(JSON.stringify({ done: command }));
  }
} finally {
  await feature.stop();
  server.closeAllConnections();
  await new Promise(resolve => server.close(resolve));
  await view.close(); await store.close(); await fs.rm(root, { recursive: true, force: true });
}

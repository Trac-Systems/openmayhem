// Run with bare to verify production stream/cursor/crypto bindings, not just Node.
import fs from 'fs';
import path from 'path';
import os from 'bare-os';
import b4a from 'b4a';
import Corestore from 'corestore';
import Hyperbee from 'hyperbee';
import PeerWallet from 'trac-wallet';
import { CONTRACT_VERSION } from '../contract/contract.js';
import { createProxyDiscovery } from '../features/mayhem/proxy-discovery.js';

const check = (condition, message) => { if (!condition) throw new Error(message); };
const hex = n => n.toString(16).padStart(64, '0');
const root = await fs.promises.mkdtemp(path.join(os.tmpdir(), 'mayhem-proxy-discovery-bare-'));
const store = new Corestore(root);
const view = new Hyperbee(store.get({ name: 'view' }), { keyEncoding: 'utf-8', valueEncoding: 'json', extension: false });
const wallet = new PeerWallet();
try {
  await wallet.ready; await wallet.generateKeyPair(); await view.ready();
  const admin = b4a.toString(wallet.publicKey, 'hex');
  const batch = view.batch();
  try {
    await batch.put('admin', admin);
    await batch.put('epoch/apply/state', { epoch: 100 });
    await batch.put(`proxy/v1/catalog/markets/${hex(1)}`, { label: 'One' });
    await batch.put(`proxy/v1/catalog/markets/${hex(2)}`, { label: 'Two' });
    await batch.flush();
  } finally { await batch.close(); }
  const peer = { wallet, base: { key: b4a.from(hex(8), 'hex'), writable: true, isIndexer: true, view, _applyState: { view } },
    config: { bootstrap: hex(8) }, msbClient: { networkId: 918, bootstrapHex: hex(9) } };
  const discover = createProxyDiscovery(peer, CONTRACT_VERSION);
  const request = query => discover({ requester: admin, request_nonce: hex(7), query });
  const query = { kind: 'markets', limit: 1 };
  const first = await request(query);
  check(first.truncated && first.checkpoint === null && first.entries[0].value.label === 'One', 'first page failed');
  const second = await request({ ...query, cursor: first.next_cursor });
  check(!second.truncated && second.checkpoint && second.entries[0].value.label === 'Two', 'signed continuation failed');
  await view.del(`proxy/v1/catalog/markets/${hex(1)}`);
  const length = view.core.length;
  const changes = await request({ ...query, since: second.checkpoint });
  check(changes.entries.length === 1 && changes.entries[0].value === null && changes.mode === 'changes', 'incremental deletion failed');
  check(view.core.length === length, 'discovery appended work');
  console.log('Bare proxy discovery: signed cursors, consistent paging and incremental deletion passed without writes.');
} finally {
  await view.close(); await store.close();
  await fs.promises.rm(root, { recursive: true, force: true });
}

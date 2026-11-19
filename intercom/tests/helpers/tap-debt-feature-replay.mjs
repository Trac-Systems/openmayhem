import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import Autobase from 'autobase';
import Corestore from 'corestore';
import Hyperbee from 'hyperbee';
import Wallet from 'trac-peer/src/wallet.js';
import { FeatureOperation, FeatureCheck } from 'trac-peer/src/operations/feature/index.js';
import MayhemContract from '../../contract/contract.js';

export async function assertTapDebtNativeReplay(t, fixture, snapshot, features) {
  const ops = features.map(({ key, value }, i) => {
    const nonce = String(i + 1).padStart(64, '0');
    const hash = fixture.sign(JSON.stringify(value) + nonce);
    return { type: 'feature', key: 'mayhem_' + key, value: { dispatch: { type: 'mayhem_feature',
      contract_version: value.contract_version, key, address: fixture.admin.publicKey, value, nonce, hash } } };
  });
  const sizes = ops.map(op => Buffer.byteLength(JSON.stringify(op)));
  assert(sizes.every(n => n <= 64_000), 'Signed default-protocol feature size exceeded');
  t.diagnostic('Signed offer and finalization in bytes: ' + sizes.join(', '));
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'tap-v31-debt-'));
  const writer = new Corestore(path.join(dir, 'writer')), replica = new Corestore(path.join(dir, 'replica'));
  const options = { extension: false, keyEncoding: 'utf-8', valueEncoding: 'json' };
  const replay = new Hyperbee(replica.get({ name: 'view' }), options), wallet = new Wallet();
  const live = new MayhemContract(fixture.contract.protocol, {}), lengths = new Map();
  await wallet.ready;
  const handler = (contractInstance, canonicalView) => new FeatureOperation(new FeatureCheck(),
    { wallet, protocolInstance: { featMaxBytes: () => 64_000 }, canonicalView, contractInstance });
  const seed = async batch => { for (const [key, value] of snapshot.values) await batch.put(key, value); };
  const base = new Autobase(writer, null, { valueEncoding: 'json', ackInterval: 0,
    open: storage => new Hyperbee(storage.get('view'), options),
    async apply(nodes, view) {
      const batch = view.batch();
      try {
        for (const node of nodes) {
          if (node.value === 'seed') await seed(batch);
          else if (node.value?.type === 'feature') {
            lengths.set(node.value.key, node.length);
            await handler(live, view).handle(node.value, batch, base, node);
          }
        }
        await batch.flush();
      } finally { await batch.close(); }
    } });
  try {
    await base.ready();
    await base.append('seed');
    for (const op of ops) {
      await base.append(op);
      await base.update();
      const result = (await base.view.get('fr/' + op.value.dispatch.hash))?.value;
      assert.equal(result?.ok, true, result?.message);
    }
    await replay.ready();
    const prefix = replay.batch();
    try { await seed(prefix); await prefix.flush(); } finally { await prefix.close(); }
    const recovered = new MayhemContract(fixture.contract.protocol, {});
    for (const op of ops) {
      const batch = replay.batch();
      try {
        await handler(recovered, base.view).handle(op, batch, base, {
          value: op, from: base.local, length: lengths.get(op.key), indexed: true, optimistic: false });
        await batch.flush();
      } finally { await batch.close(); }
    }
    assert.deepEqual(await replay.core.treeHash(), await base.view.core.treeHash());
    assert.equal((await replay.get('settle/targeted/tap/1')).value.root, features.at(-1).value.checkpoint.root);
  } finally {
    await replay.close();
    await replica.close();
    await base.close();
    await writer.close();
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

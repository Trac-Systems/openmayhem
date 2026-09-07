import assert from 'node:assert/strict';
import test from 'node:test';
import b4a from 'b4a';
import { blake3 } from '@tracsystems/blake3';
import { MsbClient } from 'trac-peer/src/msbClient.js';

async function harness() {
  const indexer = b4a.from('11'.repeat(32), 'hex');
  const viewKey = b4a.from('22'.repeat(32), 'hex');
  const txv = b4a.toString(await blake3(indexer), 'hex');
  const info = { indexers: [{ key: indexer }], views: [{ key: viewKey, length: 20 }] };
  const records = new Map();
  let liveTxv = txv;
  const client = new MsbClient({ state: {
    getSignedLength: () => 30,
    getIndexerSequenceState: async () => b4a.from(liveTxv, 'hex'),
    base: {
      system: { core: { key: b4a.from('33'.repeat(32), 'hex'), signedLength: 10 },
        getIndexedInfo: async (length) => { assert.equal(length, 10); return info; } },
      view: { core: { key: viewKey }, checkout: (length) => ({
        get: async (key) => records.get(`${length}/${key}`) ?? null, close: async () => {},
      }) },
    },
  } });
  return { client, info, records, txv, setLive: (value) => { liveTxv = value; } };
}

test('payment retirement requires a signed context change and absence at both linked and latest views', async () => {
  const ctx = await harness();
  const tx = '44'.repeat(32);
  const old = '55'.repeat(32);
  const evidence = await ctx.client.getUnexecutedContextChange(tx, old);
  assert.equal(evidence.type, 'unexecuted_at_signed_context_change');
  assert.equal(evidence.tx, tx);
  assert.equal(evidence.previous_txv, old);
  assert.equal(evidence.txv, ctx.txv);
  assert.equal(evidence.linked_view_signed_length, 20);
  assert.equal(evidence.checked_view_signed_length, 30);
  ctx.records.set(`20/${tx}`, { value: b4a.from('paid') });
  assert.equal(await ctx.client.getUnexecutedContextChange(tx, old), null);
  ctx.records.clear();
  ctx.records.set(`30/${tx}`, { value: b4a.from('paid-later') });
  assert.equal(await ctx.client.getUnexecutedContextChange(tx, old), null);
});

test('unsigned rotation, unavailable linked view and unchanged context cannot retire a payment', async () => {
  const ctx = await harness();
  const tx = '44'.repeat(32);
  assert.equal(await ctx.client.getUnexecutedContextChange(tx, ctx.txv), null);
  ctx.setLive('66'.repeat(32));
  assert.equal(await ctx.client.getUnexecutedContextChange(tx, '55'.repeat(32)), null);
  ctx.setLive(ctx.txv);
  ctx.info.views[0].length = 31;
  assert.equal(await ctx.client.getUnexecutedContextChange(tx, '55'.repeat(32)), null);
  ctx.info.views[0].length = 20;
  ctx.info.views[0].key = b4a.from('77'.repeat(32), 'hex');
  assert.equal(await ctx.client.getUnexecutedContextChange(tx, '55'.repeat(32)), null);
});

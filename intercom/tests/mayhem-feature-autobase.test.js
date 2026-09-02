import assert from 'node:assert/strict';
import crypto from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import Autobase from 'autobase';
import Corestore from 'corestore';
import Hyperbee from 'hyperbee';

import MayhemFeature from '../features/mayhem/index.js';

const waitFor = async (predicate, label, timeoutMs = 2_000) => {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
  throw new Error(`Timed out waiting for ${label}.`);
};

test('canonical feature completion needs only one append on the pinned Autobase stack', async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-feature-autobase-'));
  const store = new Corestore(root);
  const admin = 'ab'.repeat(32);
  let featureAppends = 0;
  let syntheticAckAppends = 0;
  const base = new Autobase(store, null, {
    ackInterval: 0,
    valueEncoding: 'json',
    open(viewStore) {
      return new Hyperbee(viewStore.get('view'), {
        extension: false,
        keyEncoding: 'utf-8',
        valueEncoding: 'json',
      });
    },
    async apply(nodes, view) {
      const batch = view.batch();
      for (const node of nodes) {
        const operation = node.value;
        if (operation?.type === 'test_init') {
          await batch.put('admin', admin);
          continue;
        }
        if (operation?.type === '_trac_peer_ack_v1') {
          syntheticAckAppends += 1;
          continue;
        }
        if (operation?.type !== 'feature') continue;
        featureAppends += 1;
        const dispatch = operation.value.dispatch;
        await batch.put(`fr/${dispatch.hash}`, {
          type: 'feature_result',
          status: 'applied',
          ok: true,
          result: { ok: true, op: dispatch.value.op },
        });
      }
      await batch.flush();
      await batch.close();
    },
  });

  try {
    await base.ready();
    await base.append({ type: 'test_init' });
    await waitFor(async () => {
      await base.update();
      return (await base.view.get('admin'))?.value === admin;
    }, 'admin initialization');

    const peer = {
      wallet: {
        publicKey: admin,
        sign(message) {
          return crypto.createHash('sha512').update(String(message)).digest('hex');
        },
      },
      base,
      protocol: { instance: { featMaxBytes: () => 256 * 1024 } },
    };
    const feature = new MayhemFeature(peer, {
      resultTimeoutMs: 2_000,
      resultPollMs: 5,
    });
    feature.key = 'mayhem';

    const result = await feature.submit(`consent/${admin}/1/rules-hash`, {
      op: 'consent',
      sender: admin,
      ver: 1,
      hash: 'cd'.repeat(32),
      sig: 'ef'.repeat(64),
    });

    assert.equal(result.ok, true);
    assert.equal(result.status, 'applied');
    assert.equal(featureAppends, 1);
    assert.equal(syntheticAckAppends, 0);
  } finally {
    await base.close().catch(() => {});
    await store.close().catch(() => {});
    fs.rmSync(root, { recursive: true, force: true });
  }
});

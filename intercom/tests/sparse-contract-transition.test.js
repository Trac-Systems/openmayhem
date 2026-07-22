import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import Autobase from 'autobase';
import Corestore from 'corestore';
import Hyperbee from 'hyperbee';

import {
  CONTRACT_VERSION,
  validateMayhemOperationContractVersion,
} from '../contract/contract.js';
import MayhemProtocol from '../contract/protocol.js';

const self = fileURLToPath(import.meta.url);
const stage = process.env.MAYHEM_SPARSE_TRANSITION_STAGE || '';
const stageRoot = process.env.MAYHEM_SPARSE_TRANSITION_ROOT || '';
const stageBootstrap = process.env.MAYHEM_SPARSE_TRANSITION_BOOTSTRAP || '';

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const operation = (contractVersion, kind, fields = {}) => ({
  type: 'feature',
  key: `mayhem_${kind}`,
  value: {
    dispatch: {
      type: 'mayhem_feature',
      contract_version: contractVersion,
      key: kind,
      value: { kind, ...fields },
    },
  },
});

const openBase = async (storePath, bootstrap, expectedVersion) => {
  const store = new Corestore(storePath);
  const base = new Autobase(store, bootstrap, {
    valueEncoding: 'json',
    ackInterval: 0,
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
        if (!node.value) continue;
        const normalized = validateMayhemOperationContractVersion(
          node.value,
          expectedVersion
        );
        const value = normalized.value.dispatch.value;
        if (value.kind === 'seed') {
          await batch.put('state/seed', value.value);
        } else if (value.kind === 'new-model') {
          await batch.put(`model/${value.id}`, value.model);
        }
      }
      await batch.flush();
      await batch.close();
    },
  });
  await base.ready();
  return { base, store };
};

const connect = (left, right) => {
  const outbound = left.store.replicate(true);
  const inbound = right.store.replicate(false);
  outbound.pipe(inbound).pipe(outbound);
};

const valueAt = async (peer, key) => (await peer.base.view.get(key))?.value ?? null;
const treeHash = async (peer) => (await peer.base.view.core.treeHash()).toString('hex');

const waitFor = async (predicate, label, timeoutMs = 10_000) => {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await predicate()) return;
    await delay(25);
  }
  throw new Error(`Timed out waiting for ${label}.`);
};

const reportAndExit = (value) => {
  process.stdout.write(`${JSON.stringify(value)}\n`);
  process.exit(0);
};

const runStage = async () => {
  const bootstrap = stageBootstrap ? Buffer.from(stageBootstrap, 'hex') : null;
  const writerPath = path.join(stageRoot, 'writer');
  const readerPath = path.join(stageRoot, 'reader');

  if (stage === 'seed-v1') {
    const writer = await openBase(writerPath, null, 1);
    const reader = await openBase(readerPath, writer.base.key, 1);
    connect(writer, reader);
    await writer.base.append(operation(1, 'seed', { value: 'ok' }));
    await writer.base.update();
    await waitFor(async () => {
      await reader.base.update();
      return await valueAt(reader, 'state/seed') === 'ok';
    }, 'the initial sparse view');
    reportAndExit({
      bootstrap: writer.base.key.toString('hex'),
      writer_tree_hash: await treeHash(writer),
      reader_tree_hash: await treeHash(reader),
    });
  }

  if (stage === 'writer-v2-reader-v1') {
    const writer = await openBase(writerPath, bootstrap, 2);
    const reader = await openBase(readerPath, bootstrap, 1);
    connect(writer, reader);
    await writer.base.append(operation(2, 'new-model', {
      id: 'bonsai',
      model: { backend: 'prism.cpp' },
    }));
    await writer.base.update();
    await reader.base.update();
    throw new Error('The old reader incorrectly accepted a future contract operation.');
  }

  if (stage === 'writer-v2-reader-v2') {
    const writer = await openBase(writerPath, bootstrap, 2);
    const reader = await openBase(readerPath, bootstrap, 2);
    connect(writer, reader);
    await waitFor(async () => {
      await reader.base.update();
      return await valueAt(reader, 'model/bonsai') !== null;
    }, 'the upgraded persisted sparse reader');
    reportAndExit({
      model: await valueAt(reader, 'model/bonsai'),
      writer_tree_hash: await treeHash(writer),
      reader_tree_hash: await treeHash(reader),
    });
  }

  throw new Error(`Unknown sparse transition stage ${stage}.`);
};

if (stage) await runStage();

const runChild = (name, root, bootstrap = '', expectFailure = false) => {
  const result = spawnSync(process.execPath, [self], {
    cwd: process.cwd(),
    encoding: 'utf8',
    timeout: 15_000,
    env: {
      ...process.env,
      MAYHEM_SPARSE_TRANSITION_STAGE: name,
      MAYHEM_SPARSE_TRANSITION_ROOT: root,
      MAYHEM_SPARSE_TRANSITION_BOOTSTRAP: bootstrap,
    },
  });
  if (result.error) throw result.error;
  if (expectFailure) {
    assert.notEqual(result.status, 0);
    return `${result.stderr}\n${result.stdout}`;
  }
  assert.equal(result.status, 0, result.stderr || result.stdout);
  return JSON.parse(result.stdout.trim());
};

test('versioned Mayhem operations strip the envelope before strict contract schemas', () => {
  const feature = operation(CONTRACT_VERSION, 'seed', { value: 'ok' });
  const normalizedFeature = validateMayhemOperationContractVersion(feature);
  assert.equal(normalizedFeature.value.dispatch.contract_version, undefined);
  assert.equal(normalizedFeature.value.dispatch.value.value, 'ok');

  const protocol = Object.create(MayhemProtocol.prototype);
  const tx = protocol.versionedTransactionObject({
    type: 'setRules',
    value: { op: 'set_rules', ver: 1, hash: 'aa'.repeat(32) },
  });
  assert.equal(tx.value.contract_version, CONTRACT_VERSION);
  const normalizedTx = validateMayhemOperationContractVersion({
    type: 'tx',
    value: { dispatch: tx },
  });
  assert.equal(normalizedTx.value.dispatch.value.contract_version, undefined);
  assert.equal(normalizedTx.value.dispatch.value.op, 'set_rules');
});

test('a sparse reader stops before contract drift and resumes from the same store after upgrade', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-sparse-transition-'));
  try {
    const seeded = runChild('seed-v1', root);
    assert.equal(seeded.reader_tree_hash, seeded.writer_tree_hash);

    const rejected = runChild(
      'writer-v2-reader-v1',
      root,
      seeded.bootstrap,
      true
    );
    assert.match(rejected, /Contract upgrade required: expected CONTRACT_VERSION 1, got 2/);

    const recovered = runChild('writer-v2-reader-v2', root, seeded.bootstrap);
    assert.deepEqual(recovered.model, { backend: 'prism.cpp' });
    assert.equal(recovered.reader_tree_hash, recovered.writer_tree_hash);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

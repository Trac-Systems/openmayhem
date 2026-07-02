import assert from 'node:assert/strict';
import test from 'node:test';
import MayhemContract from '../contract/contract.js';
import {
  MemoryStorage,
  execute,
  makeIdentity,
  makeTxKey,
  makeVerifier,
  signConsent,
} from './helpers/contract.js';

const rulesHash = '1'.repeat(64);
const enclaveId = '2'.repeat(64);
const manifestHash = '3'.repeat(64);
const binaryHash = '4'.repeat(64);
const artifactRoot = '5'.repeat(64);
const updatedArtifactRoot = '6'.repeat(64);

const providerRegistration = {
  op: 'register_provider',
  payout_addr: 'trac1providerpayouttarget',
  payout_method: 'tnk',
};

const enclaveRegistration = {
  op: 'register_enclave',
  enclave_id: enclaveId,
  model_id: 'qwen/qwen2.5-4b-instruct@4bit',
  backend: 'llama.cpp',
  artifact_root: artifactRoot,
  manifest_hash: manifestHash,
  att_tier: 1,
  binary_hash: binaryHash,
  caps: {
    chat: true,
    embeddings: false,
    tools: false,
    ctx: 32768,
  },
  rooms: ['mx/room/qwen2.5-4b-q4'],
};

const enclaveUpdate = {
  op: 'update_enclave',
  enclave_id: enclaveId,
  artifact_root: updatedArtifactRoot,
  caps: {
    chat: true,
    embeddings: false,
    tools: true,
    ctx: 32768,
  },
  rooms: ['mx/room/qwen2.5-4b-q4', 'mx/room/qwen2.5-4b-q4-fast'],
};

const enclaveRetire = {
  op: 'retire_enclave',
  enclave_id: enclaveId,
};

const buildRegistryLog = (provider, feePayer) => [
  {
    type: 'setRules',
    value: { op: 'set_rules', ver: 1, hash: rulesHash },
    sender: provider.publicKey,
    txNo: 1,
  },
  {
    type: 'consent',
    value: {
      op: 'consent',
      ver: 1,
      hash: rulesHash,
      sig: signConsent(provider.wallet, 1, rulesHash),
    },
    sender: provider.publicKey,
    txNo: 2,
  },
  {
    type: 'registerProvider',
    value: providerRegistration,
    sender: provider.publicKey,
    writer: feePayer.publicKey,
    txNo: 3,
  },
  {
    type: 'registerEnclave',
    value: enclaveRegistration,
    sender: provider.publicKey,
    txNo: 4,
  },
  {
    type: 'updateEnclave',
    value: enclaveUpdate,
    sender: provider.publicKey,
    txNo: 5,
  },
  {
    type: 'retireEnclave',
    value: enclaveRetire,
    sender: provider.publicKey,
    txNo: 6,
  },
];

async function applyLog(provider, opLog) {
  const storage = new MemoryStorage({ admin: provider.publicKey });
  const protocol = { peer: { wallet: makeVerifier(provider.wallet) } };
  const contract = new MayhemContract(protocol, {});
  const results = [];

  for (const op of opLog) {
    results.push(
      await execute(contract, storage, op.type, op.value, op.sender, op.txNo, op.writer)
    );
  }

  return { storage, results };
}

test('MayhemContract registry op log replays to byte-identical state', async () => {
  const provider = await makeIdentity();
  const feePayer = await makeIdentity();
  const opLog = buildRegistryLog(provider, feePayer);
  assert.notEqual(feePayer.publicKey, provider.publicKey);

  const first = await applyLog(provider, opLog);
  const second = await applyLog(provider, opLog);

  for (const result of first.results) {
    assert.equal(result.ok, true, result.message);
  }

  assert.equal(first.storage.snapshotBytes(), second.storage.snapshotBytes());

  const providerEntry = await first.storage.get(`prov/${provider.publicKey}`);
  assert.deepEqual(providerEntry.value, {
    provider: provider.publicKey,
    payout: {
      addr: providerRegistration.payout_addr,
      method: 'tnk',
    },
    status: 'active',
    probation: {
      since: makeTxKey(3),
      successful_sessions: 0,
    },
    registered_at: makeTxKey(3),
    updated_at: makeTxKey(3),
  });

  const enclaveEntry = await first.storage.get(`enclave/${enclaveId}`);
  assert.equal(enclaveEntry.value.provider, provider.publicKey);
  assert.equal(enclaveEntry.value.status, 'retired');
  assert.equal(enclaveEntry.value.artifact_root, updatedArtifactRoot);
  assert.deepEqual(enclaveEntry.value.rooms, enclaveUpdate.rooms);
  assert.equal(enclaveEntry.value.registered_at, makeTxKey(4));
  assert.equal(enclaveEntry.value.updated_at, makeTxKey(6));
  assert.equal(enclaveEntry.value.retired_at, makeTxKey(6));
});

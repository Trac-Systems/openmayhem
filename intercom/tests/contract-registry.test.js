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
};

const enclaveRetire = {
  op: 'retire_enclave',
  enclave_id: enclaveId,
};

const providerJoin = {
  op: 'join_enclave',
  enclave_id: enclaveId,
};

const buildRegistryLog = (admin, provider, feePayer) => [
  {
    type: 'setRules',
    value: { op: 'set_rules', ver: 1, hash: rulesHash },
    sender: admin.publicKey,
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
    sender: admin.publicKey,
    txNo: 4,
  },
  {
    type: 'joinEnclave',
    value: providerJoin,
    sender: provider.publicKey,
    txNo: 5,
  },
  {
    type: 'updateEnclave',
    value: enclaveUpdate,
    sender: admin.publicKey,
    txNo: 6,
  },
  {
    type: 'retireEnclave',
    value: enclaveRetire,
    sender: admin.publicKey,
    txNo: 7,
  },
];

async function applyLog(admin, provider, opLog) {
  const storage = new MemoryStorage({ admin: admin.publicKey });
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
  const admin = await makeIdentity();
  const provider = await makeIdentity();
  const feePayer = await makeIdentity();
  const opLog = buildRegistryLog(admin, provider, feePayer);
  assert.notEqual(feePayer.publicKey, provider.publicKey);

  const first = await applyLog(admin, provider, opLog);
  const second = await applyLog(admin, provider, opLog);

  for (const result of first.results) {
    assert.equal(result.ok, true, result.message);
  }

  assert.equal(first.storage.snapshotBytes(), second.storage.snapshotBytes());

  const providerEntry = await first.storage.get(`prov/${provider.publicKey}`);
  assert.deepEqual(providerEntry.value, {
    provider: provider.publicKey,
    payout: null,
    status: 'active',
    probation: {
      since: makeTxKey(3),
      since_seconds: 0,
      successful_sessions: 0,
    },
    registered_at: makeTxKey(3),
    updated_at: makeTxKey(3),
  });

  const enclaveEntry = await first.storage.get(`enclave/${enclaveId}`);
  assert.equal(enclaveEntry.value.status, 'retired');
  assert.equal(enclaveEntry.value.created_by, admin.publicKey);
  assert.equal(enclaveEntry.value.artifact_root, updatedArtifactRoot);
  assert.equal(enclaveEntry.value.registered_at, makeTxKey(4));
  assert.equal(enclaveEntry.value.updated_at, makeTxKey(7));
  assert.equal(enclaveEntry.value.retired_at, makeTxKey(7));

  const servingEntry = await first.storage.get(`serve/${provider.publicKey}/${enclaveId}`);
  assert.deepEqual(servingEntry.value, {
    provider: provider.publicKey,
    enclave_id: enclaveId,
    model_id: enclaveRegistration.model_id,
    status: 'active',
    joined_at: makeTxKey(5),
    updated_at: makeTxKey(5),
    left_at: null,
    rooms: [],
  });
});

test('MayhemContract ignores provider-authored payout and probation hints', async () => {
  const admin = await makeIdentity();
  const provider = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const protocol = { peer: { wallet: makeVerifier(provider.wallet) } };
  const contract = new MayhemContract(protocol, {});

  for (const op of [
    {
      type: 'setRules',
      value: { op: 'set_rules', ver: 1, hash: rulesHash },
      sender: admin.publicKey,
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
      value: {
        op: 'register_provider',
        payout_addr: 'provider-picked-target',
        payout_method: 'coinbase',
        registered_at_seconds: 123_456,
      },
      sender: provider.publicKey,
      txNo: 3,
    },
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }

  const providerEntry = await storage.get(`prov/${provider.publicKey}`);
  assert.equal(providerEntry.value.payout, null);
  assert.equal(providerEntry.value.probation.since_seconds, 0);

  const providerPayout = await execute(
    contract,
    storage,
    'setProviderPayout',
    {
      op: 'set_provider_payout',
      provider: provider.publicKey,
      payout_addr: 'provider-picked-target',
      payout_method: 'tnk',
    },
    provider.publicKey,
    4
  );
  assert.match(providerPayout.message, /admin required/i);

  const unsupported = await execute(
    contract,
    storage,
    'setProviderPayout',
    {
      op: 'set_provider_payout',
      provider: provider.publicKey,
      payout_addr: 'admin-approved-target',
      payout_method: 'wire',
    },
    admin.publicKey,
    5
  );
  assert.match(unsupported.message, /unsupported payout method/i);

  const adminPayout = await execute(
    contract,
    storage,
    'setProviderPayout',
    {
      op: 'set_provider_payout',
      provider: provider.publicKey,
      payout_addr: 'admin-approved-target',
      payout_method: 'tnk',
    },
    admin.publicKey,
    6
  );
  assert.deepEqual(adminPayout, {
    ok: true,
    op: 'setProviderPayout',
    provider: provider.publicKey,
  });

  const updated = await storage.get(`prov/${provider.publicKey}`);
  assert.deepEqual(updated.value.payout, {
    addr: 'admin-approved-target',
    method: 'tnk',
    set_by: admin.publicKey,
    set_at: makeTxKey(6),
  });
  assert.equal(updated.value.updated_at, makeTxKey(6));
});

test('MayhemContract admin can ban providers from future serving mutations', async () => {
  const admin = await makeIdentity();
  const provider = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const protocol = { peer: { wallet: makeVerifier(provider.wallet) } };
  const contract = new MayhemContract(protocol, {});

  for (const op of [
    {
      type: 'setRules',
      value: { op: 'set_rules', ver: 1, hash: rulesHash },
      sender: admin.publicKey,
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
      txNo: 3,
    },
    {
      type: 'registerEnclave',
      value: enclaveRegistration,
      sender: admin.publicKey,
      txNo: 4,
    },
    {
      type: 'joinEnclave',
      value: providerJoin,
      sender: provider.publicKey,
      txNo: 5,
    },
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }

  const banned = await execute(
    contract,
    storage,
    'banProvider',
    {
      op: 'ban_provider',
      provider: provider.publicKey,
      reason_hash: '7'.repeat(64),
    },
    admin.publicKey,
    6
  );
  assert.deepEqual(banned, {
    ok: true,
    op: 'banProvider',
    provider: provider.publicKey,
  });

  const providerEntry = await storage.get(`prov/${provider.publicKey}`);
  assert.equal(providerEntry.value.status, 'banned');
  assert.equal(providerEntry.value.banned_by, admin.publicKey);
  assert.equal(providerEntry.value.banned_at, makeTxKey(6));
  assert.equal(providerEntry.value.ban_reason_hash, '7'.repeat(64));

  const joinAfterBan = await execute(
    contract,
    storage,
    'joinEnclave',
    providerJoin,
    provider.publicKey,
    7
  );
  assert.match(joinAfterBan.message, /provider registration required/i);
});

test('MayhemContract rejects provider-submitted arbitrary enclave definitions', async () => {
  const admin = await makeIdentity();
  const provider = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const protocol = { peer: { wallet: makeVerifier(provider.wallet) } };
  const contract = new MayhemContract(protocol, {});

  await execute(
    contract,
    storage,
    'setRules',
    { op: 'set_rules', ver: 1, hash: rulesHash },
    admin.publicKey,
    1
  );
  await execute(
    contract,
    storage,
    'consent',
    {
      op: 'consent',
      ver: 1,
      hash: rulesHash,
      sig: signConsent(provider.wallet, 1, rulesHash),
    },
    provider.publicKey,
    2
  );
  await execute(
    contract,
    storage,
    'registerProvider',
    providerRegistration,
    provider.publicKey,
    3
  );

  const providerEnclave = await execute(
    contract,
    storage,
    'registerEnclave',
    enclaveRegistration,
    provider.publicKey,
    4
  );
  assert.match(providerEnclave.message, /admin required/i);
});

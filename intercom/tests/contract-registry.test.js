import assert from 'node:assert/strict';
import test from 'node:test';
import MayhemContract, { deriveRoomId } from '../contract/contract.js';
import {
  MemoryStorage,
  execute,
  executeFeature,
  makeIdentity,
  makeTxKey,
  makeVerifier,
  providerLifecycleFeatureKey,
  seedCurrentAdminPrice,
  signConsent,
  signProviderKyb,
  signProviderLifecycleIntent,
} from './helpers/contract.js';

const rulesHash = '1'.repeat(64);
const enclaveId = '2'.repeat(64);
const manifestHash = '3'.repeat(64);
const binaryHash = '4'.repeat(64);
const artifactRoot = '5'.repeat(64);
const updatedArtifactRoot = '6'.repeat(64);
const artifactSource = {
  kind: 'huggingface',
  repo: 'mayhem-catalog/qwen2.5-4b-instruct-GGUF',
  revision: '7'.repeat(40),
  path: 'qwen2.5-4b-instruct-Q4_K_M.gguf',
};

const providerRegistration = {
  op: 'register_provider',
};

const enclaveRegistration = {
  op: 'register_enclave',
  enclave_id: enclaveId,
  model_id: 'qwen/qwen2.5-4b-instruct@4bit',
  backend: 'llama.cpp',
  artifact_root: artifactRoot,
  artifact_root_kind: 'blake3_merkle_v1',
  artifact_source: artifactSource,
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
  artifact_source: {
    ...artifactSource,
    path: 'qwen2.5-4b-instruct-Q4_K_M.v2.gguf',
  },
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

const modelRef = {
  op: 'set_model_ref',
  model_id: enclaveRegistration.model_id,
  price_ref_mu: {
    in_per_1k: 20,
    out_per_1k: 60,
  },
};

const catalogRelease = {
  op: 'publish_catalog',
  catalog_id: 'mayhem-models',
  source_kind: 'huggingface',
  catalog_url: 'https://huggingface.co/TracNetwork/mayhem-catalog/resolve/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/models.json',
  signature_url: 'https://huggingface.co/TracNetwork/mayhem-catalog/resolve/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/models.json.sig',
  catalog_hash: '8'.repeat(64),
  signature_hash: '9'.repeat(64),
  key_id: 'mayhem-catalog-tracnetwork-v1',
  public_key: 'a'.repeat(64),
  model_count: 3,
  artifact_count: 5,
  canaries: [{
    set_id: 'canary-launch-v1',
    url: 'https://huggingface.co/TracNetwork/mayhem-catalog/resolve/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/canaries/canary-launch-v1.json',
    hash: 'b'.repeat(64),
  }],
};

const priceSchedule = {
  op: 'set_price',
  enclave_id: enclaveId,
  in_per_1k_mu: 20,
  out_per_1k_mu: 60,
  per_req_mu: 0,
  min_session_mu: 0,
  effective_at: 0,
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
    type: 'setModelRef',
    value: modelRef,
    sender: admin.publicKey,
    txNo: 5,
  },
  {
    type: 'publishCatalog',
    value: catalogRelease,
    sender: admin.publicKey,
    txNo: 6,
  },
  {
    type: 'setPrice',
    value: priceSchedule,
    sender: admin.publicKey,
    txNo: 7,
  },
  {
    type: 'joinEnclave',
    value: providerJoin,
    sender: provider.publicKey,
    txNo: 8,
  },
  {
    type: 'updateEnclave',
    value: enclaveUpdate,
    sender: admin.publicKey,
    txNo: 9,
  },
  {
    type: 'retireEnclave',
    value: enclaveRetire,
    sender: admin.publicKey,
    txNo: 10,
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
    enclaves: [],
    probation: {
      since: makeTxKey(3),
      since_seconds: 0,
      successful_sessions: 0,
    },
    registered_at: makeTxKey(3),
    updated_at: makeTxKey(10),
  });

  const catalogEntry = await first.storage.get('catalog/current');
  assert.equal(catalogEntry.value.catalog_hash, catalogRelease.catalog_hash);
  assert.equal(catalogEntry.value.signature_hash, catalogRelease.signature_hash);
  assert.equal(catalogEntry.value.published_by, admin.publicKey);
  assert.equal(catalogEntry.value.published_by_role, 'admin');
  assert.equal(catalogEntry.value.ver, 1);
  assert.deepEqual(catalogEntry.value.canaries, catalogRelease.canaries);
  const catalogReleaseEntry = await first.storage.get(`catalog/release/${catalogRelease.catalog_hash}`);
  assert.deepEqual(catalogReleaseEntry.value, catalogEntry.value);

  const enclaveEntry = await first.storage.get(`enclave/${enclaveId}`);
  assert.equal(enclaveEntry.value.status, 'retired');
  assert.equal(enclaveEntry.value.created_by, admin.publicKey);
  assert.equal(enclaveEntry.value.created_by_role, 'admin');
  assert.equal(enclaveEntry.value.model_class, 'text-generation');
  assert.equal(enclaveEntry.value.artifact_root, updatedArtifactRoot);
  assert.equal(enclaveEntry.value.artifact_root_kind, 'blake3_merkle_v1');
  assert.deepEqual(enclaveEntry.value.artifact_source, enclaveUpdate.artifact_source);
  assert.equal(enclaveEntry.value.source_sha256, null);
  assert.equal(enclaveEntry.value.registered_at, makeTxKey(4));
  assert.equal(enclaveEntry.value.updated_by, admin.publicKey);
  assert.equal(enclaveEntry.value.updated_by_role, 'admin');
  assert.equal(enclaveEntry.value.updated_at, makeTxKey(10));
  assert.equal(enclaveEntry.value.retired_at, makeTxKey(10));
  assert.equal(enclaveEntry.value.retired_by, admin.publicKey);
  assert.equal(enclaveEntry.value.retired_by_role, 'admin');
  assert.deepEqual(enclaveEntry.value.providers, []);
  assert.deepEqual(enclaveEntry.value.tombstoned_providers, [provider.publicKey]);

  const servingEntry = await first.storage.get(`serve/${provider.publicKey}/${enclaveId}`);
  assert.deepEqual(servingEntry.value, {
    provider: provider.publicKey,
    enclave_id: enclaveId,
    model_id: enclaveRegistration.model_id,
    status: 'tombstoned',
    joined_at: makeTxKey(8),
    updated_at: makeTxKey(10),
    left_at: null,
    tombstoned_at: makeTxKey(10),
    tombstone_reason_hash: null,
    rooms: [],
  });
});

test('MayhemContract model_class defaults old text records and allows new admin classes', async () => {
  const admin = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const protocol = { peer: { wallet: makeVerifier(admin.wallet) } };
  const contract = new MayhemContract(protocol, {});
  const legacyEnclaveId = 'b'.repeat(64);

  await storage.put(`enclave/${legacyEnclaveId}`, {
    enclave_id: legacyEnclaveId,
    model_id: enclaveRegistration.model_id,
    backend: 'llama.cpp',
    artifact_root: artifactRoot,
    artifact_root_kind: 'blake3_merkle_v1',
    artifact_source: artifactSource,
    manifest_hash: manifestHash,
    att_tier: 1,
    binary_hash: binaryHash,
    caps: {
      chat: true,
      tools: false,
      ctx: 32768,
    },
    status: 'active',
    providers: [],
    created_by: admin.publicKey,
    created_by_role: 'admin',
    registered_at: makeTxKey(1),
    updated_at: makeTxKey(1),
    retired_at: null,
  });
  await storage.put(`modelref/${enclaveRegistration.model_id}`, {
    model_id: enclaveRegistration.model_id,
    price_ref_mu: {
      in_per_1k: 20,
      out_per_1k: 60,
    },
  });

  const legacyPrice = await execute(
    contract,
    storage,
    'setPrice',
    {
      ...priceSchedule,
      enclave_id: legacyEnclaveId,
    },
    admin.publicKey,
    2
  );
  assert.equal(legacyPrice.ok, true, legacyPrice.message);

  const embeddingRegistration = {
    ...enclaveRegistration,
    enclave_id: 'c'.repeat(64),
    model_id: 'admin/embed-small@fp16',
    model_class: 'embedding',
    artifact_root: 'd'.repeat(64),
    manifest_hash: 'e'.repeat(64),
    binary_hash: 'f'.repeat(64),
    caps: {
      embeddings: true,
      ctx: 8192,
    },
  };
  const registered = await execute(
    contract,
    storage,
    'registerEnclave',
    embeddingRegistration,
    admin.publicKey,
    3
  );
  assert.equal(registered.ok, true, registered.message);
  assert.equal((await storage.get(`enclave/${embeddingRegistration.enclave_id}`)).value.model_class, 'embedding');

  const modelRefResult = await execute(
    contract,
    storage,
    'setModelRef',
    {
      op: 'set_model_ref',
      model_id: embeddingRegistration.model_id,
      model_class: 'embedding',
      price_ref_mu: {
        in_per_1k: 2,
        out_per_1k: 2,
      },
    },
    admin.publicKey,
    4
  );
  assert.equal(modelRefResult.ok, true, modelRefResult.message);
  assert.equal((await storage.get(`modelref/${embeddingRegistration.model_id}`)).value.model_class, 'embedding');
});

test('MayhemContract rejects unsupported model classes and mismatched model references', async () => {
  const admin = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const protocol = { peer: { wallet: makeVerifier(admin.wallet) } };
  const contract = new MayhemContract(protocol, {});

  const unsupportedEnclave = await execute(
    contract,
    storage,
    'registerEnclave',
    {
      ...enclaveRegistration,
      enclave_id: 'b'.repeat(64),
      model_class: 'provider-defined-magic',
    },
    admin.publicKey,
    1
  );
  assert.match(unsupportedEnclave.message, /unsupported enclave model_class/i);

  const unsupportedModelRef = await execute(
    contract,
    storage,
    'setModelRef',
    {
      ...modelRef,
      model_class: 'provider-defined-magic',
    },
    admin.publicKey,
    2
  );
  assert.match(unsupportedModelRef.message, /unsupported model reference model_class/i);

  const registered = await execute(
    contract,
    storage,
    'registerEnclave',
    {
      ...enclaveRegistration,
      enclave_id: 'c'.repeat(64),
      model_class: 'embedding',
    },
    admin.publicKey,
    3
  );
  assert.equal(registered.ok, true, registered.message);
  await storage.put(`modelref/${enclaveRegistration.model_id}`, {
    model_id: enclaveRegistration.model_id,
    model_class: 'text-generation',
    price_ref_mu: {
      in_per_1k: 20,
      out_per_1k: 60,
    },
  });

  const mismatchedPrice = await execute(
    contract,
    storage,
    'setPrice',
    {
      ...priceSchedule,
      enclave_id: 'c'.repeat(64),
    },
    admin.publicKey,
    4
  );
  assert.match(mismatchedPrice.message, /model_class must match/i);
});

test('MayhemContract requires Hugging Face catalog anchors to use pinned revisions', async () => {
  const admin = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const protocol = { peer: { wallet: makeVerifier(admin.wallet) } };
  const contract = new MayhemContract(protocol, {});
  const mutableRelease = {
    ...catalogRelease,
    catalog_url: 'https://huggingface.co/TracNetwork/mayhem-catalog/resolve/main/models.json',
  };

  const result = await execute(
    contract,
    storage,
    'publishCatalog',
    mutableRelease,
    admin.publicKey,
    1
  );

  assert.ok(result instanceof Error);
  assert.match(result.message, /40-hex-revision/i);
  assert.equal(await storage.get('catalog/current'), null);
});

test('MayhemContract requires a current admin price before provider serving rows', async () => {
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
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }

  const unpricedJoin = await execute(
    contract,
    storage,
    'joinEnclave',
    providerJoin,
    provider.publicKey,
    5
  );
  assert.match(unpricedJoin.message, /current admin price required/i);
  assert.equal(await storage.get(`serve/${provider.publicKey}/${enclaveId}`), null);

  await seedCurrentAdminPrice(storage, {
    enclaveId,
    modelId: enclaveRegistration.model_id,
    admin: admin.publicKey,
    txNo: 6,
  });
  let priceScheduleEntry = await storage.get(`price/${enclaveId}`);
  priceScheduleEntry.value.current.set_by = provider.publicKey;
  delete priceScheduleEntry.value.current.set_by_role;
  await storage.put(`price/${enclaveId}`, priceScheduleEntry.value);
  const providerSetPriceJoin = await execute(
    contract,
    storage,
    'joinEnclave',
    providerJoin,
    provider.publicKey,
    6
  );
  assert.match(providerSetPriceJoin.message, /current price set by the current admin/i);
  assert.equal(await storage.get(`serve/${provider.publicKey}/${enclaveId}`), null);

  priceScheduleEntry = await storage.get(`price/${enclaveId}`);
  priceScheduleEntry.value.current.set_by = admin.publicKey;
  delete priceScheduleEntry.value.current.set_by_role;
  await storage.put(`price/${enclaveId}`, priceScheduleEntry.value);
  const missingRolePriceJoin = await execute(
    contract,
    storage,
    'joinEnclave',
    providerJoin,
    provider.publicKey,
    7
  );
  assert.match(missingRolePriceJoin.message, /admin-set enclave price/i);
  assert.equal(await storage.get(`serve/${provider.publicKey}/${enclaveId}`), null);

  priceScheduleEntry = await storage.get(`price/${enclaveId}`);
  priceScheduleEntry.value.current.set_by_role = 'admin';
  await storage.put(`price/${enclaveId}`, priceScheduleEntry.value);
  const pricedJoin = await execute(
    contract,
    storage,
    'joinEnclave',
    providerJoin,
    provider.publicKey,
    8
  );
  assert.equal(pricedJoin.ok, true, pricedJoin.message);

  const roomId = await deriveRoomId(enclaveId, admin.publicKey, 'priced-room');
  const opened = await execute(
    contract,
    storage,
    'openRoom',
    {
      op: 'open_room',
      enclave_id: enclaveId,
      model_id: enclaveRegistration.model_id,
      nonce: 'priced-room',
      label: 'priced-room',
      policy: {},
    },
    admin.publicKey,
    9
  );
  assert.equal(opened.ok, true, opened.message);

  priceScheduleEntry = await storage.get(`price/${enclaveId}`);
  priceScheduleEntry.value.current.set_by_role = 'provider';
  await storage.put(`price/${enclaveId}`, priceScheduleEntry.value);
  const providerRolePriceRoomJoin = await execute(
    contract,
    storage,
    'joinRoom',
    {
      op: 'join_room',
      room_id: roomId,
      enclave_id: enclaveId,
    },
    provider.publicKey,
    9
  );
  assert.match(providerRolePriceRoomJoin.message, /current admin-set enclave price/i);
  assert.equal(await storage.get(`roomserve/${roomId}/${provider.publicKey}/${enclaveId}`), null);

  await storage.del(`price/${enclaveId}`);

  const unpricedRoomJoin = await execute(
    contract,
    storage,
    'joinRoom',
    {
      op: 'join_room',
      room_id: roomId,
      enclave_id: enclaveId,
    },
    provider.publicKey,
    10
  );
  assert.match(unpricedRoomJoin.message, /current admin price required/i);
  assert.equal(await storage.get(`roomserve/${roomId}/${provider.publicKey}/${enclaveId}`), null);
});

test('MayhemContract applies consent and provider lifecycle through free mayhem feature records', async () => {
  const admin = await makeIdentity();
  const provider = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const protocol = { peer: { wallet: makeVerifier(provider.wallet) } };
  const contract = new MayhemContract(protocol, {});

  let result = await execute(
    contract,
    storage,
    'setRules',
    { op: 'set_rules', ver: 1, hash: rulesHash },
    admin.publicKey,
    1
  );
  assert.equal(result.ok, true, result.message);

  await executeFeature(
    contract,
    storage,
    'mayhem_feature',
    `consent/${provider.publicKey}/1/${rulesHash}`,
    {
      op: 'consent',
      sender: provider.publicKey,
      ver: 1,
      hash: rulesHash,
      sig: signConsent(provider.wallet, 1, rulesHash),
    },
    admin.publicKey
  );
  assert.equal((await storage.get(`consent/${provider.publicKey}`)).value.via, 'feature');

  const registerIntent = {
    op: 'register_provider',
    provider: provider.publicKey,
    nonce: 'a'.repeat(64),
  };
  await executeFeature(
    contract,
    storage,
    'mayhem_feature',
    await providerLifecycleFeatureKey(registerIntent),
    {
      op: 'provider_lifecycle',
      intent: registerIntent,
      sig: signProviderLifecycleIntent(provider.wallet, registerIntent),
    },
    admin.publicKey
  );
  assert.equal((await storage.get(`prov/${provider.publicKey}`)).value.status, 'active');

  result = await execute(contract, storage, 'registerEnclave', enclaveRegistration, admin.publicKey, 2);
  assert.equal(result.ok, true, result.message);
  await seedCurrentAdminPrice(storage, {
    enclaveId,
    modelId: enclaveRegistration.model_id,
    admin: admin.publicKey,
    txNo: 3,
  });

  const joinEnclaveIntent = {
    op: 'join_enclave',
    provider: provider.publicKey,
    enclave_id: enclaveId,
    nonce: 'b'.repeat(64),
  };
  await executeFeature(
    contract,
    storage,
    'mayhem_feature',
    await providerLifecycleFeatureKey(joinEnclaveIntent),
    {
      op: 'provider_lifecycle',
      intent: joinEnclaveIntent,
      sig: signProviderLifecycleIntent(provider.wallet, joinEnclaveIntent),
    },
    admin.publicKey
  );
  assert.equal((await storage.get(`serve/${provider.publicKey}/${enclaveId}`)).value.status, 'active');

  const nonce = 'feature-room';
  const roomId = await deriveRoomId(enclaveId, admin.publicKey, nonce);
  result = await execute(
    contract,
    storage,
    'openRoom',
    {
      op: 'open_room',
      enclave_id: enclaveId,
      model_id: enclaveRegistration.model_id,
      nonce,
      label: 'feature-room',
      policy: { min_reputation: 0 },
    },
    admin.publicKey,
    4
  );
  assert.equal(result.ok, true, result.message);

  const joinRoomIntent = {
    op: 'join_room',
    provider: provider.publicKey,
    enclave_id: enclaveId,
    room_id: roomId,
    nonce: 'c'.repeat(64),
  };
  await executeFeature(
    contract,
    storage,
    'mayhem_feature',
    await providerLifecycleFeatureKey(joinRoomIntent),
    {
      op: 'provider_lifecycle',
      intent: joinRoomIntent,
      sig: signProviderLifecycleIntent(provider.wallet, joinRoomIntent),
    },
    admin.publicKey
  );
  assert.equal(
    (await storage.get(`roomserve/${roomId}/${provider.publicKey}/${enclaveId}`)).value.status,
    'active'
  );
});

test('MayhemContract provider serving price gate fails closed without current admin key', async () => {
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
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }
  await seedCurrentAdminPrice(storage, {
    enclaveId,
    modelId: enclaveRegistration.model_id,
    admin: admin.publicKey,
    txNo: 5,
  });
  await storage.del('admin');

  const before = storage.snapshotBytes();
  const result = await execute(
    contract,
    storage,
    'joinEnclave',
    providerJoin,
    provider.publicKey,
    6
  );
  assert.match(result.message, /current admin key/i);
  assert.equal(storage.snapshotBytes(), before);
  assert.equal(await storage.get(`serve/${provider.publicKey}/${enclaveId}`), null);
});

test('MayhemContract provider serving rejects explicit non-admin enclave markers', async () => {
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
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }
  await seedCurrentAdminPrice(storage, {
    enclaveId,
    modelId: enclaveRegistration.model_id,
    admin: admin.publicKey,
    txNo: 5,
  });

  const enclave = (await storage.get(`enclave/${enclaveId}`)).value;
  await storage.put(`enclave/${enclaveId}`, {
    ...enclave,
    created_by_role: 'provider',
  });

  const before = storage.snapshotBytes();
  const rejected = await execute(
    contract,
    storage,
    'joinEnclave',
    providerJoin,
    provider.publicKey,
    6
  );
  assert.match(rejected.message, /admin-created enclave/i);
  assert.equal(storage.snapshotBytes(), before);
  assert.equal(await storage.get(`serve/${provider.publicKey}/${enclaveId}`), null);
});

test('MayhemContract validates admin enclave caps as capability-only records', async () => {
  const admin = await makeIdentity();
  const provider = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const protocol = { peer: { wallet: makeVerifier(provider.wallet) } };
  const contract = new MayhemContract(protocol, {});

  const providerFeeCaps = await execute(
    contract,
    storage,
    'registerEnclave',
    {
      ...enclaveRegistration,
      enclave_id: '7'.repeat(64),
      caps: {
        ...enclaveRegistration.caps,
        provider_fee_bps: 1000,
      },
    },
    admin.publicKey,
    1
  );
  assert.match(providerFeeCaps.message, /unsupported enclave caps field.*provider_fee_bps/i);
  assert.equal(await storage.get(`enclave/${'7'.repeat(64)}`), null);

  const stringBooleanCaps = await execute(
    contract,
    storage,
    'registerEnclave',
    {
      ...enclaveRegistration,
      enclave_id: '8'.repeat(64),
      caps: {
        ...enclaveRegistration.caps,
        tools: 'true',
      },
    },
    admin.publicKey,
    2
  );
  assert.match(stringBooleanCaps.message, /caps tools must be a boolean/i);

  const invalidContextCaps = await execute(
    contract,
    storage,
    'registerEnclave',
    {
      ...enclaveRegistration,
      enclave_id: '9'.repeat(64),
      caps: {
        chat: true,
        tools: false,
        ctx: 0,
      },
    },
    admin.publicKey,
    3
  );
  assert.match(invalidContextCaps.message, /caps ctx must be a positive integer/i);

  const catalogStyleCaps = {
    tools: true,
    json: true,
    ctx_max: 8192,
    vision: false,
  };
  const accepted = await execute(
    contract,
    storage,
    'registerEnclave',
    {
      ...enclaveRegistration,
      enclave_id: 'a'.repeat(64),
      caps: catalogStyleCaps,
    },
    admin.publicKey,
    4
  );
  assert.equal(accepted.ok, true, accepted.message);

  const tensorParallelCaps = await execute(
    contract,
    storage,
    'registerEnclave',
    {
      ...enclaveRegistration,
      enclave_id: 'b'.repeat(64),
      caps: {
        ...catalogStyleCaps,
        tp_degree: 2,
        max_batch_size: 4,
        max_num_tokens: 4096,
      },
    },
    admin.publicKey,
    5
  );
  assert.equal(tensorParallelCaps.ok, true, tensorParallelCaps.message);

  const invalidTensorParallelCaps = await execute(
    contract,
    storage,
    'registerEnclave',
    {
      ...enclaveRegistration,
      enclave_id: 'c'.repeat(64),
      caps: {
        ...catalogStyleCaps,
        tp_degree: 0,
      },
    },
    admin.publicKey,
    6
  );
  assert.match(invalidTensorParallelCaps.message, /caps tp_degree must be a positive integer/i);

  const unsupportedUpdate = await execute(
    contract,
    storage,
    'updateEnclave',
    {
      op: 'update_enclave',
      enclave_id: 'a'.repeat(64),
      caps: {
        ...catalogStyleCaps,
        price_ver: 99,
      },
    },
    admin.publicKey,
    7
  );
  assert.match(unsupportedUpdate.message, /unsupported enclave caps field.*price_ver/i);

  const updatedCaps = {
    ...catalogStyleCaps,
    embeddings: true,
  };
  const validUpdate = await execute(
    contract,
    storage,
    'updateEnclave',
    {
      op: 'update_enclave',
      enclave_id: 'a'.repeat(64),
      caps: updatedCaps,
    },
    admin.publicKey,
    8
  );
  assert.equal(validUpdate.ok, true, validUpdate.message);

  const stored = await storage.get(`enclave/${'a'.repeat(64)}`);
  assert.deepEqual(stored.value.caps, updatedCaps);
  assert.equal(stored.value.updated_by, admin.publicKey);
  assert.equal(stored.value.updated_by_role, 'admin');
  assert.equal(stored.value.updated_at, makeTxKey(8));
});

test('MayhemContract accepts canonical integer attestation tiers 1 through 4 only', async () => {
  const admin = await makeIdentity();
  const provider = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const protocol = { peer: { wallet: makeVerifier(provider.wallet) } };
  const contract = new MayhemContract(protocol, {});

  const tier4Register = await execute(
    contract,
    storage,
    'registerEnclave',
    {
      ...enclaveRegistration,
      enclave_id: 'd'.repeat(64),
      att_tier: 4,
    },
    admin.publicKey,
    1
  );
  assert.equal(tier4Register.ok, true, tier4Register.message);

  const tier4Stored = await storage.get(`enclave/${'d'.repeat(64)}`);
  assert.equal(tier4Stored.value.att_tier, 4);

  const tier3Update = await execute(
    contract,
    storage,
    'updateEnclave',
    {
      op: 'update_enclave',
      enclave_id: 'd'.repeat(64),
      att_tier: 3,
    },
    admin.publicKey,
    2
  );
  assert.equal(tier3Update.ok, true, tier3Update.message);

  const tier3Stored = await storage.get(`enclave/${'d'.repeat(64)}`);
  assert.equal(tier3Stored.value.att_tier, 3);

  const fractionalRegister = await execute(
    contract,
    storage,
    'registerEnclave',
    {
      ...enclaveRegistration,
      enclave_id: 'e'.repeat(64),
      att_tier: 3 / 2,
    },
    admin.publicKey,
    3
  );
  assert.notEqual(fractionalRegister.ok, true);
  assert.match(fractionalRegister.message, /invalid schema/i);
  assert.equal(await storage.get(`enclave/${'e'.repeat(64)}`), null);
});

test('MayhemContract rejects provider-authored payout and probation hints', async () => {
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
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }

  const providerAuthoredTerms = await execute(
    contract,
    storage,
    'registerProvider',
    {
      op: 'register_provider',
      payout_addr: 'provider-picked-target',
      payout_method: 'coinbase',
      registered_at_seconds: 123_456,
    },
    provider.publicKey,
    3
  );
  assert.match(providerAuthoredTerms.message, /payout_addr|payout_method|registered_at_seconds|forbidden/i);
  assert.equal(await storage.get(`prov/${provider.publicKey}`), null);

  const registered = await execute(
    contract,
    storage,
    'registerProvider',
    { op: 'register_provider' },
    provider.publicKey,
    4
  );
  assert.equal(registered.ok, true, registered.message);

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
    5
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
    6
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
    7
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
    set_by_role: 'admin',
    set_at: makeTxKey(7),
  });
  assert.equal(updated.value.updated_at, makeTxKey(7));
});

test('MayhemContract admin verifies and revokes provider KYB without raw documents', async () => {
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
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }

  const unsignedKyb = {
    op: 'set_provider_kyb',
    provider: provider.publicKey,
    legal_name: 'Acme AI GmbH',
    jurisdiction: 'DE',
    proof_hash: 'a'.repeat(64),
    kyb_ref: 'KYB-2026-0001',
    verified_at: 1_788_000_000,
    schema_version: 1,
  };
  const signedKyb = {
    ...unsignedKyb,
    admin_sig: signProviderKyb(admin.wallet, unsignedKyb),
  };

  const nonAdmin = await execute(
    contract,
    storage,
    'setProviderKyb',
    signedKyb,
    provider.publicKey,
    4
  );
  assert.match(nonAdmin.message, /admin required/i);

  const badSig = await execute(
    contract,
    storage,
    'setProviderKyb',
    {
      ...unsignedKyb,
      legal_name: 'Acme AI GmbH Tampered',
      admin_sig: signedKyb.admin_sig,
    },
    admin.publicKey,
    5
  );
  assert.match(badSig.message, /kyb admin signature/i);

  const rawDocuments = await execute(
    contract,
    storage,
    'setProviderKyb',
    {
      ...signedKyb,
      documents: [{ kind: 'passport', number: 'raw-pii' }],
    },
    admin.publicKey,
    6
  );
  assert.notEqual(rawDocuments.ok, true);
  assert.equal(await storage.get(`kyb/${provider.publicKey}`), null);

  const verified = await execute(
    contract,
    storage,
    'setProviderKyb',
    signedKyb,
    admin.publicKey,
    7
  );
  assert.deepEqual(verified, {
    ok: true,
    op: 'setProviderKyb',
    provider: provider.publicKey,
    att_tier: 4,
  });

  const kyb = await storage.get(`kyb/${provider.publicKey}`);
  assert.deepEqual(kyb.value, {
    status: 'verified',
    provider: provider.publicKey,
    legal_name: 'Acme AI GmbH',
    jurisdiction: 'DE',
    proof_hash: 'a'.repeat(64),
    kyb_ref: 'KYB-2026-0001',
    verified_at: 1_788_000_000,
    verified_by: admin.publicKey,
    verified_by_role: 'admin',
    admin_sig: signedKyb.admin_sig,
    schema_version: 1,
    updated_at: makeTxKey(7),
  });
  assert.equal(JSON.stringify(kyb.value).includes('passport'), false);
  assert.equal(JSON.stringify(kyb.value).includes('raw-pii'), false);

  const providerEntry = await storage.get(`prov/${provider.publicKey}`);
  assert.equal(providerEntry.value.kyb.status, 'verified');
  assert.equal(providerEntry.value.kyb.legal_name, 'Acme AI GmbH');
  assert.equal(providerEntry.value.kyb.verified_by_role, 'admin');

  const providerRevoke = await execute(
    contract,
    storage,
    'revokeProviderKyb',
    {
      op: 'revoke_provider_kyb',
      provider: provider.publicKey,
      reason_hash: 'b'.repeat(64),
    },
    provider.publicKey,
    8
  );
  assert.match(providerRevoke.message, /admin required/i);

  const revoked = await execute(
    contract,
    storage,
    'revokeProviderKyb',
    {
      op: 'revoke_provider_kyb',
      provider: provider.publicKey,
      reason_hash: 'b'.repeat(64),
    },
    admin.publicKey,
    9
  );
  assert.deepEqual(revoked, {
    ok: true,
    op: 'revokeProviderKyb',
    provider: provider.publicKey,
  });

  const revokedKyb = await storage.get(`kyb/${provider.publicKey}`);
  assert.equal(revokedKyb.value.status, 'revoked');
  assert.equal(revokedKyb.value.revoked_by, admin.publicKey);
  assert.equal(revokedKyb.value.revoked_by_role, 'admin');
  assert.equal(revokedKyb.value.revoke_reason_hash, 'b'.repeat(64));
  const revokedProvider = await storage.get(`prov/${provider.publicKey}`);
  assert.equal(revokedProvider.value.kyb.status, 'revoked');
});

test('MayhemContract rejects provider-authored serving terms on joins', async () => {
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
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }

  await seedCurrentAdminPrice(storage, {
    enclaveId,
    modelId: enclaveRegistration.model_id,
    admin: admin.publicKey,
    txNo: 5,
  });

  const providerPricedJoin = await execute(
    contract,
    storage,
    'joinEnclave',
    {
      op: 'join_enclave',
      enclave_id: enclaveId,
      model_id: 'provider/custom@4bit',
      price_ver: 999,
      in_per_1k_mu: 1,
      out_per_1k_mu: 1,
    },
    provider.publicKey,
    5
  );
  assert.match(providerPricedJoin.message, /model_id|price_ver|in_per_1k_mu|forbidden/i);
  assert.equal(await storage.get(`serve/${provider.publicKey}/${enclaveId}`), null);

  const joinedEnclave = await execute(
    contract,
    storage,
    'joinEnclave',
    providerJoin,
    provider.publicKey,
    6
  );
  assert.equal(joinedEnclave.ok, true, joinedEnclave.message);

  const roomId = await deriveRoomId(enclaveId, admin.publicKey, 'provider-term-room');
  const openedRoom = await execute(
    contract,
    storage,
    'openRoom',
    {
      op: 'open_room',
      enclave_id: enclaveId,
      model_id: enclaveRegistration.model_id,
      nonce: 'provider-term-room',
      label: 'provider-term-room',
      policy: { min_reputation: 0, max_price_mult: 1 },
    },
    admin.publicKey,
    7
  );
  assert.equal(openedRoom.ok, true, openedRoom.message);

  const providerRoomTerms = await execute(
    contract,
    storage,
    'joinRoom',
    {
      op: 'join_room',
      room_id: roomId,
      enclave_id: enclaveId,
      model_id: 'provider/custom@4bit',
      sidechannel: 'mx/room/provider-local-only',
      policy: { max_price_mult: 99 },
      price_ver: 999,
    },
    provider.publicKey,
    8
  );
  assert.match(providerRoomTerms.message, /model_id|sidechannel|policy|price_ver|forbidden/i);
  assert.equal(await storage.get(`roomserve/${roomId}/${provider.publicKey}/${enclaveId}`), null);
  assert.deepEqual((await storage.get(`serve/${provider.publicKey}/${enclaveId}`)).value.rooms, []);

  const joinedRoom = await execute(
    contract,
    storage,
    'joinRoom',
    {
      op: 'join_room',
      room_id: roomId,
      enclave_id: enclaveId,
    },
    provider.publicKey,
    9
  );
  assert.equal(joinedRoom.ok, true, joinedRoom.message);

  const roomServing = await storage.get(`roomserve/${roomId}/${provider.publicKey}/${enclaveId}`);
  assert.equal(roomServing.value.model_id, enclaveRegistration.model_id);
  assert.equal(roomServing.value.sidechannel, `mx/room/${roomId}`);
  assert.equal(Object.hasOwn(roomServing.value, 'price_ver'), false);
  assert.equal(Object.hasOwn(roomServing.value, 'policy'), false);

  const providerLeaveRoomTerms = await execute(
    contract,
    storage,
    'leaveRoom',
    {
      op: 'leave_room',
      room_id: roomId,
      enclave_id: enclaveId,
      sidechannel: 'mx/room/provider-local-only',
    },
    provider.publicKey,
    10
  );
  assert.match(providerLeaveRoomTerms.message, /sidechannel|provider-authored fields/i);
  assert.equal(
    (await storage.get(`roomserve/${roomId}/${provider.publicKey}/${enclaveId}`)).value.status,
    'active'
  );

  const leftRoom = await execute(
    contract,
    storage,
    'leaveRoom',
    {
      op: 'leave_room',
      room_id: roomId,
      enclave_id: enclaveId,
    },
    provider.publicKey,
    11
  );
  assert.equal(leftRoom.ok, true, leftRoom.message);

  const providerLeaveEnclaveTerms = await execute(
    contract,
    storage,
    'leaveEnclave',
    {
      op: 'leave_enclave',
      enclave_id: enclaveId,
      model_id: 'provider/custom@4bit',
    },
    provider.publicKey,
    12
  );
  assert.match(providerLeaveEnclaveTerms.message, /model_id|provider-authored fields/i);
  assert.equal((await storage.get(`serve/${provider.publicKey}/${enclaveId}`)).value.status, 'active');

  const leftEnclave = await execute(
    contract,
    storage,
    'leaveEnclave',
    {
      op: 'leave_enclave',
      enclave_id: enclaveId,
    },
    provider.publicKey,
    13
  );
  assert.equal(leftEnclave.ok, true, leftEnclave.message);
});

test('MayhemContract rejects unsafe canonical registry identifiers', async () => {
  const admin = await makeIdentity();
  const provider = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const protocol = { peer: { wallet: makeVerifier(provider.wallet) } };
  const contract = new MayhemContract(protocol, {});

  const badEnclaveRegister = await execute(
    contract,
    storage,
    'registerEnclave',
    {
      ...enclaveRegistration,
      enclave_id: 'bad/enclave',
    },
    admin.publicKey,
    1
  );
  assert.match(badEnclaveRegister.message, /invalid enclave id/i);
  assert.equal(await storage.get('enclave/bad/enclave'), null);

  const badModelRegister = await execute(
    contract,
    storage,
    'registerEnclave',
    {
      ...enclaveRegistration,
      enclave_id: 'a'.repeat(64),
      model_id: '/provider/model',
    },
    admin.publicKey,
    2
  );
  assert.match(badModelRegister.message, /invalid model id/i);
  assert.equal(await storage.get(`enclave/${'a'.repeat(64)}`), null);

  const badSourceRegister = await execute(
    contract,
    storage,
    'registerEnclave',
    {
      ...enclaveRegistration,
      enclave_id: 'b'.repeat(64),
      artifact_source: {
        ...artifactSource,
        revision: 'main',
      },
    },
    admin.publicKey,
    3
  );
  assert.match(badSourceRegister.message, /artifact_source\.revision/i);
  assert.equal(await storage.get(`enclave/${'b'.repeat(64)}`), null);

  const badArtifactKindRegister = await execute(
    contract,
    storage,
    'registerEnclave',
    {
      ...enclaveRegistration,
      enclave_id: 'c'.repeat(64),
      artifact_root_kind: 'blake3_descriptor_until_p2_4',
    },
    admin.publicKey,
    3
  );
  assert.match(badArtifactKindRegister.message, /artifact_root_kind/i);
  assert.equal(await storage.get(`enclave/${'c'.repeat(64)}`), null);

  const badUpdate = await execute(
    contract,
    storage,
    'updateEnclave',
    {
      op: 'update_enclave',
      enclave_id: 'bad/enclave',
      artifact_root: updatedArtifactRoot,
    },
    admin.publicKey,
    3
  );
  assert.match(badUpdate.message, /invalid enclave id/i);

  const badRetire = await execute(
    contract,
    storage,
    'retireEnclave',
    {
      op: 'retire_enclave',
      enclave_id: 'bad/enclave',
    },
    admin.publicKey,
    4
  );
  assert.match(badRetire.message, /invalid enclave id/i);

  const badJoin = await execute(
    contract,
    storage,
    'joinEnclave',
    {
      op: 'join_enclave',
      enclave_id: 'bad/enclave',
    },
    provider.publicKey,
    5
  );
  assert.match(badJoin.message, /invalid enclave id/i);

  const badLeave = await execute(
    contract,
    storage,
    'leaveEnclave',
    {
      op: 'leave_enclave',
      enclave_id: 'bad/enclave',
    },
    provider.publicKey,
    6
  );
  assert.match(badLeave.message, /invalid enclave id/i);

  const badBan = await execute(
    contract,
    storage,
    'banProvider',
    {
      op: 'ban_provider',
      provider: 'bad/provider',
    },
    admin.publicKey,
    7
  );
  assert.match(badBan.message, /invalid provider id/i);
  assert.equal(await storage.get('prov/bad/provider'), null);
});

test('MayhemContract admin can ban providers from future serving mutations', async () => {
  const admin = await makeIdentity();
  const provider = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const protocol = { peer: { wallet: makeVerifier(provider.wallet) } };
  const contract = new MayhemContract(protocol, {});
  const roomId = await deriveRoomId(enclaveId, admin.publicKey, 'ban-room');

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
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }
  await seedCurrentAdminPrice(storage, {
    enclaveId,
    modelId: enclaveRegistration.model_id,
    admin: admin.publicKey,
    txNo: 5,
  });

  const joinedEnclave = await execute(
    contract,
    storage,
    'joinEnclave',
    providerJoin,
    provider.publicKey,
    5
  );
  assert.equal(joinedEnclave.ok, true, joinedEnclave.message);

  const openedRoom = await execute(
    contract,
    storage,
    'openRoom',
    {
      op: 'open_room',
      enclave_id: enclaveId,
      model_id: enclaveRegistration.model_id,
      nonce: 'ban-room',
      label: 'ban-room',
      policy: {},
    },
    admin.publicKey,
    6
  );
  assert.equal(openedRoom.ok, true, openedRoom.message);

  const joinedRoom = await execute(
    contract,
    storage,
    'joinRoom',
    {
      op: 'join_room',
      room_id: roomId,
      enclave_id: enclaveId,
    },
    provider.publicKey,
    7
  );
  assert.equal(joinedRoom.ok, true, joinedRoom.message);
  assert.deepEqual((await storage.get(`room/${roomId}`)).value.serves, [
    { provider: provider.publicKey, enclave_id: enclaveId },
  ]);

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
    8
  );
  assert.deepEqual(banned, {
    ok: true,
    op: 'banProvider',
    provider: provider.publicKey,
    tombstoned_enclaves: [enclaveId],
  });

  const providerEntry = await storage.get(`prov/${provider.publicKey}`);
  assert.equal(providerEntry.value.status, 'banned');
  assert.equal(providerEntry.value.banned_by, admin.publicKey);
  assert.equal(providerEntry.value.banned_by_role, 'admin');
  assert.equal(providerEntry.value.banned_at, makeTxKey(8));
  assert.equal(providerEntry.value.ban_reason_hash, '7'.repeat(64));
  assert.deepEqual(providerEntry.value.enclaves, []);
  assert.deepEqual(providerEntry.value.tombstoned_enclaves, [enclaveId]);

  const servingEntry = await storage.get(`serve/${provider.publicKey}/${enclaveId}`);
  assert.equal(servingEntry.value.status, 'tombstoned');
  assert.deepEqual(servingEntry.value.rooms, []);
  assert.equal(servingEntry.value.tombstone_reason_hash, '7'.repeat(64));

  const roomServingEntry = await storage.get(`roomserve/${roomId}/${provider.publicKey}/${enclaveId}`);
  assert.equal(roomServingEntry.value.status, 'tombstoned');
  assert.equal(roomServingEntry.value.tombstone_reason_hash, '7'.repeat(64));
  const roomEntry = await storage.get(`room/${roomId}`);
  assert.deepEqual(roomEntry.value.serves, []);
  assert.equal(roomEntry.value.serves_updated_at, makeTxKey(8));

  const joinAfterBan = await execute(
    contract,
    storage,
    'joinEnclave',
    providerJoin,
    provider.publicKey,
    9
  );
  assert.match(joinAfterBan.message, /provider registration required/i);
});

test('MayhemContract admin retirement tombstones indexed provider room serving', async () => {
  const admin = await makeIdentity();
  const provider = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const protocol = { peer: { wallet: makeVerifier(provider.wallet) } };
  const contract = new MayhemContract(protocol, {});
  const roomId = await deriveRoomId(enclaveId, admin.publicKey, 'retire-room');

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
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }
  await seedCurrentAdminPrice(storage, {
    enclaveId,
    modelId: enclaveRegistration.model_id,
    admin: admin.publicKey,
    txNo: 5,
  });

  const joinedEnclave = await execute(
    contract,
    storage,
    'joinEnclave',
    providerJoin,
    provider.publicKey,
    5
  );
  assert.equal(joinedEnclave.ok, true, joinedEnclave.message);

  const enclaveBeforeRetire = await storage.get(`enclave/${enclaveId}`);
  assert.deepEqual(enclaveBeforeRetire.value.providers, [provider.publicKey]);

  const openedRoom = await execute(
    contract,
    storage,
    'openRoom',
    {
      op: 'open_room',
      enclave_id: enclaveId,
      model_id: enclaveRegistration.model_id,
      nonce: 'retire-room',
      label: 'retire-room',
      policy: {},
    },
    admin.publicKey,
    6
  );
  assert.equal(openedRoom.ok, true, openedRoom.message);

  const joinedRoom = await execute(
    contract,
    storage,
    'joinRoom',
    {
      op: 'join_room',
      room_id: roomId,
      enclave_id: enclaveId,
    },
    provider.publicKey,
    7
  );
  assert.equal(joinedRoom.ok, true, joinedRoom.message);
  assert.deepEqual((await storage.get(`room/${roomId}`)).value.serves, [
    { provider: provider.publicKey, enclave_id: enclaveId },
  ]);

  const retired = await execute(
    contract,
    storage,
    'retireEnclave',
    {
      op: 'retire_enclave',
      enclave_id: enclaveId,
    },
    admin.publicKey,
    8
  );
  assert.deepEqual(retired, {
    ok: true,
    op: 'retireEnclave',
    enclave_id: enclaveId,
    tombstoned_providers: [provider.publicKey],
  });

  const providerEntry = await storage.get(`prov/${provider.publicKey}`);
  assert.equal(providerEntry.value.status, 'active');
  assert.deepEqual(providerEntry.value.enclaves, []);

  const enclaveEntry = await storage.get(`enclave/${enclaveId}`);
  assert.equal(enclaveEntry.value.status, 'retired');
  assert.deepEqual(enclaveEntry.value.providers, []);
  assert.deepEqual(enclaveEntry.value.tombstoned_providers, [provider.publicKey]);

  const servingEntry = await storage.get(`serve/${provider.publicKey}/${enclaveId}`);
  assert.equal(servingEntry.value.status, 'tombstoned');
  assert.deepEqual(servingEntry.value.rooms, []);
  assert.equal(servingEntry.value.tombstoned_at, makeTxKey(8));

  const roomServingEntry = await storage.get(`roomserve/${roomId}/${provider.publicKey}/${enclaveId}`);
  assert.equal(roomServingEntry.value.status, 'tombstoned');
  assert.equal(roomServingEntry.value.tombstoned_at, makeTxKey(8));
  const roomEntry = await storage.get(`room/${roomId}`);
  assert.deepEqual(roomEntry.value.serves, []);
  assert.equal(roomEntry.value.serves_updated_at, makeTxKey(8));
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

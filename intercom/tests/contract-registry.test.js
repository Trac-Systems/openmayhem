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
  textRateMap,
} from './helpers/contract.js';

const rulesHash = '1'.repeat(64);
const enclaveId = '2'.repeat(64);
const manifestHash = '3'.repeat(64);
const binaryHash = '4'.repeat(64);
const artifactRoot = '5'.repeat(64);
const updatedArtifactRoot = '6'.repeat(64);
const hardwareFingerprint = '8'.repeat(64);
const deviceKey = '9'.repeat(64);
const artifactSource = {
  kind: 'huggingface',
  repo: 'mayhem-catalog/qwen2.5-4b-instruct-GGUF',
  revision: '7'.repeat(40),
  path: 'qwen2.5-4b-instruct-Q4_K_M.gguf',
};
const artifactSidecars = {
  mmproj: {
    source: {
      kind: 'huggingface',
      repo: 'mayhem-catalog/qwen2.5-4b-instruct-GGUF',
      revision: '7'.repeat(40),
      path: 'mmproj-qwen2.5-4b-instruct-Q4_K_M.gguf',
    },
    path: 'mmproj-qwen2.5-4b-instruct-Q4_K_M.gguf',
    artifact_root: '8'.repeat(64),
    artifact_root_kind: 'blake3_merkle_v1',
    weights_bytes: 1234,
    source_sha256: '9'.repeat(64),
  },
};

function makeArtifactSidecars(count) {
  return Object.fromEntries(Array.from({ length: count }, (_, index) => {
    const name = `component_${index.toString().padStart(2, '0')}`;
    const path = `components/${name}.safetensors`;
    return [name, {
      source: {
        kind: 'huggingface',
        repo: 'mayhem-catalog/compound-model',
        revision: '7'.repeat(40),
        path,
      },
      path,
      artifact_root: index.toString(16).padStart(2, '0').repeat(32),
      artifact_root_kind: 'blake3_merkle_v1',
      weights_bytes: index + 1,
      source_sha256: (index + 1).toString(16).padStart(2, '0').repeat(32),
    }];
  }));
}

const launchMeasurements = {
  schema_version: 1,
  effective_epoch: 0,
  platform: 'azure-h100-sev-snp-nvidia-cc',
  layers: {
    workload: {
      vtpm_pcr_0: 'a'.repeat(96),
    },
  },
};

const providerRegistration = {
  op: 'register_provider',
};

const enclaveRegistration = {
  op: 'register_enclave',
  enclave_id: enclaveId,
  model_id: 'qwen/qwen2.5-4b-instruct@4bit',
  model_class: 'text-generation',
  backend: 'llama.cpp',
  artifact_root: artifactRoot,
  artifact_root_kind: 'blake3_merkle_v1',
  artifact_source: artifactSource,
  manifest_hash: manifestHash,
  att_tier: 1,
  quant: 'INT4',
  binary_hash: binaryHash,
  caps: {
    chat: true,
    embeddings: false,
    tools: false,
    ctx: 32768,
    modality_set: ['text'],
    speciality_levels: {},
  },
};

const enclaveUpdate = {
  op: 'update_enclave',
  enclave_id: enclaveId,
  caps: {
    chat: true,
    embeddings: false,
    tools: true,
    ctx: 32768,
    modality_set: ['text'],
    speciality_levels: {},
  },
};

const enclaveRetire = {
  op: 'retire_enclave',
  enclave_id: enclaveId,
};

const modelRef = {
  op: 'set_model_ref',
  model_id: enclaveRegistration.model_id,
  model_class: 'text-generation',
  rate_map: textRateMap(20, 60),
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
    prompt_ids: ['launch-text'],
  }],
};

const priceSchedule = {
  op: 'set_price',
  enclave_id: enclaveId,
  rate_map: textRateMap(20, 60),
  per_req_au: '0',
  min_session_au: '0',
  effective_at: 0,
};

const providerJoin = {
  op: 'join_enclave',
  enclave_id: enclaveId,
  att_tier: 1,
  attestation_head: 'd'.repeat(64),
  served_ctx: 32768,
  served_modalities: ['text'],
  served_specialities: {},
  ctx_bracket: 'le32k',
  ctx_bracket_table_ver: 1,
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
    accepted_rails: ['fiat'],
    accepted_rails_schema_version: 1,
    accepted_rails_set_by: provider.publicKey,
    accepted_rails_set_at: makeTxKey(3),
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
  assert.equal(enclaveEntry.value.quant, 'int4');
  assert.equal(enclaveEntry.value.artifact_root, artifactRoot);
  assert.equal(enclaveEntry.value.artifact_root_kind, 'blake3_merkle_v1');
  assert.deepEqual(enclaveEntry.value.artifact_source, artifactSource);
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
    att_tier: 1,
    effective_att_tier: 1,
    attestation_head: 'd'.repeat(64),
    served_ctx: 32768,
    served_modalities: ['text'],
    served_specialities: {},
    ctx_bracket: 'le32k',
    ctx_bracket_table_ver: 1,
    joined_at: makeTxKey(8),
    updated_at: makeTxKey(10),
    left_at: null,
    tombstoned_at: makeTxKey(10),
    tombstone_reason_hash: null,
    rooms: [],
  });
});

test('MayhemContract requires model_class and allows admin model classes', async () => {
  const admin = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const protocol = { peer: { wallet: makeVerifier(admin.wallet) } };
  const contract = new MayhemContract(protocol, {});

  const missingEnclaveClass = await execute(
    contract,
    storage,
    'registerEnclave',
    {
      ...enclaveRegistration,
      enclave_id: 'b'.repeat(64),
      model_id: 'admin/missing-class@q4',
      model_class: undefined,
    },
    admin.publicKey,
    2
  );
  assert.equal(missingEnclaveClass instanceof Error, true);
  assert.match(missingEnclaveClass.message, /Invalid schema/i);

  const missingModelRefClass = await execute(
    contract,
    storage,
    'setModelRef',
    {
      op: 'set_model_ref',
      model_id: 'admin/missing-modelref-class@q4',
      rate_map: textRateMap(20, 60),
    },
    admin.publicKey,
    3
  );
  assert.equal(missingModelRefClass instanceof Error, true);
  assert.match(missingModelRefClass.message, /Invalid schema/i);

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
      modality_set: ['embedding'],
      speciality_levels: {},
    },
  };
  const registered = await execute(
    contract,
    storage,
    'registerEnclave',
    embeddingRegistration,
    admin.publicKey,
    4
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
      rate_map: [{ unit: 'embedding', per_unit_au: '2', granularity: 1 }],
    },
    admin.publicKey,
    5
  );
  assert.equal(modelRefResult.ok, true, modelRefResult.message);
  assert.equal((await storage.get(`modelref/${embeddingRegistration.model_id}`)).value.model_class, 'embedding');

  const musicRegistration = {
    ...enclaveRegistration,
    enclave_id: '1'.repeat(64),
    model_id: 'admin/music-small@fp16',
    model_class: 'music-generation',
    backend: 'ace-step',
    artifact_root: '2'.repeat(64),
    manifest_hash: '3'.repeat(64),
    binary_hash: '4'.repeat(64),
    caps: {
      audio: true,
      output_modality: 'audio',
      output_modalities: ['audio'],
      modality_set: ['audio'],
      speciality_levels: {},
    },
  };
  const musicRegistered = await execute(
    contract,
    storage,
    'registerEnclave',
    musicRegistration,
    admin.publicKey,
    6
  );
  assert.equal(musicRegistered.ok, true, musicRegistered.message);
  assert.equal((await storage.get(`enclave/${musicRegistration.enclave_id}`)).value.model_class, 'music-generation');

  const musicModelRef = await execute(
    contract,
    storage,
    'setModelRef',
    {
      op: 'set_model_ref',
      model_id: musicRegistration.model_id,
      model_class: 'music-generation',
      rate_map: [
        { unit: 'input_character', per_unit_au: '1', granularity: 100 },
        { unit: 'audio_second', per_unit_au: '50', granularity: 1 },
      ],
    },
    admin.publicKey,
    7
  );
  assert.equal(musicModelRef.ok, true, musicModelRef.message);
  assert.equal((await storage.get(`modelref/${musicRegistration.model_id}`)).value.model_class, 'music-generation');
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
      caps: {
        embeddings: true,
        ctx: 8192,
        modality_set: ['embedding'],
        speciality_levels: {},
      },
    },
    admin.publicKey,
    3
  );
  assert.equal(registered.ok, true, registered.message);
  await storage.put(`modelref/${enclaveRegistration.model_id}`, {
    model_id: enclaveRegistration.model_id,
    model_class: 'text-generation',
    rate_map: textRateMap(20, 60),
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

test('MayhemContract stores admin-bound enclave artifact sidecars only when canonical', async () => {
  const admin = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const protocol = { peer: { wallet: makeVerifier(admin.wallet) } };
  const contract = new MayhemContract(protocol, {});

  const registered = await execute(
    contract,
    storage,
    'registerEnclave',
    {
      ...enclaveRegistration,
      enclave_id: 'b'.repeat(64),
      artifact_sidecars: artifactSidecars,
      caps: {
        ...enclaveRegistration.caps,
        vision: true,
        modality_set: ['text', 'image'],
        speciality_levels: {},
      },
    },
    admin.publicKey,
    1
  );
  assert.equal(registered.ok, true, registered.message);
  assert.deepEqual(
    (await storage.get(`enclave/${'b'.repeat(64)}`)).value.artifact_sidecars,
    artifactSidecars
  );

  const mismatchedPath = await execute(
    contract,
    storage,
    'registerEnclave',
    {
      ...enclaveRegistration,
      enclave_id: 'c'.repeat(64),
      artifact_sidecars: {
        mmproj: {
          ...artifactSidecars.mmproj,
          source: {
            ...artifactSidecars.mmproj.source,
            path: 'different-mmproj.gguf',
          },
        },
      },
    },
    admin.publicKey,
    2
  );
  assert.match(mismatchedPath.message, /source\.path must match path/i);
  assert.equal(await storage.get(`enclave/${'c'.repeat(64)}`), null);

  const compoundRegistration = await execute(
    contract,
    storage,
    'registerEnclave',
    {
      ...enclaveRegistration,
      enclave_id: 'd'.repeat(64),
      artifact_sidecars: makeArtifactSidecars(25),
    },
    admin.publicKey,
    3
  );
  assert.equal(compoundRegistration.ok, true, compoundRegistration.message);
  assert.equal(
    Object.keys((await storage.get(`enclave/${'d'.repeat(64)}`)).value.artifact_sidecars).length,
    25
  );

  const excessiveSidecars = await execute(
    contract,
    storage,
    'registerEnclave',
    {
      ...enclaveRegistration,
      enclave_id: 'e'.repeat(64),
      artifact_sidecars: makeArtifactSidecars(65),
    },
    admin.publicKey,
    4
  );
  assert.match(excessiveSidecars.message, /too many entries/i);
  assert.equal(await storage.get(`enclave/${'e'.repeat(64)}`), null);
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

test('MayhemContract anchors optional Comfy parts index metadata', async () => {
  const admin = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const protocol = { peer: { wallet: makeVerifier(admin.wallet) } };
  const contract = new MayhemContract(protocol, {});
  const revision = 'c'.repeat(40);
  const release = {
    ...catalogRelease,
    parts_anchor: {
      index_ver: 7,
      source_kind: 'huggingface',
      index_url: `https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/resolve/${revision}/index.json`,
      anchor_url: `https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/resolve/${revision}/anchor.json`,
      anchor_hash: 'd'.repeat(64),
      index_root: 'e'.repeat(64),
      record_count: 2,
      repo_revision: revision,
    },
    blessed_runtimes: [{
      runtime_id: 'comfyui-v0.30.1',
      comfy_release_hash: '1'.repeat(64),
      env_lock_hash: '2'.repeat(64),
      whitelist_ver: 3,
      status: 'blessed',
      min_grace_epochs: 2,
    }],
    outcome_classes: [{
      class_id: 'image.light.512',
      enclave_id: '3'.repeat(64),
      definition_hash: '4'.repeat(64),
      status: 'active',
    }],
  };

  const result = await execute(
    contract,
    storage,
    'publishCatalog',
    release,
    admin.publicKey,
    1
  );

  assert.equal(result.ok, true, result.message);
  const current = await storage.get('catalog/current');
  assert.deepEqual(current.value.parts_anchor, release.parts_anchor);
  assert.deepEqual(current.value.blessed_runtimes, release.blessed_runtimes);
  assert.deepEqual(current.value.outcome_classes, release.outcome_classes);
});

test('MayhemContract rejects mutable Comfy parts index anchors', async () => {
  const admin = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const protocol = { peer: { wallet: makeVerifier(admin.wallet) } };
  const contract = new MayhemContract(protocol, {});
  const revision = 'c'.repeat(40);
  const release = {
    ...catalogRelease,
    parts_anchor: {
      index_ver: 7,
      source_kind: 'huggingface',
      index_url: 'https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/resolve/main/index.json',
      anchor_url: `https://huggingface.co/datasets/TracNetwork/openmayhem-parts-index/resolve/${revision}/anchor.json`,
      anchor_hash: 'd'.repeat(64),
      index_root: 'e'.repeat(64),
      record_count: 2,
      repo_revision: revision,
    },
  };

  const result = await execute(
    contract,
    storage,
    'publishCatalog',
    release,
    admin.publicKey,
    1
  );

  assert.ok(result instanceof Error);
  assert.match(result.message, /parts index URL/i);
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
  const priceScheduleKey = `price/${enclaveId}/le32k`;
  let priceScheduleEntry = await storage.get(priceScheduleKey);
  priceScheduleEntry.value.current.set_by = provider.publicKey;
  delete priceScheduleEntry.value.current.set_by_role;
  await storage.put(priceScheduleKey, priceScheduleEntry.value);
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

  priceScheduleEntry = await storage.get(priceScheduleKey);
  priceScheduleEntry.value.current.set_by = admin.publicKey;
  delete priceScheduleEntry.value.current.set_by_role;
  await storage.put(priceScheduleKey, priceScheduleEntry.value);
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

  priceScheduleEntry = await storage.get(priceScheduleKey);
  priceScheduleEntry.value.current.set_by_role = 'admin';
  await storage.put(priceScheduleKey, priceScheduleEntry.value);
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
    10
  );
  assert.equal(opened.ok, true, opened.message);

  priceScheduleEntry = await storage.get(priceScheduleKey);
  priceScheduleEntry.value.current.set_by_role = 'provider';
  await storage.put(priceScheduleKey, priceScheduleEntry.value);
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

  await storage.del(priceScheduleKey);

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
  const registered = (await storage.get(`prov/${provider.publicKey}`)).value;
  assert.equal(registered.status, 'active');
  assert.deepEqual(registered.accepted_rails, ['fiat']);

  const railsIntent = {
    op: 'set_provider_rails',
    provider: provider.publicKey,
    rails: ['tap'],
    nonce: 'd'.repeat(64),
  };
  const railsResult = await executeFeature(
    contract,
    storage,
    'mayhem_feature',
    await providerLifecycleFeatureKey(railsIntent),
    {
      op: 'provider_lifecycle',
      intent: railsIntent,
      sig: signProviderLifecycleIntent(provider.wallet, railsIntent),
    },
    admin.publicKey
  );
  assert.deepEqual(railsResult, {
    ok: true,
    op: 'setProviderRails',
    provider: provider.publicKey,
    rails: ['tap'],
  });
  const tapOnly = (await storage.get(`prov/${provider.publicKey}`)).value;
  assert.deepEqual(tapOnly.accepted_rails, ['tap']);
  assert.equal(tapOnly.accepted_rails_set_by, provider.publicKey);
  assert.equal(tapOnly.accepted_rails_set_at, await providerLifecycleFeatureKey(railsIntent));

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
    att_tier: 1,
    attestation_head: 'd'.repeat(64),
    served_ctx: 32768,
    served_modalities: ['text'],
    served_specialities: {},
    ctx_bracket: 'le32k',
    ctx_bracket_table_ver: 1,
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
  const serving = (await storage.get(`serve/${provider.publicKey}/${enclaveId}`)).value;
  assert.equal(serving.status, 'active');
  assert.equal(serving.served_ctx, 32768);
  assert.equal(serving.ctx_bracket, 'le32k');
  assert.equal(serving.ctx_bracket_table_ver, 1);

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

test('MayhemContract enclave admission checks the provider attestation tier', async () => {
  const admin = await makeIdentity();
  const provider = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const contract = new MayhemContract({ peer: { wallet: makeVerifier(provider.wallet) } }, {});

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
  const tier2Enclave = { ...enclaveRegistration, att_tier: 2 };
  const registered = await execute(
    contract,
    storage,
    'registerEnclave',
    tier2Enclave,
    admin.publicKey,
    4
  );
  assert.equal(registered.ok, true, registered.message);
  await seedCurrentAdminPrice(storage, {
    enclaveId,
    modelId: tier2Enclave.model_id,
    admin: admin.publicKey,
    txNo: 5,
  });

  const lowTier = await execute(
    contract,
    storage,
    'joinEnclave',
    { ...providerJoin, att_tier: 1 },
    provider.publicKey,
    6
  );
  assert.match(lowTier.message, /does not match enclave tier 2/i);
  assert.equal(await storage.get(`serve/${provider.publicKey}/${enclaveId}`), null);

  const admitted = await execute(
    contract,
    storage,
    'joinEnclave',
    { ...providerJoin, att_tier: 2 },
    provider.publicKey,
    7
  );
  assert.equal(admitted.ok, true, admitted.message);
  const serving = (await storage.get(`serve/${provider.publicKey}/${enclaveId}`)).value;
  assert.equal(serving.att_tier, 2);
  assert.equal(serving.effective_att_tier, 2);
  assert.equal(serving.attestation_head, 'd'.repeat(64));
});

test('MayhemContract appends Tier-3 measurement blessings as admin-only feature records', async () => {
  const admin = await makeIdentity();
  const outsider = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const protocol = { peer: { wallet: makeVerifier(admin.wallet) } };
  const contract = new MayhemContract(protocol, {});
  const value = {
    op: 'tier3_bless_measurement',
    platform: 'azure-h100-sev-snp-nvidia-cc',
    layer: 'workload',
    measurement_name: 'vtpm_pcr_0',
    measurement: 'a'.repeat(96),
    effective_epoch: 7,
    at: 1783517300,
    region: 'centralus',
    derivation_hash: 'b'.repeat(64),
    source: 'admin-derive',
  };
  const key = await contract.tier3MeasurementFeatureKey(value);
  if (key instanceof Error) throw key;

  contract._mayhemLastFeatureResult = undefined;
  const rejectedRaw = await executeFeature(contract, storage, 'mayhem_feature', key, value, outsider.publicKey);
  const rejected = rejectedRaw ?? contract._mayhemLastFeatureResult;
  assert.match(rejected.message, /admin required/i);
  assert.equal(await storage.get(`tier3/measurement/${value.platform}`), null);

  contract._mayhemLastFeatureResult = undefined;
  const appliedRaw = await executeFeature(contract, storage, 'mayhem_feature', key, value, admin.publicKey);
  const applied = appliedRaw ?? contract._mayhemLastFeatureResult;
  assert.equal(applied.ok, true, applied.message);
  assert.equal(applied.status, 'blessed');
  const first = await storage.get(`tier3/measurement/${value.platform}`);
  assert.deepEqual(first.value.measurements.workload.vtpm_pcr_0, [value.measurement]);
  assert.equal(first.value.entries.length, 1);
  assert.equal(first.value.entries[0].layer, 'workload');
  assert.equal(first.value.entries[0].region, 'centralus');

  contract._mayhemLastFeatureResult = undefined;
  const duplicateRaw = await executeFeature(contract, storage, 'mayhem_feature', key, value, admin.publicKey);
  const duplicate = duplicateRaw ?? contract._mayhemLastFeatureResult;
  assert.equal(duplicate.status, 'already_blessed');
  assert.equal((await storage.get(`tier3/measurement/${value.platform}`)).value.entries.length, 1);

  const rolled = { ...value, measurement: 'c'.repeat(96), at: value.at + 1 };
  const rolledKey = await contract.tier3MeasurementFeatureKey(rolled);
  if (rolledKey instanceof Error) throw rolledKey;
  contract._mayhemLastFeatureResult = undefined;
  const rolledRaw = await executeFeature(contract, storage, 'mayhem_feature', rolledKey, rolled, admin.publicKey);
  const rolledResult = rolledRaw ?? contract._mayhemLastFeatureResult;
  assert.equal(rolledResult.status, 'blessed');
  const afterRoll = await storage.get(`tier3/measurement/${value.platform}`);
  assert.deepEqual(afterRoll.value.measurements.workload.vtpm_pcr_0, [value.measurement, rolled.measurement]);
  assert.equal(afterRoll.value.entries.length, 2);
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
      enclave_id: '0'.repeat(64),
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
        modality_set: ['text'],
        speciality_levels: {},
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
    modality_set: ['text'],
    speciality_levels: {},
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

  const vllmEnclave = await execute(
    contract,
    storage,
    'registerEnclave',
    {
      ...enclaveRegistration,
      enclave_id: '9'.repeat(64),
      backend: 'vllm',
      caps: {
        ...catalogStyleCaps,
        tools: true,
        tp_degree: 1,
        max_batch_size: 2,
        max_num_tokens: 1024,
        kv_bytes_per_token: 20480,
        vllm_dtype: 'bfloat16',
        vllm_gpu_memory_utilization_pct: 40,
      },
    },
    admin.publicKey,
    55
  );
  assert.equal(vllmEnclave.ok, true, vllmEnclave.message);

  const invalidVllmMemoryCap = await execute(
    contract,
    storage,
    'registerEnclave',
    {
      ...enclaveRegistration,
      enclave_id: '8'.repeat(64),
      backend: 'vllm',
      caps: {
        ...catalogStyleCaps,
        vllm_gpu_memory_utilization_pct: 101,
      },
    },
    admin.publicKey,
    56
  );
  assert.match(invalidVllmMemoryCap.message, /vllm_gpu_memory_utilization_pct must be between 1 and 100/i);
  assert.equal(await storage.get(`enclave/${'0'.repeat(64)}`), null);

  const diffusionImageCaps = {
    image: true,
    output_modality: 'image',
    output_modalities: ['image'],
    modality_set: ['image'],
    speciality_levels: {},
    max_image_width: 1024,
    max_image_height: 1024,
    max_image_steps: 50,
  };
  const diffusionImage = await execute(
    contract,
    storage,
    'registerEnclave',
    {
      ...enclaveRegistration,
      enclave_id: 'd'.repeat(64),
      model_id: 'admin/image-small@fp16',
      model_class: 'image-generation',
      backend: 'diffusers',
      caps: diffusionImageCaps,
    },
    admin.publicKey,
    6
  );
  assert.equal(diffusionImage.ok, true, diffusionImage.message);
  assert.deepEqual((await storage.get(`enclave/${'d'.repeat(64)}`)).value.caps, diffusionImageCaps);

  const transformersAsrCaps = {
    chat: false,
    tools: false,
    json: false,
    vision: false,
    image: false,
    video: false,
    audio: true,
    ctx: 1,
    ctx_max: 1,
    output_modality: 'text',
    output_modalities: ['text'],
    modality_set: ['audio', 'text'],
    speciality_levels: {},
  };
  const transformersAsr = await execute(
    contract,
    storage,
    'registerEnclave',
    {
      ...enclaveRegistration,
      enclave_id: '6'.repeat(64),
      model_id: 'admin/parakeet',
      model_class: 'stt',
      backend: 'transformers-asr',
      caps: transformersAsrCaps,
    },
    admin.publicKey,
    57
  );
  assert.equal(transformersAsr.ok, true, transformersAsr.message);
  assert.deepEqual((await storage.get(`enclave/${'6'.repeat(64)}`)).value.caps, transformersAsrCaps);

  const invalidOutputModality = await execute(
    contract,
    storage,
    'registerEnclave',
    {
      ...enclaveRegistration,
      enclave_id: 'e'.repeat(64),
      caps: {
        ...catalogStyleCaps,
        image: true,
        output_modality: 'image',
        modality_set: ['text', 'image'],
        speciality_levels: {},
      },
    },
    admin.publicKey,
    7
  );
  assert.match(invalidOutputModality.message, /output_modality image is not allowed for model_class text-generation/i);

  const extensibleBackend = await execute(
    contract,
    storage,
    'registerEnclave',
    {
      ...enclaveRegistration,
      enclave_id: 'f'.repeat(64),
      backend: 'provider-local-python',
      caps: catalogStyleCaps,
    },
    admin.publicKey,
    8
  );
  assert.equal(extensibleBackend.ok, true, extensibleBackend.message);
  assert.equal(
    (await storage.get(`enclave/${'f'.repeat(64)}`)).value.backend,
    'provider-local-python'
  );

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
    9
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
    10
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
    11
  );
  assert.equal(validUpdate.ok, true, validUpdate.message);

  const stored = await storage.get(`enclave/${'a'.repeat(64)}`);
  assert.deepEqual(stored.value.caps, updatedCaps);
  assert.equal(stored.value.updated_by, admin.publicKey);
  assert.equal(stored.value.updated_by_role, 'admin');
  assert.equal(stored.value.updated_at, makeTxKey(11));

  const nextBinaryHash = 'e'.repeat(64);
  const releaseUpdate = await execute(
    contract,
    storage,
    'updateEnclave',
    {
      op: 'update_enclave',
      enclave_id: 'a'.repeat(64),
      binary_hash: nextBinaryHash,
    },
    admin.publicKey,
    12
  );
  assert.equal(releaseUpdate.ok, true, releaseUpdate.message);
  const afterRelease = await storage.get(`enclave/${'a'.repeat(64)}`);
  assert.equal(afterRelease.value.binary_hash, nextBinaryHash);
  assert.deepEqual(afterRelease.value.approved_binary_hashes, [binaryHash, nextBinaryHash]);

  const releaseRevoke = await execute(
    contract,
    storage,
    'updateEnclave',
    {
      op: 'update_enclave',
      enclave_id: 'a'.repeat(64),
      approved_binary_hashes: [nextBinaryHash],
    },
    admin.publicKey,
    13
  );
  assert.equal(releaseRevoke.ok, true, releaseRevoke.message);
  assert.deepEqual(
    (await storage.get(`enclave/${'a'.repeat(64)}`)).value.approved_binary_hashes,
    [nextBinaryHash]
  );

  const immutableUpdate = await execute(
    contract,
    storage,
    'updateEnclave',
    {
      op: 'update_enclave',
      enclave_id: 'a'.repeat(64),
      artifact_root: updatedArtifactRoot,
    },
    admin.publicKey,
    14
  );
  assert.match(immutableUpdate.message, /does not accept immutable fields: artifact_root/i);
  assert.equal((await storage.get(`enclave/${'a'.repeat(64)}`)).value.artifact_root, artifactRoot);
});

test('MayhemContract allows launch enclave hardware tiers and rejects KYB-only tier', async () => {
  const admin = await makeIdentity();
  const provider = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const protocol = { peer: { wallet: makeVerifier(provider.wallet) } };
  const contract = new MayhemContract(protocol, {});

  const tier1Register = await execute(
    contract,
    storage,
    'registerEnclave',
    {
      ...enclaveRegistration,
      enclave_id: 'd'.repeat(64),
      att_tier: 1,
    },
    admin.publicKey,
    1
  );
  assert.equal(tier1Register.ok, true, tier1Register.message);

  const tier1Stored = await storage.get(`enclave/${'d'.repeat(64)}`);
  assert.equal(tier1Stored.value.att_tier, 1);

  const tier2Register = await execute(
    contract,
    storage,
    'registerEnclave',
    {
      ...enclaveRegistration,
      enclave_id: 'f'.repeat(64),
      att_tier: 2,
    },
    admin.publicKey,
    2
  );
  assert.equal(tier2Register.ok, true, tier2Register.message);
  const tier2Stored = await storage.get(`enclave/${'f'.repeat(64)}`);
  assert.equal(tier2Stored.value.att_tier, 2);

  const tier3MissingMeasurement = await execute(
    contract,
    storage,
    'updateEnclave',
    {
      op: 'update_enclave',
      enclave_id: 'd'.repeat(64),
      att_tier: 3,
    },
    admin.publicKey,
    3
  );
  assert.notEqual(tier3MissingMeasurement.ok, true);
  assert.match(tier3MissingMeasurement.message, /launch_measurements/i);

  const tier3VendorOnlyMeasurement = await execute(
    contract,
    storage,
    'updateEnclave',
    {
      op: 'update_enclave',
      enclave_id: 'd'.repeat(64),
      att_tier: 3,
      launch_measurements: {
        schema_version: 1,
        effective_epoch: 0,
        platform: 'azure-h100-sev-snp-nvidia-cc',
        layers: {
          vendor: {
            snp_launch_digest: 'b'.repeat(96),
          },
        },
      },
    },
    admin.publicKey,
    4
  );
  assert.notEqual(tier3VendorOnlyMeasurement.ok, true);
  assert.match(tier3VendorOnlyMeasurement.message, /workload PCR/i);

  const tier3Update = await execute(
    contract,
    storage,
    'updateEnclave',
    {
      op: 'update_enclave',
      enclave_id: 'd'.repeat(64),
      att_tier: 3,
      launch_measurements: launchMeasurements,
    },
    admin.publicKey,
    5
  );
  assert.equal(tier3Update.ok, true, tier3Update.message);
  const afterTier3Update = await storage.get(`enclave/${'d'.repeat(64)}`);
  assert.equal(afterTier3Update.value.att_tier, 3);
  assert.deepEqual(afterTier3Update.value.launch_measurements, launchMeasurements);

  const tier4Register = await execute(
    contract,
    storage,
    'registerEnclave',
    {
      ...enclaveRegistration,
      enclave_id: 'b'.repeat(64),
      att_tier: 4,
    },
    admin.publicKey,
    5
  );
  assert.notEqual(tier4Register.ok, true);
  assert.match(tier4Register.message, /Tier 4 is provider KYB/i);
  assert.equal(await storage.get(`enclave/${'b'.repeat(64)}`), null);

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
    5
  );
  assert.notEqual(fractionalRegister.ok, true);
  assert.match(fractionalRegister.message, /invalid schema/i);
  assert.equal(await storage.get(`enclave/${'e'.repeat(64)}`), null);
});

test('MayhemContract schedules enclave min-tier notice without stranding active providers', async () => {
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

  const joined = await execute(contract, storage, 'joinEnclave', providerJoin, provider.publicKey, 6);
  assert.equal(joined.ok, true, joined.message);

  const shortNotice = await execute(
    contract,
    storage,
    'setEnclaveMinTier',
    {
      op: 'set_enclave_min_tier',
      enclave_id: enclaveId,
      min_att_tier: 1,
      submitted_epoch: 10,
      effective_epoch: 20,
      submitted_at: 0,
      reason_hash: 'e'.repeat(64),
    },
    admin.publicKey,
    7
  );
  assert.notEqual(shortNotice.ok, true);
  assert.match(shortNotice.message, /min_tier_notice_epochs/i);

  const tier2 = await execute(
    contract,
    storage,
    'setEnclaveMinTier',
    {
      op: 'set_enclave_min_tier',
      enclave_id: enclaveId,
      min_att_tier: 2,
      submitted_epoch: 10,
      effective_epoch: 34,
      submitted_at: 0,
      reason_hash: 'f'.repeat(64),
    },
    admin.publicKey,
    8
  );
  assert.equal(tier2.ok, true, tier2.message);
  const tier2Policy = await storage.get(`tierpolicy/enclave/${enclaveId}`);
  assert.equal(tier2Policy.value.pending.min_att_tier, 2);

  const tier4 = await execute(
    contract,
    storage,
    'setEnclaveMinTier',
    {
      op: 'set_enclave_min_tier',
      enclave_id: enclaveId,
      min_att_tier: 4,
      submitted_epoch: 10,
      effective_epoch: 34,
      submitted_at: 0,
      reason_hash: 'e'.repeat(64),
    },
    admin.publicKey,
    9
  );
  assert.notEqual(tier4.ok, true);
  assert.match(tier4.message, /Tier 4 is provider KYB/i);

  const scheduled = await execute(
    contract,
    storage,
    'setEnclaveMinTier',
    {
      op: 'set_enclave_min_tier',
      enclave_id: enclaveId,
      min_att_tier: 1,
      submitted_epoch: 10,
      effective_epoch: 34,
      submitted_at: 0,
      reason_hash: 'a'.repeat(64),
    },
    admin.publicKey,
    9
  );
  assert.equal(scheduled.ok, true, scheduled.message);
  const policy = await storage.get(`tierpolicy/enclave/${enclaveId}`);
  assert.equal(policy.value.current_min_att_tier, 1);
  assert.equal(policy.value.pending.min_att_tier, 1);
  assert.equal(policy.value.pending.effective_epoch, 34);
  assert.equal(policy.value.pending.reason_hash, 'a'.repeat(64));
  const enclave = await storage.get(`enclave/${enclaveId}`);
  assert.equal(enclave.value.min_att_tier, 1);
  assert.equal(enclave.value.pending_min_att_tier.effective_epoch, 34);
  const serving = await storage.get(`serve/${provider.publicKey}/${enclaveId}`);
  assert.equal(serving.value.status, 'active');
  assert.equal((await storage.get(`prov/${provider.publicKey}`)).value.status, 'active');
});

test('MayhemContract rejects provider-authored payout hints and exposes no mutable payout op', async () => {
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
      payout_method: 'unsupported',
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
  assert.equal(Object.hasOwn(providerEntry.value, 'payouts'), false);
  assert.equal(providerEntry.value.probation.since_seconds, 0);
  assert.equal(typeof contract.setProviderPayout, 'undefined');
  assert.equal(contract.schemas?.has?.('setProviderPayout') ?? false, false);
  assert.equal(typeof contract.tnkSettlement, 'undefined');
  assert.equal(typeof contract.fiatSettlement, 'undefined');
  assert.equal(typeof contract.requireAdminSetPayoutTarget, 'undefined');
  assert.equal(contract.schemas?.has?.('tnkSettlement') ?? false, false);
  assert.equal(contract.schemas?.has?.('fiatSettlement') ?? false, false);
  assert.equal(providerEntry.value.updated_at, makeTxKey(4));
});

test('MayhemContract lets providers declare accepted payment rails without stale rail compatibility', async () => {
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
      value: { op: 'register_provider' },
      sender: provider.publicKey,
      txNo: 3,
    },
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }

  const registered = (await storage.get(`prov/${provider.publicKey}`)).value;
  assert.deepEqual(registered.accepted_rails, ['fiat']);
  assert.equal(registered.accepted_rails_schema_version, 1);
  assert.equal(registered.accepted_rails_set_by, provider.publicKey);

  const rejectedUnsupportedRail = await execute(
    contract,
    storage,
    'setProviderRails',
    {
      op: 'set_provider_rails',
      rails: ['unsupported'],
    },
    provider.publicKey,
    4
  );
  assert.match(rejectedUnsupportedRail.message, /unsupported provider payment rail/i);

  const rejectedEmpty = await execute(
    contract,
    storage,
    'setProviderRails',
    {
      op: 'set_provider_rails',
      rails: [],
    },
    provider.publicKey,
    5
  );
  assert.notEqual(rejectedEmpty.ok, true);
  assert.match(rejectedEmpty.message, /invalid schema|cannot be empty/i);

  const rejectedDuplicate = await execute(
    contract,
    storage,
    'setProviderRails',
    {
      op: 'set_provider_rails',
      rails: ['fiat', 'fiat'],
    },
    provider.publicKey,
    6
  );
  assert.match(rejectedDuplicate.message, /duplicate provider payment rail/i);

  const accepted = await execute(
    contract,
    storage,
    'setProviderRails',
    {
      op: 'set_provider_rails',
      rails: ['fiat', 'tap', 'tnk'],
    },
    provider.publicKey,
    7
  );
  assert.deepEqual(accepted, {
    ok: true,
    op: 'setProviderRails',
    provider: provider.publicKey,
    rails: ['fiat', 'tap', 'tnk'],
  });
  const updated = (await storage.get(`prov/${provider.publicKey}`)).value;
  assert.deepEqual(updated.accepted_rails, ['fiat', 'tap', 'tnk']);
  assert.equal(updated.accepted_rails_schema_version, 1);
  assert.equal(updated.accepted_rails_set_by, provider.publicKey);
  assert.equal(updated.accepted_rails_set_at, makeTxKey(7));
  assert.equal(updated.updated_at, makeTxKey(7));
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

  const replacement = await makeIdentity();
  for (const op of [
    {
      type: 'consent',
      value: {
        op: 'consent',
        ver: 1,
        hash: rulesHash,
        sig: signConsent(replacement.wallet, 1, rulesHash),
      },
      sender: replacement.publicKey,
      txNo: 10,
    },
    {
      type: 'registerProvider',
      value: providerRegistration,
      sender: replacement.publicKey,
      txNo: 11,
    },
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }
  const replacementKyb = {
    ...unsignedKyb,
    provider: replacement.publicKey,
  };
  const replacementSignedKyb = {
    ...replacementKyb,
    admin_sig: signProviderKyb(admin.wallet, replacementKyb),
  };
  const regrantRevokedIdentity = await execute(
    contract,
    storage,
    'setProviderKyb',
    replacementSignedKyb,
    admin.publicKey,
    12
  );
  assert.match(regrantRevokedIdentity.message, /kyb identity is banned or revoked/i);
  assert.equal(await storage.get(`kyb/${replacement.publicKey}`), null);
});

test('MayhemContract lets providers narrow but never invent admin speciality levels', async () => {
  const admin = await makeIdentity();
  const provider = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const protocol = { peer: { wallet: makeVerifier(provider.wallet) } };
  const contract = new MayhemContract(protocol, {});
  const specialityEnclave = {
    ...enclaveRegistration,
    caps: {
      ...enclaveRegistration.caps,
      speciality_levels: {
        reasoning_effort: ['none', 'high'],
        verbosity: ['concise', 'detailed'],
      },
    },
  };

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
      value: specialityEnclave,
      sender: admin.publicKey,
      txNo: 4,
    },
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }
  await seedCurrentAdminPrice(storage, {
    enclaveId,
    modelId: specialityEnclave.model_id,
    admin: admin.publicKey,
    txNo: 5,
  });
  const join = {
    ...providerJoin,
    served_specialities: {
      reasoning_effort: ['none'],
      verbosity: ['concise', 'detailed'],
    },
  };

  const arbitraryName = await execute(
    contract,
    storage,
    'joinEnclave',
    {
      ...join,
      served_specialities: {
        ...join.served_specialities,
        arbitrary_model: ['provider-choice'],
      },
    },
    provider.publicKey,
    6
  );
  assert.match(arbitraryName.message, /exactly the admin enclave speciality names/i);
  assert.equal(await storage.get(`serve/${provider.publicKey}/${enclaveId}`), null);

  const arbitraryLevel = await execute(
    contract,
    storage,
    'joinEnclave',
    {
      ...join,
      served_specialities: {
        ...join.served_specialities,
        reasoning_effort: ['ultra'],
      },
    },
    provider.publicKey,
    7
  );
  assert.match(arbitraryLevel.message, /must be a subset of the admin enclave levels/i);
  assert.equal(await storage.get(`serve/${provider.publicKey}/${enclaveId}`), null);

  const accepted = await execute(
    contract,
    storage,
    'joinEnclave',
    join,
    provider.publicKey,
    8
  );
  assert.equal(accepted.ok, true, accepted.message);
  assert.deepEqual(
    (await storage.get(`serve/${provider.publicKey}/${enclaveId}`)).value.served_specialities,
    join.served_specialities
  );
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
      att_tier: 1,
      attestation_head: 'd'.repeat(64),
      served_ctx: 32768,
      served_modalities: ['text'],
      served_specialities: {},
      ctx_bracket: 'le32k',
      ctx_bracket_table_ver: 1,
      model_id: 'provider/custom@4bit',
      price_ver: 999,
      rate_map: textRateMap(1, 1),
    },
    provider.publicKey,
    5
  );
  assert.match(providerPricedJoin.message, /model_id|price_ver|rate_map|forbidden/i);
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
      att_tier: 1,
      attestation_head: 'd'.repeat(64),
      served_ctx: 32768,
      served_modalities: ['text'],
      served_specialities: {},
      ctx_bracket: 'le32k',
      ctx_bracket_table_ver: 1,
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
    device_key: null,
    hardware_fingerprint: null,
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

test('MayhemContract provider/device bans are reversible and Tier-1 fingerprint matches only flag review', async () => {
  const admin = await makeIdentity();
  const provider = await makeIdentity();
  const replacement = await makeIdentity();
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
      type: 'consent',
      value: {
        op: 'consent',
        ver: 1,
        hash: rulesHash,
        sig: signConsent(replacement.wallet, 1, rulesHash),
      },
      sender: replacement.publicKey,
      txNo: 3,
    },
    {
      type: 'registerProvider',
      value: providerRegistration,
      sender: provider.publicKey,
      txNo: 4,
    },
    {
      type: 'registerProvider',
      value: providerRegistration,
      sender: replacement.publicKey,
      txNo: 5,
    },
    {
      type: 'registerEnclave',
      value: enclaveRegistration,
      sender: admin.publicKey,
      txNo: 6,
    },
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }
  await seedCurrentAdminPrice(storage, {
    enclaveId,
    modelId: enclaveRegistration.model_id,
    admin: admin.publicKey,
    txNo: 7,
  });

  const firstJoin = await execute(
    contract,
    storage,
    'joinEnclave',
    { ...providerJoin, hardware_fingerprint: hardwareFingerprint, device_key: deviceKey },
    provider.publicKey,
    7
  );
  assert.equal(firstJoin.ok, true, firstJoin.message);

  const banned = await execute(
    contract,
    storage,
    'banProvider',
    {
      op: 'ban_provider',
      provider: provider.publicKey,
      reason_hash: 'a'.repeat(64),
      device_key: deviceKey,
      hardware_fingerprint: hardwareFingerprint,
    },
    admin.publicKey,
    8
  );
  assert.equal(banned.ok, true, banned.message);
  assert.equal((await storage.get(`ban/provider/${provider.publicKey}`)).value.status, 'banned');
  assert.equal((await storage.get(`ban/device/${deviceKey}`)).value.status, 'banned');
  const fingerprintBan = (await storage.get(`ban/fingerprint/${hardwareFingerprint}`)).value;
  assert.equal(fingerprintBan.status, 'banned');
  assert.equal(fingerprintBan.auto_reject, true);

  const rejectedDevice = await execute(
    contract,
    storage,
    'joinEnclave',
    { ...providerJoin, hardware_fingerprint: hardwareFingerprint, device_key: deviceKey },
    replacement.publicKey,
    9
  );
  assert.match(rejectedDevice.message, /device key is banned/i);

  const providerUnban = await execute(
    contract,
    storage,
    'unban',
    {
      op: 'unban',
      target_type: 'provider',
      target: provider.publicKey,
      reason_hash: 'b'.repeat(64),
    },
    admin.publicKey,
    10
  );
  assert.equal(providerUnban.ok, true, providerUnban.message);
  assert.equal((await storage.get(`prov/${provider.publicKey}`)).value.status, 'active');
  assert.equal((await storage.get(`ban/provider/${provider.publicKey}`)).value.status, 'unbanned');

  const deviceUnban = await execute(
    contract,
    storage,
    'unban',
    {
      op: 'unban',
      target_type: 'device',
      target: deviceKey,
      reason_hash: 'c'.repeat(64),
    },
    admin.publicKey,
    11
  );
  assert.equal(deviceUnban.ok, true, deviceUnban.message);
  assert.equal((await storage.get(`ban/device/${deviceKey}`)).value.status, 'unbanned');

  const rebound = await execute(
    contract,
    storage,
    'deviceRebind',
    {
      op: 'device_rebind',
      device_key: deviceKey,
      provider: replacement.publicKey,
      reason_hash: 'd'.repeat(64),
    },
    admin.publicKey,
    12
  );
  assert.equal(rebound.ok, true, rebound.message);
  assert.equal((await storage.get(`device/${deviceKey}`)).value.provider, replacement.publicKey);

  const rejectedFingerprint = await execute(
    contract,
    storage,
    'joinEnclave',
    { ...providerJoin, hardware_fingerprint: hardwareFingerprint, device_key: deviceKey },
    replacement.publicKey,
    13
  );
  assert.match(rejectedFingerprint.message, /hardware fingerprint is banned/i);
  assert.equal(await storage.get(`serve/${replacement.publicKey}/${enclaveId}`), null);

  const fingerprintUnban = await execute(
    contract,
    storage,
    'unban',
    {
      op: 'unban',
      target_type: 'fingerprint',
      target: hardwareFingerprint,
      reason_hash: 'e'.repeat(64),
    },
    admin.publicKey,
    14
  );
  assert.equal(fingerprintUnban.ok, true, fingerprintUnban.message);
  assert.equal((await storage.get(`ban/fingerprint/${hardwareFingerprint}`)).value.status, 'unbanned');

  const cleanJoin = await execute(
    contract,
    storage,
    'joinEnclave',
    { ...providerJoin, hardware_fingerprint: hardwareFingerprint, device_key: deviceKey },
    replacement.publicKey,
    15
  );
  assert.equal(cleanJoin.ok, true, cleanJoin.message);
  const serving = await storage.get(`serve/${replacement.publicKey}/${enclaveId}`);
  assert.equal(serving.value.status, 'active');
  assert.equal(serving.value.fingerprint_review, undefined);
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

import assert from 'node:assert/strict';
import test from 'node:test';
import MayhemContract, { deriveRoomId } from '../contract/contract.js';
import {
  MemoryStorage,
  execute,
  makeIdentity,
  makeTxKey,
  makeVerifier,
  seedCurrentAdminPrice,
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

const modelRef = {
  op: 'set_model_ref',
  model_id: enclaveRegistration.model_id,
  price_ref_mu: {
    in_per_1k: 20,
    out_per_1k: 60,
  },
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
    type: 'setPrice',
    value: priceSchedule,
    sender: admin.publicKey,
    txNo: 6,
  },
  {
    type: 'joinEnclave',
    value: providerJoin,
    sender: provider.publicKey,
    txNo: 7,
  },
  {
    type: 'updateEnclave',
    value: enclaveUpdate,
    sender: admin.publicKey,
    txNo: 8,
  },
  {
    type: 'retireEnclave',
    value: enclaveRetire,
    sender: admin.publicKey,
    txNo: 9,
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
    updated_at: makeTxKey(9),
  });

  const enclaveEntry = await first.storage.get(`enclave/${enclaveId}`);
  assert.equal(enclaveEntry.value.status, 'retired');
  assert.equal(enclaveEntry.value.created_by, admin.publicKey);
  assert.equal(enclaveEntry.value.created_by_role, 'admin');
  assert.equal(enclaveEntry.value.artifact_root, updatedArtifactRoot);
  assert.equal(enclaveEntry.value.registered_at, makeTxKey(4));
  assert.equal(enclaveEntry.value.updated_at, makeTxKey(9));
  assert.equal(enclaveEntry.value.retired_at, makeTxKey(9));
  assert.deepEqual(enclaveEntry.value.providers, []);
  assert.deepEqual(enclaveEntry.value.tombstoned_providers, [provider.publicKey]);

  const servingEntry = await first.storage.get(`serve/${provider.publicKey}/${enclaveId}`);
  assert.deepEqual(servingEntry.value, {
    provider: provider.publicKey,
    enclave_id: enclaveId,
    model_id: enclaveRegistration.model_id,
    status: 'tombstoned',
    joined_at: makeTxKey(7),
    updated_at: makeTxKey(9),
    left_at: null,
    tombstoned_at: makeTxKey(9),
    tombstone_reason_hash: null,
    rooms: [],
  });
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
  const pricedJoin = await execute(
    contract,
    storage,
    'joinEnclave',
    providerJoin,
    provider.publicKey,
    7
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
    8
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

test('MayhemContract provider serving price gate fails closed without current admin key', async () => {
  const admin = await makeIdentity();
  const provider = await makeIdentity();
  const storage = new MemoryStorage();
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
    5
  );
  assert.match(unsupportedUpdate.message, /unsupported enclave caps field.*price_ver/i);

  const stored = await storage.get(`enclave/${'a'.repeat(64)}`);
  assert.deepEqual(stored.value.caps, catalogStyleCaps);
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

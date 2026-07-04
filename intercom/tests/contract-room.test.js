import assert from 'node:assert/strict';
import test from 'node:test';
import MayhemContract, { deriveRoomId, roomSidechannelName } from '../contract/contract.js';
import {
  MemoryStorage,
  execute,
  makeIdentity,
  makeTxKey,
  makeVerifier,
  seedCurrentAdminPrice,
  signConsent,
} from './helpers/contract.js';

const rulesHash = 'd'.repeat(64);
const enclaveId = 'e'.repeat(64);
const otherEnclaveId = 'f'.repeat(64);
const modelId = 'meta/llama-3.1-8b-instruct@4bit';
const otherModelId = 'qwen/qwen2.5-4b-instruct@4bit';

const providerRegistration = {
  op: 'register_provider',
};

const enclaveRegistration = {
  op: 'register_enclave',
  enclave_id: enclaveId,
  model_id: modelId,
  backend: 'llama.cpp',
  artifact_root: 'a'.repeat(64),
  artifact_root_kind: 'blake3_merkle_v1',
  artifact_source: {
    kind: 'huggingface',
    repo: 'mayhem-test/llama-3.1-8b-instruct-GGUF',
    revision: '1'.repeat(40),
    path: 'llama-3.1-8b-instruct-Q4_K_M.gguf',
  },
  manifest_hash: 'b'.repeat(64),
  att_tier: 1,
  binary_hash: 'c'.repeat(64),
  caps: {
    chat: true,
    tools: false,
    ctx: 32768,
  },
};

async function setupRoomAdmin(provider) {
  const admin = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const protocol = provider ? { peer: { wallet: makeVerifier(provider.wallet) } } : {};
  const contract = new MayhemContract(protocol, {});
  return { contract, storage, admin };
}

test('MayhemContract canonical room is admin-opened and provider-joined with served enclave', async () => {
  const provider = await makeIdentity();
  const { contract, storage: peerAStorage, admin } = await setupRoomAdmin(provider);
  const outsider = await makeIdentity();
  const nonce = 'room-nonce-001';
  const expectedRoomId = await deriveRoomId(enclaveId, admin.publicKey, nonce);
  const expectedSidechannel = roomSidechannelName(expectedRoomId);
  const policy = {
    min_reputation: 0,
    max_price_mult: 1,
    region_hint: 'eu',
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
      value: enclaveRegistration,
      sender: admin.publicKey,
      txNo: 4,
    },
  ]) {
    const result = await execute(contract, peerAStorage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }
  await seedCurrentAdminPrice(peerAStorage, {
    enclaveId,
    modelId,
    admin: admin.publicKey,
    txNo: 5,
  });

  const nonAdminOpen = await execute(
    contract,
    peerAStorage,
    'openRoom',
    {
      op: 'open_room',
      enclave_id: enclaveId,
      model_id: modelId,
      nonce,
      label: 'eu-central',
      policy,
    },
    outsider.publicKey,
    5
  );
  assert.match(nonAdminOpen.message, /admin required/i);

  const opened = await execute(
    contract,
    peerAStorage,
    'openRoom',
    {
      op: 'open_room',
      enclave_id: enclaveId,
      model_id: modelId,
      nonce,
      label: 'eu-central',
      policy,
    },
    admin.publicKey,
    6
  );
  assert.deepEqual(opened, {
    ok: true,
    op: 'openRoom',
    room_id: expectedRoomId,
    sidechannel: expectedSidechannel,
  });

  const joinBeforeServing = await execute(
    contract,
    peerAStorage,
    'joinRoom',
    {
      op: 'join_room',
      room_id: expectedRoomId,
      enclave_id: enclaveId,
    },
    provider.publicKey,
    7
  );
  assert.match(joinBeforeServing.message, /not serving enclave/i);

  const providerServesEnclave = await execute(
    contract,
    peerAStorage,
    'joinEnclave',
    {
      op: 'join_enclave',
      enclave_id: enclaveId,
    },
    provider.publicKey,
    8
  );
  assert.equal(providerServesEnclave.ok, true, providerServesEnclave.message);

  const joined = await execute(
    contract,
    peerAStorage,
    'joinRoom',
    {
      op: 'join_room',
      room_id: expectedRoomId,
      enclave_id: enclaveId,
    },
    provider.publicKey,
    9
  );
  assert.deepEqual(joined, {
    ok: true,
    op: 'joinRoom',
    room_id: expectedRoomId,
    provider: provider.publicKey,
    enclave_id: enclaveId,
    sidechannel: expectedSidechannel,
  });

  const peerBStorage = MemoryStorage.fromSnapshotBytes(peerAStorage.snapshotBytes());
  const discovered = await peerBStorage.get(`room/${expectedRoomId}`);
  assert.deepEqual(discovered.value, {
    room_id: expectedRoomId,
    sidechannel: expectedSidechannel,
    enclave_id: enclaveId,
    model_id: modelId,
    label: 'eu-central',
    creator: admin.publicKey,
    creator_role: 'admin',
    policy,
    serves: [{ provider: provider.publicKey, enclave_id: enclaveId }],
    serves_updated_at: makeTxKey(9),
    created_at: makeTxKey(6),
    updated_at: makeTxKey(6),
    closed_at: null,
    status: 'open',
  });

  const roomServing = await peerBStorage.get(`roomserve/${expectedRoomId}/${provider.publicKey}/${enclaveId}`);
  assert.deepEqual(roomServing.value, {
    room_id: expectedRoomId,
    sidechannel: expectedSidechannel,
    provider: provider.publicKey,
    enclave_id: enclaveId,
    model_id: modelId,
    status: 'active',
    joined_at: makeTxKey(9),
    updated_at: makeTxKey(9),
    left_at: null,
  });

  const serving = await peerBStorage.get(`serve/${provider.publicKey}/${enclaveId}`);
  assert.deepEqual(serving.value.rooms, [expectedRoomId]);
  assert.equal(serving.value.updated_at, makeTxKey(9));

  const leaveEnclaveWithRoom = await execute(
    contract,
    peerAStorage,
    'leaveEnclave',
    {
      op: 'leave_enclave',
      enclave_id: enclaveId,
    },
    provider.publicKey,
    10
  );
  assert.match(leaveEnclaveWithRoom.message, /leave rooms before leaving enclave/i);

  const closed = await execute(
    contract,
    peerAStorage,
    'closeRoom',
    {
      op: 'close_room',
      room_id: expectedRoomId,
    },
    admin.publicKey,
    11
  );
  assert.deepEqual(closed, {
    ok: true,
    op: 'closeRoom',
    room_id: expectedRoomId,
    sidechannel: expectedSidechannel,
    tombstoned_serves: [{ provider: provider.publicKey, enclave_id: enclaveId }],
  });

  const peerBAfterClose = MemoryStorage.fromSnapshotBytes(peerAStorage.snapshotBytes());
  const closedRoom = await peerBAfterClose.get(`room/${expectedRoomId}`);
  assert.equal(closedRoom.value.status, 'closed');
  assert.equal(closedRoom.value.updated_at, makeTxKey(11));
  assert.equal(closedRoom.value.closed_at, makeTxKey(11));
  assert.equal(closedRoom.value.closed_by, admin.publicKey);
  assert.equal(closedRoom.value.closed_by_role, 'admin');
  assert.deepEqual(closedRoom.value.serves, []);
  assert.equal(closedRoom.value.serves_updated_at, makeTxKey(11));
  assert.deepEqual(closedRoom.value.tombstoned_serves, [{ provider: provider.publicKey, enclave_id: enclaveId }]);

  const tombstonedRoomServing = await peerBAfterClose.get(`roomserve/${expectedRoomId}/${provider.publicKey}/${enclaveId}`);
  assert.equal(tombstonedRoomServing.value.status, 'tombstoned');
  assert.equal(tombstonedRoomServing.value.tombstoned_at, makeTxKey(11));
  const servingAfterClose = await peerBAfterClose.get(`serve/${provider.publicKey}/${enclaveId}`);
  assert.deepEqual(servingAfterClose.value.rooms, []);
  assert.equal(servingAfterClose.value.updated_at, makeTxKey(11));

  const leave = await execute(
    contract,
    peerAStorage,
    'leaveRoom',
    {
      op: 'leave_room',
      room_id: expectedRoomId,
      enclave_id: enclaveId,
    },
    provider.publicKey,
    12
  );
  assert.deepEqual(leave, {
    ok: true,
    op: 'leaveRoom',
    room_id: expectedRoomId,
    provider: provider.publicKey,
    enclave_id: enclaveId,
    sidechannel: expectedSidechannel,
    status: 'tombstoned',
    idempotent: true,
  });

  const leftRoomServing = await peerAStorage.get(`roomserve/${expectedRoomId}/${provider.publicKey}/${enclaveId}`);
  assert.equal(leftRoomServing.value.status, 'tombstoned');
  assert.equal(leftRoomServing.value.tombstoned_at, makeTxKey(11));

  const servingAfterLeaveRoom = await peerAStorage.get(`serve/${provider.publicKey}/${enclaveId}`);
  assert.deepEqual(servingAfterLeaveRoom.value.rooms, []);
  assert.equal(servingAfterLeaveRoom.value.updated_at, makeTxKey(11));
});

test('MayhemContract validates admin room policy as canonical routing controls', async () => {
  const { contract, storage, admin } = await setupRoomAdmin();
  const registered = await execute(
    contract,
    storage,
    'registerEnclave',
    enclaveRegistration,
    admin.publicKey,
    1
  );
  assert.equal(registered.ok, true, registered.message);

  const unsupported = await execute(
    contract,
    storage,
    'openRoom',
    {
      op: 'open_room',
      enclave_id: enclaveId,
      nonce: 'policy-side-terms',
      label: 'bad-policy',
      policy: {
        region_hint: 'eu',
        provider_fee_bps: 250,
      },
    },
    admin.publicKey,
    2
  );
  assert.match(unsupported.message, /unsupported room policy field: provider_fee_bps/i);
  const rejectedRoomId = await deriveRoomId(enclaveId, admin.publicKey, 'policy-side-terms');
  assert.equal(await storage.get(`room/${rejectedRoomId}`), null);

  for (const [index, [policy, message]] of [
    [{ min_reputation: 2 }, /min_reputation must be between 0 and 1/i],
    [{ max_price_mult: 0 }, /max_price_mult must be positive/i],
    [{ region_hint: '' }, /region_hint must be a non-empty string/i],
    [{ canary_set: ['canary-dev-v1'] }, /canary_set must be a non-empty string/i],
  ].entries()) {
    const result = await execute(
      contract,
      storage,
      'openRoom',
      {
        op: 'open_room',
        enclave_id: enclaveId,
        nonce: `policy-invalid-${Object.keys(policy)[0]}`,
        label: 'invalid-policy',
        policy,
      },
      admin.publicKey,
      3 + index
    );
    assert.match(result.message, message);
  }

  const validPolicy = {
    region_hint: 'eu',
    canary_set: 'canary-dev-v1',
    min_reputation: 0.5,
    max_price_mult: 1.25,
  };
  const opened = await execute(
    contract,
    storage,
    'openRoom',
    {
      op: 'open_room',
      enclave_id: enclaveId,
      nonce: 'policy-good',
      label: 'good-policy',
      policy: validPolicy,
    },
    admin.publicKey,
    7
  );
  assert.equal(opened.ok, true, opened.message);

  const roomId = await deriveRoomId(enclaveId, admin.publicKey, 'policy-good');
  const room = await storage.get(`room/${roomId}`);
  assert.deepEqual(room.value.policy, validPolicy);
});

test('MayhemContract rejects unsafe canonical room identifiers', async () => {
  const provider = await makeIdentity();
  const { contract, storage, admin } = await setupRoomAdmin(provider);

  const badEnclaveRoom = await execute(
    contract,
    storage,
    'openRoom',
    {
      op: 'open_room',
      enclave_id: 'bad/enclave',
      nonce: 'bad-enclave-room',
      label: 'bad-enclave-room',
      policy: {},
    },
    admin.publicKey,
    1
  );
  assert.match(badEnclaveRoom.message, /invalid enclave id/i);

  const badModelRoom = await execute(
    contract,
    storage,
    'openRoom',
    {
      op: 'open_room',
      enclave_id: enclaveId,
      model_id: '/provider/model',
      nonce: 'bad-model-room',
      label: 'bad-model-room',
      policy: {},
    },
    admin.publicKey,
    2
  );
  assert.match(badModelRoom.message, /invalid model id/i);

  const badClose = await execute(
    contract,
    storage,
    'closeRoom',
    {
      op: 'close_room',
      room_id: 'bad/room',
    },
    admin.publicKey,
    3
  );
  assert.match(badClose.message, /invalid room id/i);

  const badJoinRoom = await execute(
    contract,
    storage,
    'joinRoom',
    {
      op: 'join_room',
      room_id: 'bad/room',
      enclave_id: enclaveId,
    },
    provider.publicKey,
    4
  );
  assert.match(badJoinRoom.message, /invalid room id/i);

  const badJoinEnclave = await execute(
    contract,
    storage,
    'joinRoom',
    {
      op: 'join_room',
      room_id: 'room-safe',
      enclave_id: 'bad/enclave',
    },
    provider.publicKey,
    5
  );
  assert.match(badJoinEnclave.message, /invalid enclave id/i);

  const badLeaveRoom = await execute(
    contract,
    storage,
    'leaveRoom',
    {
      op: 'leave_room',
      room_id: 'bad/room',
      enclave_id: enclaveId,
    },
    provider.publicKey,
    6
  );
  assert.match(badLeaveRoom.message, /invalid room id/i);

  const badLeaveEnclave = await execute(
    contract,
    storage,
    'leaveRoom',
    {
      op: 'leave_room',
      room_id: 'room-safe',
      enclave_id: 'bad/enclave',
    },
    provider.publicKey,
    7
  );
  assert.match(badLeaveEnclave.message, /invalid enclave id/i);
});

test('MayhemContract room serving rejects explicit non-admin authority markers', async () => {
  const provider = await makeIdentity();
  const { contract, storage, admin } = await setupRoomAdmin(provider);
  const nonce = 'polluted-room';
  const roomId = await deriveRoomId(enclaveId, admin.publicKey, nonce);

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
    modelId,
    admin: admin.publicKey,
    txNo: 5,
  });

  const enclave = (await storage.get(`enclave/${enclaveId}`)).value;
  await storage.put(`enclave/${enclaveId}`, {
    ...enclave,
    created_by_role: 'provider',
  });
  const pollutedEnclaveOpen = await execute(
    contract,
    storage,
    'openRoom',
    {
      op: 'open_room',
      enclave_id: enclaveId,
      model_id: modelId,
      nonce,
      label: 'polluted',
      policy: {},
    },
    admin.publicKey,
    6
  );
  assert.match(pollutedEnclaveOpen.message, /admin-created enclave/i);
  assert.equal(await storage.get(`room/${roomId}`), null);

  await storage.put(`enclave/${enclaveId}`, {
    ...enclave,
    created_by_role: 'admin',
  });
  const opened = await execute(
    contract,
    storage,
    'openRoom',
    {
      op: 'open_room',
      enclave_id: enclaveId,
      model_id: modelId,
      nonce,
      label: 'polluted',
      policy: {},
    },
    admin.publicKey,
    7
  );
  assert.equal(opened.ok, true, opened.message);

  const joinedEnclave = await execute(
    contract,
    storage,
    'joinEnclave',
    {
      op: 'join_enclave',
      enclave_id: enclaveId,
    },
    provider.publicKey,
    8
  );
  assert.equal(joinedEnclave.ok, true, joinedEnclave.message);

  const room = (await storage.get(`room/${roomId}`)).value;
  await storage.put(`room/${roomId}`, {
    ...room,
    creator_role: 'provider',
  });
  const beforeRoomJoin = storage.snapshotBytes();
  const pollutedRoomJoin = await execute(
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
  assert.match(pollutedRoomJoin.message, /admin-created room/i);
  assert.equal(storage.snapshotBytes(), beforeRoomJoin);
  assert.equal(await storage.get(`roomserve/${roomId}/${provider.publicKey}/${enclaveId}`), null);
  assert.deepEqual((await storage.get(`serve/${provider.publicKey}/${enclaveId}`)).value.rooms, []);
});

test('MayhemContract rejects model-only rooms because canonical rooms are enclave-scoped', async () => {
  const provider = await makeIdentity();
  const { contract, storage, admin } = await setupRoomAdmin(provider);
  const nonce = 'model-only-room';
  const modelOnlyRoomId = await deriveRoomId(modelId, admin.publicKey, nonce);

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
    modelId,
    admin: admin.publicKey,
    txNo: 5,
  });

  const opened = await execute(
    contract,
    storage,
    'openRoom',
    {
      op: 'open_room',
      model_id: modelId,
      nonce,
      label: 'model-only',
      policy: {},
    },
    admin.publicKey,
    6
  );
  assert.match(opened.message, /invalid schema/i);
  assert.equal(await storage.get(`room/${modelOnlyRoomId}`), null);

  const providerServesEnclave = await execute(
    contract,
    storage,
    'joinEnclave',
    {
      op: 'join_enclave',
      enclave_id: enclaveId,
    },
    provider.publicKey,
    7
  );
  assert.equal(providerServesEnclave.ok, true, providerServesEnclave.message);
});

test('MayhemContract rejects non-canonical and wrong-enclave room offers', async () => {
  const provider = await makeIdentity();
  const { contract, storage, admin } = await setupRoomAdmin(provider);
  const roomId = await deriveRoomId(enclaveId, admin.publicKey, 'canonical-room');

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
      type: 'registerEnclave',
      value: {
        ...enclaveRegistration,
        enclave_id: otherEnclaveId,
        model_id: modelId,
        artifact_root: '1'.repeat(64),
      },
      sender: admin.publicKey,
      txNo: 5,
    },
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }
  await seedCurrentAdminPrice(storage, {
    enclaveId: otherEnclaveId,
    modelId,
    admin: admin.publicKey,
    txNo: 6,
  });

  const providerServesOther = await execute(
    contract,
    storage,
    'joinEnclave',
    {
      op: 'join_enclave',
      enclave_id: otherEnclaveId,
    },
    provider.publicKey,
    7
  );
  assert.equal(providerServesOther.ok, true, providerServesOther.message);

  const nonCanonicalJoin = await execute(
    contract,
    storage,
    'joinRoom',
    {
      op: 'join_room',
      room_id: 'mx/room/provider-local-only',
      enclave_id: otherEnclaveId,
    },
    provider.publicKey,
    8
  );
  assert.match(nonCanonicalJoin.message, /invalid room id/i);

  const wrongModelHint = await execute(
    contract,
    storage,
    'openRoom',
    {
      op: 'open_room',
      enclave_id: enclaveId,
      model_id: otherModelId,
      nonce: 'canonical-room',
      label: 'eu-central',
      policy: {},
    },
    admin.publicKey,
    9
  );
  assert.match(wrongModelHint.message, /room model does not match enclave model/i);

  const opened = await execute(
    contract,
    storage,
    'openRoom',
    {
      op: 'open_room',
      enclave_id: enclaveId,
      model_id: modelId,
      nonce: 'canonical-room',
      label: 'eu-central',
      policy: {},
    },
    admin.publicKey,
    10
  );
  assert.equal(opened.ok, true, opened.message);

  const wrongEnclaveJoin = await execute(
    contract,
    storage,
    'joinRoom',
    {
      op: 'join_room',
      room_id: roomId,
      enclave_id: otherEnclaveId,
    },
    provider.publicKey,
    11
  );
  assert.match(wrongEnclaveJoin.message, /room enclave does not match/i);
});

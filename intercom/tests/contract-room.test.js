import assert from 'node:assert/strict';
import test from 'node:test';
import MayhemContract, { deriveRoomId, roomSidechannelName } from '../contract/contract.js';
import {
  MemoryStorage,
  execute,
  makeIdentity,
  makeTxKey,
  makeVerifier,
  signConsent,
} from './helpers/contract.js';

const rulesHash = 'd'.repeat(64);
const enclaveId = 'e'.repeat(64);
const otherEnclaveId = 'f'.repeat(64);
const modelId = 'meta/llama-3.1-8b-instruct@4bit';
const otherModelId = 'qwen/qwen2.5-4b-instruct@4bit';

const providerRegistration = {
  op: 'register_provider',
  payout_addr: 'trac1providerpayouttarget',
  payout_method: 'tnk',
};

const enclaveRegistration = {
  op: 'register_enclave',
  enclave_id: enclaveId,
  model_id: modelId,
  backend: 'llama.cpp',
  artifact_root: 'a'.repeat(64),
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
  const expectedRoomId = await deriveRoomId(modelId, admin.publicKey, nonce);
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

  const nonAdminOpen = await execute(
    contract,
    peerAStorage,
    'openRoom',
    {
      op: 'open_room',
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
    model_id: modelId,
    label: 'eu-central',
    creator: admin.publicKey,
    policy,
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
  });

  const peerBAfterClose = MemoryStorage.fromSnapshotBytes(peerAStorage.snapshotBytes());
  const closedRoom = await peerBAfterClose.get(`room/${expectedRoomId}`);
  assert.equal(closedRoom.value.status, 'closed');
  assert.equal(closedRoom.value.updated_at, makeTxKey(11));
  assert.equal(closedRoom.value.closed_at, makeTxKey(11));

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
  });

  const leftRoomServing = await peerAStorage.get(`roomserve/${expectedRoomId}/${provider.publicKey}/${enclaveId}`);
  assert.equal(leftRoomServing.value.status, 'inactive');
  assert.equal(leftRoomServing.value.left_at, makeTxKey(12));

  const servingAfterLeaveRoom = await peerAStorage.get(`serve/${provider.publicKey}/${enclaveId}`);
  assert.deepEqual(servingAfterLeaveRoom.value.rooms, []);
  assert.equal(servingAfterLeaveRoom.value.updated_at, makeTxKey(12));
});

test('MayhemContract rejects non-canonical and wrong-model room offers', async () => {
  const provider = await makeIdentity();
  const { contract, storage, admin } = await setupRoomAdmin(provider);
  const roomId = await deriveRoomId(modelId, admin.publicKey, 'canonical-room');

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
      value: {
        ...enclaveRegistration,
        enclave_id: otherEnclaveId,
        model_id: otherModelId,
      },
      sender: admin.publicKey,
      txNo: 4,
    },
    {
      type: 'joinEnclave',
      value: {
        op: 'join_enclave',
        enclave_id: otherEnclaveId,
      },
      sender: provider.publicKey,
      txNo: 5,
    },
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }

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
    6
  );
  assert.match(nonCanonicalJoin.message, /room not found/i);

  const opened = await execute(
    contract,
    storage,
    'openRoom',
    {
      op: 'open_room',
      model_id: modelId,
      nonce: 'canonical-room',
      label: 'eu-central',
      policy: {},
    },
    admin.publicKey,
    7
  );
  assert.equal(opened.ok, true, opened.message);

  const wrongModelJoin = await execute(
    contract,
    storage,
    'joinRoom',
    {
      op: 'join_room',
      room_id: roomId,
      enclave_id: otherEnclaveId,
    },
    provider.publicKey,
    8
  );
  assert.match(wrongModelJoin.message, /model does not match/i);
});

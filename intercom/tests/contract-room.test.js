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

const rulesHash = '7'.repeat(64);

async function setupConsentedRoomCreator() {
  const creator = await makeIdentity();
  const storage = new MemoryStorage({ admin: creator.publicKey });
  const protocol = { peer: { wallet: makeVerifier(creator.wallet) } };
  const contract = new MayhemContract(protocol, {});

  await execute(
    contract,
    storage,
    'setRules',
    { op: 'set_rules', ver: 1, hash: rulesHash },
    creator.publicKey,
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
      sig: signConsent(creator.wallet, 1, rulesHash),
    },
    creator.publicKey,
    2
  );

  return { contract, storage, creator };
}

test('MayhemContract room opened on peer A is discoverable from peer B state', async () => {
  const { contract, storage: peerAStorage, creator } = await setupConsentedRoomCreator();
  const modelId = 'meta/llama-3.1-8b-instruct@4bit';
  const nonce = 'room-nonce-001';
  const expectedRoomId = await deriveRoomId(modelId, creator.publicKey, nonce);
  const expectedSidechannel = roomSidechannelName(expectedRoomId);
  const policy = {
    min_reputation: 0,
    max_price_mult: 1,
    region_hint: 'eu',
  };

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
    creator.publicKey,
    3
  );
  assert.deepEqual(opened, {
    ok: true,
    op: 'openRoom',
    room_id: expectedRoomId,
    sidechannel: expectedSidechannel,
  });

  const peerBStorage = MemoryStorage.fromSnapshotBytes(peerAStorage.snapshotBytes());
  const discovered = await peerBStorage.get(`room/${expectedRoomId}`);
  assert.deepEqual(discovered.value, {
    room_id: expectedRoomId,
    sidechannel: expectedSidechannel,
    model_id: modelId,
    label: 'eu-central',
    creator: creator.publicKey,
    policy,
    created_at: makeTxKey(3),
    updated_at: makeTxKey(3),
    closed_at: null,
    status: 'open',
  });

  const closed = await execute(
    contract,
    peerAStorage,
    'closeRoom',
    {
      op: 'close_room',
      room_id: expectedRoomId,
    },
    creator.publicKey,
    4
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
  assert.equal(closedRoom.value.updated_at, makeTxKey(4));
  assert.equal(closedRoom.value.closed_at, makeTxKey(4));
});

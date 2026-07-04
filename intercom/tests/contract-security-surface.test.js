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
const modelId = 'mayhem/security-review-model@q4';
const enclaveId = '2'.repeat(64);
const roomNonce = 'security-review-room';

const enclaveRegistration = {
  op: 'register_enclave',
  enclave_id: enclaveId,
  model_id: modelId,
  backend: 'llama.cpp',
  artifact_root: '3'.repeat(64),
  manifest_hash: '4'.repeat(64),
  att_tier: 1,
  binary_hash: '5'.repeat(64),
  caps: { chat: true, tools: false, ctx: 32768 },
};

async function setupSecuritySurface() {
  const admin = await makeIdentity();
  const provider = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const contract = new MayhemContract({ peer: { wallet: makeVerifier(provider.wallet) } }, {});

  await storage.put(`modelref/${modelId}`, {
    model_id: modelId,
    price_ref_mu: {
      in_per_1k: 20,
      out_per_1k: 60,
    },
  });

  const roomId = await deriveRoomId(enclaveId, admin.publicKey, roomNonce);
  const bootstrap = [
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
    {
      type: 'registerEnclave',
      value: enclaveRegistration,
      sender: admin.publicKey,
      txNo: 4,
    },
    {
      type: 'openRoom',
      value: {
        op: 'open_room',
        enclave_id: enclaveId,
        model_id: modelId,
        nonce: roomNonce,
        label: 'security-review',
        policy: { region_hint: 'local' },
      },
      sender: admin.publicKey,
      txNo: 5,
    },
  ];

  for (const op of bootstrap) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }
  await seedCurrentAdminPrice(storage, {
    enclaveId,
    modelId,
    admin: admin.publicKey,
    txNo: 6,
    inPer1kMu: 20,
    outPer1kMu: 60,
    minSessionMu: 100,
  });

  return { admin, provider, roomId, storage, contract };
}

test('MayhemContract keeps providers out of canonical economy and control-plane ops', async () => {
  const { admin, provider, roomId, storage, contract } = await setupSecuritySurface();
  const providerSender = provider.publicKey;
  const adminOnlyAttempts = [
    ['setRules', { op: 'set_rules', ver: 2, hash: '6'.repeat(64) }],
    [
      'setParams',
      {
        op: 'set_params',
        submitted_at: 0,
        effective_at: 86_400,
        values: { fee_bps: 1_000 },
      },
    ],
    [
      'setProviderPayout',
      {
        op: 'set_provider_payout',
        provider: provider.publicKey,
        payout_addr: 'provider-picked-target',
        payout_method: 'tnk',
      },
    ],
    [
      'banProvider',
      {
        op: 'ban_provider',
        provider: provider.publicKey,
        reason_hash: '7'.repeat(64),
      },
    ],
    [
      'setModelRef',
      {
        op: 'set_model_ref',
        model_id: 'provider/arbitrary-model@q4',
        price_ref_mu: {
          in_per_1k: 1,
          out_per_1k: 1,
        },
      },
    ],
    [
      'registerEnclave',
      {
        ...enclaveRegistration,
        enclave_id: '8'.repeat(64),
        model_id: 'provider/arbitrary-model@q4',
      },
    ],
    [
      'updateEnclave',
      {
        op: 'update_enclave',
        enclave_id: enclaveId,
        artifact_root: '9'.repeat(64),
      },
    ],
    ['retireEnclave', { op: 'retire_enclave', enclave_id: enclaveId }],
    [
      'openRoom',
      {
        op: 'open_room',
        enclave_id: enclaveId,
        model_id: modelId,
        nonce: 'provider-room',
        label: 'provider-room',
        policy: {},
      },
    ],
    ['closeRoom', { op: 'close_room', room_id: roomId }],
    [
      'setPrice',
      {
        op: 'set_price',
        enclave_id: enclaveId,
        in_per_1k_mu: 18,
        out_per_1k_mu: 55,
        per_req_mu: 0,
        min_session_mu: 100,
        effective_at: 21_600,
      },
    ],
    [
      'recordReputationEvent',
      {
        op: 'record_reputation_event',
        provider: provider.publicKey,
        event_id: 'provider-forged-event',
        kind: 'session_ok',
        epoch: 1,
        at: 3_600,
        paid_mu: 100,
        evidence_hash: 'a'.repeat(64),
      },
    ],
    [
      'anchorReputation',
      {
        op: 'anchor_reputation',
        provider: provider.publicKey,
        epoch: 1,
        folded_at: 3_600,
        events_head: 'b'.repeat(64),
        r_bps: 9_000,
        raw_milli: 20_000,
        successful_sessions: 100,
      },
    ],
    [
      'epochApply',
      {
        op: 'epoch_apply',
        epoch: 1,
        at: 3_600,
        debits: [],
        earnings: [{ provider: provider.publicKey, gross_mu: 1_000 }],
      },
    ],
    [
      'rateOracle',
      {
        op: 'rate_oracle',
        tnk_usd_e6: 2_000_000,
        source: 'gate-spot',
        ts: 3_600,
      },
    ],
    [
      'tnkDeposit',
      {
        op: 'tnk_deposit',
        memo_hash: 'memo-security',
        tnk_e18: '1000000000000000000',
        msb_tx_hash: 'msb-security',
        epoch: 1,
        at: 3_600,
      },
    ],
    [
      'fiatDeposit',
      {
        op: 'fiat_deposit',
        rail: 'stripe',
        who: provider.publicKey,
        mu: 1_000_000,
        ext_ref_hash: 'c'.repeat(64),
        fiat_currency: 'usd',
        fiat_amount_minor: 100,
        epoch: 1,
        at: 3_600,
      },
    ],
    [
      'fiatChargeback',
      {
        op: 'fiat_chargeback',
        rail: 'stripe',
        who: provider.publicKey,
        mu: 1_000_000,
        ext_ref_hash: 'c'.repeat(64),
        dispute_ref_hash: 'd'.repeat(64),
        fiat_currency: 'usd',
        fiat_amount_minor: 100,
        epoch: 1,
        at: 3_600,
      },
    ],
    [
      'payoutConfirm',
      {
        op: 'payout_confirm',
        kind: 'provider',
        epoch: 1,
        who: provider.publicKey,
        mu: 100,
        tnk_e18: '1000000000000000000',
        msb_tx_hash: 'pay-security',
        at: 3_600,
      },
    ],
    [
      'disputeResolve',
      {
        op: 'dispute_resolve',
        dispute_id: 1,
        outcome: 'provider_fault',
        deposit_action: 'refund',
        rationale_hash: 'c'.repeat(64),
        slash: true,
        at: 3_600,
      },
    ],
  ];

  let txNo = 6;
  for (const [type, value] of adminOnlyAttempts) {
    const before = storage.snapshotBytes();
    const result = await execute(contract, storage, type, value, providerSender, txNo);
    assert.match(result.message, /admin required/i, `${type} should be admin-only`);
    assert.equal(storage.snapshotBytes(), before, `${type} must not mutate state`);
    txNo += 1;
  }

  const joinedEnclave = await execute(
    contract,
    storage,
    'joinEnclave',
    { op: 'join_enclave', enclave_id: enclaveId },
    provider.publicKey,
    txNo
  );
  assert.deepEqual(joinedEnclave, {
    ok: true,
    op: 'joinEnclave',
    provider: provider.publicKey,
    enclave_id: enclaveId,
  });

  const joinedRoom = await execute(
    contract,
    storage,
    'joinRoom',
    { op: 'join_room', room_id: roomId, enclave_id: enclaveId },
    provider.publicKey,
    txNo + 1
  );
  assert.equal(joinedRoom.ok, true, joinedRoom.message);
  assert.equal(joinedRoom.op, 'joinRoom');
  assert.equal(joinedRoom.provider, provider.publicKey);
  assert.equal(joinedRoom.room_id, roomId);

  const providerRecord = await storage.get(`prov/${provider.publicKey}`);
  assert.equal(providerRecord.value.status, 'active');
  assert.equal(providerRecord.value.payout, null);
  assert.equal((await storage.get(`enclave/${enclaveId}`)).value.created_by, admin.publicKey);
  assert.equal((await storage.get(`enclave/${enclaveId}`)).value.created_by_role, 'admin');
  assert.equal((await storage.get(`room/${roomId}`)).value.creator, admin.publicKey);
  assert.equal((await storage.get(`room/${roomId}`)).value.creator_role, 'admin');
  assert.equal(await storage.get('rules/2'), null);
  assert.equal(await storage.get('modelref/provider/arbitrary-model@q4'), null);
  assert.equal(await storage.get(`enclave/${'8'.repeat(64)}`), null);
  const price = await storage.get(`price/${enclaveId}`);
  assert.equal(price.value.current.set_by, admin.publicKey);
  assert.equal(price.value.current.set_by_role, 'admin');
  assert.equal(price.value.current.ver, 1);
  assert.equal((await storage.get(`serve/${provider.publicKey}/${enclaveId}`)).value.joined_at, makeTxKey(txNo));
});

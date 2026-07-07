import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import MayhemContract, { deriveRoomId } from '../contract/contract.js';
import {
  MemoryStorage,
  execute,
  executeDepositFeature,
  executeEpochApplyFeature,
  executeRateFeature,
  makeIdentity,
  makeTxKey,
  makeVerifier,
  seedCurrentAdminPrice,
  signConsent,
  textRateMap,
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
  artifact_root_kind: 'blake3_merkle_v1',
  artifact_source: {
    kind: 'huggingface',
    repo: 'mayhem-test/security-review-model',
    revision: '6'.repeat(40),
    path: 'security-review-model.gguf',
  },
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
    rate_map: textRateMap(20, 60),
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

const buildAdminOnlyAttempts = (provider, roomId) => [
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
      rate_map: textRateMap(1, 1),
    },
  ],
  [
    'publishCatalog',
    {
      op: 'publish_catalog',
      catalog_id: 'provider-catalog',
      source_kind: 'huggingface',
      catalog_url: 'https://huggingface.co/provider/fake-catalog/resolve/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/models.json',
      signature_url: 'https://huggingface.co/provider/fake-catalog/resolve/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/models.json.sig',
      catalog_hash: '8'.repeat(64),
      signature_hash: '9'.repeat(64),
      key_id: 'provider-key',
      public_key: 'a'.repeat(64),
      model_count: 1,
      artifact_count: 1,
      canaries: [{
        set_id: 'provider-canary',
        url: 'https://huggingface.co/provider/fake-catalog/resolve/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/canaries/provider-canary.json',
        hash: 'b'.repeat(64),
      }],
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
      rate_map: textRateMap(18, 55),
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

const buildAdminOnlyFeatureAttempts = (provider) => [
  {
    op: 'tnk_deposit',
    memo_hash: 'memo-security',
    tnk_e18: '1000000000000000000',
    msb_tx_hash: 'msb-security',
    epoch: 1,
    at: 3_600,
  },
  {
    op: 'tap_deposit',
    who: '0x1111111111111111111111111111111111111111',
    tap_wei: '1000000000000000000',
    eth_tx_hash: `0x${'e'.repeat(64)}`,
    log_index: 0,
    block_number: 123,
    pool_address: '0x2222222222222222222222222222222222222222',
    chain_id: 61_000,
    epoch: 1,
    at: 3_600,
  },
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
];

const buildAdminOnlyRateFeatureAttempts = () => [
  {
    op: 'rate_oracle',
    tnk_usd_e6: 2_000_000,
    source: 'gate-spot',
    ts: 3_600,
  },
  {
    op: 'tap_rate_oracle',
    tap_usd_e6: 2_000_000,
    source: 'uniswap-v2',
    ts: 3_600,
  },
];

test('MayhemContract replicated source avoids locale and env-dependent admission knobs', () => {
  const contractSource = fs.readFileSync(new URL('../contract/contract.js', import.meta.url), 'utf8');
  const protocolSource = fs.readFileSync(new URL('../contract/protocol.js', import.meta.url), 'utf8');

  assert.doesNotMatch(contractSource, /localeCompare/);
  assert.doesNotMatch(contractSource, /MAYHEM_TX_MAX_BYTES/);
  assert.match(protocolSource, /MAYHEM_TX_MAX_BYTES/);
});

test('MayhemContract rejects admin ops before genesis admin is present', async () => {
  const outsider = await makeIdentity();
  const storage = new MemoryStorage();
  const contract = new MayhemContract({ peer: { wallet: makeVerifier(outsider.wallet) } }, {});

  let txNo = 1;
  for (const [type, value] of buildAdminOnlyAttempts(outsider, 'missing-room')) {
    const before = storage.snapshotBytes();
    const result = await execute(contract, storage, type, value, outsider.publicKey, txNo);
    assert.match(result.message, /admin required/i, `${type} should require genesis admin`);
    assert.equal(storage.snapshotBytes(), before, `${type} must not mutate state before genesis admin`);
    txNo += 1;
  }

  const beforeFeature = storage.snapshotBytes();
  const featureResult = await executeEpochApplyFeature(
    contract,
    storage,
    {
      op: 'epoch_apply',
      epoch: 1,
      at: 3_600,
      debits: [],
      earnings: [{ rail: 'fiat', provider: outsider.publicKey, gross_mu: 1_000 }],
    },
    outsider.publicKey
  );
  assert.match(featureResult.message, /admin required/i, 'epochApply feature should require genesis admin');
  assert.equal(storage.snapshotBytes(), beforeFeature, 'epochApply feature must not mutate before genesis admin');

  for (const value of buildAdminOnlyFeatureAttempts(outsider)) {
    const before = storage.snapshotBytes();
    const result = await executeDepositFeature(contract, storage, value, outsider.publicKey);
    assert.match(result.message, /admin required/i, `${value.op} feature should require genesis admin`);
    assert.equal(storage.snapshotBytes(), before, `${value.op} feature must not mutate before genesis admin`);
  }

  for (const value of buildAdminOnlyRateFeatureAttempts()) {
    const before = storage.snapshotBytes();
    const result = await executeRateFeature(contract, storage, value, outsider.publicKey);
    assert.match(result.message, /admin required/i, `${value.op} feature should require genesis admin`);
    assert.equal(storage.snapshotBytes(), before, `${value.op} feature must not mutate before genesis admin`);
  }
});

test('MayhemContract keeps providers out of canonical economy and control-plane ops', async () => {
  const { admin, provider, roomId, storage, contract } = await setupSecuritySurface();
  const providerSender = provider.publicKey;
  const adminOnlyAttempts = buildAdminOnlyAttempts(provider, roomId);

  let txNo = 6;
  for (const [type, value] of adminOnlyAttempts) {
    const before = storage.snapshotBytes();
    const result = await execute(contract, storage, type, value, providerSender, txNo);
    assert.match(result.message, /admin required/i, `${type} should be admin-only`);
    assert.equal(storage.snapshotBytes(), before, `${type} must not mutate state`);
    txNo += 1;
  }

  const beforeFeature = storage.snapshotBytes();
  const featureResult = await executeEpochApplyFeature(
    contract,
    storage,
    {
      op: 'epoch_apply',
      epoch: 1,
      at: 3_600,
      debits: [],
      earnings: [{ rail: 'fiat', provider: provider.publicKey, gross_mu: 1_000 }],
    },
    providerSender
  );
  assert.match(featureResult.message, /admin required/i, 'epochApply feature should be admin-only');
  assert.equal(storage.snapshotBytes(), beforeFeature, 'epochApply feature must not mutate state');

  for (const value of buildAdminOnlyFeatureAttempts(provider)) {
    const before = storage.snapshotBytes();
    const result = await executeDepositFeature(contract, storage, value, providerSender);
    assert.match(result.message, /admin required/i, `${value.op} feature should be admin-only`);
    assert.equal(storage.snapshotBytes(), before, `${value.op} feature must not mutate state`);
  }

  for (const value of buildAdminOnlyRateFeatureAttempts()) {
    const before = storage.snapshotBytes();
    const result = await executeRateFeature(contract, storage, value, providerSender);
    assert.match(result.message, /admin required/i, `${value.op} feature should be admin-only`);
    assert.equal(storage.snapshotBytes(), before, `${value.op} feature must not mutate state`);
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

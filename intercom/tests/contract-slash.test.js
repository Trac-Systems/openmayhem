import assert from 'node:assert/strict';
import test from 'node:test';
import MayhemContract from '../contract/contract.js';
import {
  MemoryStorage,
  execute,
  makeIdentity,
  makeTxKey,
  makeVerifier,
  seedCurrentAdminPrice,
  signProbeResult,
  signConsent,
} from './helpers/contract.js';

const rulesHash = '8'.repeat(64);
const enclaveId = '7'.repeat(64);
const modelId = 'mayhem/qwen3.5-4b@q4';
const DAY_SECONDS = 24 * 60 * 60;

async function setupSlashContract() {
  const admin = await makeIdentity();
  const provider = await makeIdentity();
  const auditor = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const contract = new MayhemContract({ peer: { wallet: makeVerifier(provider.wallet) } }, {});

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
        sig: signConsent(auditor.wallet, 1, rulesHash),
      },
      sender: auditor.publicKey,
      txNo: 3,
    },
    {
      type: 'registerProvider',
      value: { op: 'register_provider' },
      sender: provider.publicKey,
      txNo: 4,
    },
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }

  return { admin, provider, auditor, storage, contract };
}

async function setupProviderServing(ctx) {
  const registered = await execute(
    ctx.contract,
    ctx.storage,
    'registerEnclave',
    {
      op: 'register_enclave',
      enclave_id: enclaveId,
      model_id: modelId,
      backend: 'llama.cpp',
      artifact_root: 'a'.repeat(64),
      artifact_root_kind: 'blake3_merkle_v1',
      artifact_source: {
        kind: 'huggingface',
        repo: 'mayhem-test/slash-model',
        revision: '1'.repeat(40),
        path: 'slash-model.gguf',
      },
      manifest_hash: 'b'.repeat(64),
      att_tier: 1,
      binary_hash: 'c'.repeat(64),
      caps: { chat: true, tools: false, ctx: 32768 },
    },
    ctx.admin.publicKey,
    5
  );
  assert.equal(registered.ok, true, registered.message);
  const catalog = await execute(
    ctx.contract,
    ctx.storage,
    'publishCatalog',
    {
      op: 'publish_catalog',
      catalog_id: 'mayhem-models',
      source_kind: 'huggingface',
      catalog_url: 'https://huggingface.co/TracNetwork/mayhem-catalog/resolve/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/models.json',
      signature_url: 'https://huggingface.co/TracNetwork/mayhem-catalog/resolve/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/models.json.sig',
      catalog_hash: '8'.repeat(64),
      signature_hash: '9'.repeat(64),
      key_id: 'mayhem-catalog-tracnetwork-v1',
      public_key: 'a'.repeat(64),
      model_count: 1,
      artifact_count: 1,
      canaries: [{
        set_id: 'canary-dev-v1',
        url: 'https://huggingface.co/TracNetwork/mayhem-catalog/resolve/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/canaries/canary-dev-v1.json',
        hash: 'b'.repeat(64),
      }],
    },
    ctx.admin.publicKey,
    6
  );
  assert.equal(catalog.ok, true, catalog.message);
  await seedCurrentAdminPrice(ctx.storage, {
    enclaveId,
    modelId,
    admin: ctx.admin.publicKey,
    txNo: 6,
  });

  const joinedEnclave = await execute(
    ctx.contract,
    ctx.storage,
    'joinEnclave',
    { op: 'join_enclave', enclave_id: enclaveId },
    ctx.provider.publicKey,
    6
  );
  assert.equal(joinedEnclave.ok, true, joinedEnclave.message);

  const room = await execute(
    ctx.contract,
    ctx.storage,
    'openRoom',
    {
      op: 'open_room',
      enclave_id: enclaveId,
      model_id: modelId,
      nonce: 'slash-room',
      label: 'Slash Room',
      policy: {},
    },
    ctx.admin.publicKey,
    7
  );
  assert.equal(room.ok, true, room.message);

  const joinedRoom = await execute(
    ctx.contract,
    ctx.storage,
    'joinRoom',
    { op: 'join_room', room_id: room.room_id, enclave_id: enclaveId },
    ctx.provider.publicKey,
    8
  );
  assert.equal(joinedRoom.ok, true, joinedRoom.message);
  return room.room_id;
}

function signedCanaryProbe(ctx, overrides = {}) {
  const value = {
    op: 'probe_result',
    probe_id: 'canary-slash',
    probe_kind: 'canary',
    provider: ctx.provider.publicKey,
    enclave_id: enclaveId,
    binary_hash: 'c'.repeat(64),
    epoch: 3,
    at: 10_800,
    canary_set: 'canary-dev-v1',
    verification_method: 'token_fingerprint',
    match_bps: 1_000,
    pass: false,
    session_receipt_hash: 'c'.repeat(64),
    evidence_hash: 'd'.repeat(64),
    ...overrides,
  };
  value.auditor_sig = signProbeResult(ctx.auditor.wallet, value, ctx.auditor.publicKey);
  return value;
}

function seedHeldEarnings(storage, provider, overrides = {}) {
  return storage.put(`earn/fiat/${provider.publicKey}`, {
    provider: provider.publicKey,
    rail: 'fiat',
    denom: 'au_usd',
    total_au: '10000',
    held_au: '6000',
    paid_cum_au: '1000',
    holdbacks: [
      { epoch: 1, au: '2000' },
      { epoch: 2, au: '4000' },
    ],
    updated_epoch: 2,
    updated_at: null,
    ...overrides,
  });
}

test('MayhemContract canary mismatch slashes held earnings, tombstones serving, and bans provider', async () => {
  const ctx = await setupSlashContract();
  const roomId = await setupProviderServing(ctx);
  await seedHeldEarnings(ctx.storage, ctx.provider);

  const registeredAuditor = await execute(
    ctx.contract,
    ctx.storage,
    'auditorRegister',
    { op: 'auditor_register', auditor: ctx.auditor.publicKey },
    ctx.admin.publicKey,
    9
  );
  assert.equal(registeredAuditor.ok, true, registeredAuditor.message);

  const result = await execute(
    ctx.contract,
    ctx.storage,
    'probeResult',
    signedCanaryProbe(ctx),
    ctx.auditor.publicKey,
    10
  );
  assert.equal(result.ok, true, result.message);
  assert.equal(result.provenance_violation, true);

  assert.deepEqual((await ctx.storage.get(`earn/fiat/${ctx.provider.publicKey}`)).value, {
    provider: ctx.provider.publicKey,
    rail: 'fiat',
    denom: 'au_usd',
    total_au: '4000',
    held_au: '0',
    paid_cum_au: '1000',
    holdbacks: [],
    updated_epoch: 2,
    updated_at: makeTxKey(10),
    slashed_cum_au: '6000',
    last_slash_at: makeTxKey(10),
  });
  assert.deepEqual((await ctx.storage.get(`bal/${ctx.auditor.publicKey}/fiat`)).value, {
    user: ctx.auditor.publicKey,
    rail: 'fiat',
    denom: 'au_usd',
    au: '3000',
    updated_epoch: 3,
    updated_at: makeTxKey(10),
  });
  assert.deepEqual((await ctx.storage.get('fee/fiat/cum')).value, {
    rail: 'fiat',
    denom: 'au_usd',
    cum_au: '3000',
    swept_cum_au: '0',
    updated_epoch: 3,
    updated_at: makeTxKey(10),
    last_apply_hash: null,
    last_fee_bps: null,
    settled_cum_au: '3000',
    last_slash_at: makeTxKey(10),
  });

  const providerRecord = (await ctx.storage.get(`prov/${ctx.provider.publicKey}`)).value;
  assert.equal(providerRecord.status, 'banned');
  assert.equal(providerRecord.banned_by, ctx.auditor.publicKey);

  const serving = (await ctx.storage.get(`serve/${ctx.provider.publicKey}/${enclaveId}`)).value;
  assert.equal(serving.status, 'tombstoned');
  assert.deepEqual(serving.rooms, []);

  const roomServing = (await ctx.storage.get(
    `roomserve/${roomId}/${ctx.provider.publicKey}/${enclaveId}`
  )).value;
  assert.equal(roomServing.status, 'tombstoned');

  const slash = (await ctx.storage.get(`ev/slash/${ctx.provider.publicKey}/${makeTxKey(10)}`)).value;
  assert.equal(slash.reason, 'canary_mismatch');
  assert.equal(slash.source, 'probe');
  assert.equal(slash.forfeited_au, '6000');
  assert.equal(slash.beneficiary, ctx.auditor.publicKey);
  assert.equal(slash.beneficiary_au, '3000');
  assert.equal(slash.treasury_au, '3000');
  assert.equal(slash.provider_banned, true);
  assert.equal(slash.tombstone.serve_tombstoned, true);
  assert.deepEqual(slash.tombstone.rooms_tombstoned, [roomId]);
  assert.equal(slash.slash_hash.length, 64);
});

test('MayhemContract context needle canary mismatch is slashable', async () => {
  const ctx = await setupSlashContract();
  await setupProviderServing(ctx);
  await seedHeldEarnings(ctx.storage, ctx.provider);

  const registeredAuditor = await execute(
    ctx.contract,
    ctx.storage,
    'auditorRegister',
    { op: 'auditor_register', auditor: ctx.auditor.publicKey },
    ctx.admin.publicKey,
    9
  );
  assert.equal(registeredAuditor.ok, true, registeredAuditor.message);

  const result = await execute(
    ctx.contract,
    ctx.storage,
    'probeResult',
    signedCanaryProbe(ctx, {
      probe_id: 'context-needle-slash',
      verification_method: 'context_needle',
      match_bps: 0,
      pass: false,
    }),
    ctx.auditor.publicKey,
    10
  );
  assert.equal(result.ok, true, result.message);
  assert.equal(result.provenance_violation, true);

  const probe = (await ctx.storage.get('ev/probe/context-needle-slash')).value;
  assert.equal(probe.verification_method, 'context_needle');
  assert.equal(probe.pass, false);

  const slash = (await ctx.storage.get(`ev/slash/${ctx.provider.publicKey}/${makeTxKey(10)}`)).value;
  assert.equal(slash.reason, 'canary_mismatch');
  assert.equal(slash.provider_banned, true);
  assert.equal(slash.tombstone.serve_tombstoned, true);
});

test('MayhemContract canary mismatch uses admin-governed fraud_slash_bps', async () => {
  const ctx = await setupSlashContract();
  await setupProviderServing(ctx);
  await seedHeldEarnings(ctx.storage, ctx.provider);

  const registeredAuditor = await execute(
    ctx.contract,
    ctx.storage,
    'auditorRegister',
    { op: 'auditor_register', auditor: ctx.auditor.publicKey },
    ctx.admin.publicKey,
    9
  );
  assert.equal(registeredAuditor.ok, true, registeredAuditor.message);

  const scheduled = await execute(
    ctx.contract,
    ctx.storage,
    'setParams',
    {
      op: 'set_params',
      submitted_at: 0,
      effective_at: DAY_SECONDS,
      values: { fraud_slash_bps: 5_000 },
    },
    ctx.admin.publicKey,
    10
  );
  assert.equal(scheduled.ok, true, scheduled.message);

  const result = await execute(
    ctx.contract,
    ctx.storage,
    'probeResult',
    signedCanaryProbe(ctx, { at: DAY_SECONDS, epoch: 24, probe_id: 'canary-admin-slash' }),
    ctx.auditor.publicKey,
    11
  );
  assert.equal(result.ok, true, result.message);

  const slash = (await ctx.storage.get(`ev/slash/${ctx.provider.publicKey}/${makeTxKey(11)}`)).value;
  assert.equal(slash.slash_bps, 5_000);
  assert.equal(slash.forfeited_au, '3000');
  assert.equal(slash.beneficiary_au, '1500');
  assert.equal(slash.treasury_au, '1500');

  const earning = (await ctx.storage.get(`earn/fiat/${ctx.provider.publicKey}`)).value;
  assert.equal(earning.total_au, '7000');
  assert.equal(earning.held_au, '3000');
  assert.deepEqual(earning.holdbacks, [
    { epoch: 1, au: '2000' },
    { epoch: 2, au: '1000' },
  ]);
});

test('MayhemContract dispute_lost reputation event partially slashes held earnings without ban', async () => {
  const ctx = await setupSlashContract();
  await seedHeldEarnings(ctx.storage, ctx.provider, {
    total_au: '20000',
    held_au: '10000',
    paid_cum_au: '2000',
    holdbacks: [
      { epoch: 1, au: '4000' },
      { epoch: 2, au: '6000' },
    ],
  });

  const result = await execute(
    ctx.contract,
    ctx.storage,
    'recordReputationEvent',
    {
      op: 'record_rep_event',
      provider: ctx.provider.publicKey,
      event_id: 'dispute-lost-1',
      kind: 'dispute_lost',
      epoch: 4,
      at: 14_400,
      evidence_hash: 'e'.repeat(64),
      beneficiary: ctx.admin.publicKey,
    },
    ctx.admin.publicKey,
    5
  );
  assert.equal(result.ok, true, result.message);
  assert.equal(result.slash.reason, 'dispute_lost');
  assert.equal(result.slash.forfeited_au, '2000');

  assert.deepEqual((await ctx.storage.get(`earn/fiat/${ctx.provider.publicKey}`)).value, {
    provider: ctx.provider.publicKey,
    rail: 'fiat',
    denom: 'au_usd',
    total_au: '18000',
    held_au: '8000',
    paid_cum_au: '2000',
    holdbacks: [
      { epoch: 1, au: '4000' },
      { epoch: 2, au: '4000' },
    ],
    updated_epoch: 2,
    updated_at: makeTxKey(5),
    slashed_cum_au: '2000',
    last_slash_at: makeTxKey(5),
  });
  assert.equal((await ctx.storage.get(`prov/${ctx.provider.publicKey}`)).value.status, 'active');
  assert.equal((await ctx.storage.get(`bal/${ctx.admin.publicKey}/fiat`)).value.au, '1000');
  assert.equal((await ctx.storage.get('fee/fiat/cum')).value.cum_au, '1000');

  const slash = (await ctx.storage.get(`ev/slash/${ctx.provider.publicKey}/${makeTxKey(5)}`)).value;
  assert.equal(slash.source, 'dispute');
  assert.equal(slash.provider_banned, false);
  assert.equal(slash.slash_bps, 2_000);
});

test('MayhemContract dispute_lost reputation event uses admin-governed dispute_lost_slash_bps', async () => {
  const ctx = await setupSlashContract();
  await seedHeldEarnings(ctx.storage, ctx.provider, {
    total_au: '20000',
    held_au: '10000',
    paid_cum_au: '2000',
    holdbacks: [
      { epoch: 1, au: '4000' },
      { epoch: 2, au: '6000' },
    ],
  });

  const scheduled = await execute(
    ctx.contract,
    ctx.storage,
    'setParams',
    {
      op: 'set_params',
      submitted_at: 0,
      effective_at: DAY_SECONDS,
      values: { dispute_lost_slash_bps: 1_000 },
    },
    ctx.admin.publicKey,
    5
  );
  assert.equal(scheduled.ok, true, scheduled.message);

  const result = await execute(
    ctx.contract,
    ctx.storage,
    'recordReputationEvent',
    {
      op: 'record_rep_event',
      provider: ctx.provider.publicKey,
      event_id: 'dispute-lost-admin-slash',
      kind: 'dispute_lost',
      epoch: 24,
      at: DAY_SECONDS,
      evidence_hash: 'f'.repeat(64),
      beneficiary: ctx.admin.publicKey,
    },
    ctx.admin.publicKey,
    6
  );
  assert.equal(result.ok, true, result.message);
  assert.equal(result.slash.slash_bps, 1_000);
  assert.equal(result.slash.forfeited_au, '1000');
  assert.equal(result.slash.beneficiary_au, '500');
  assert.equal(result.slash.treasury_au, '500');
});

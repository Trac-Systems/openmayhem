import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import b4a from 'b4a';
import MayhemContract, { SESSION_RECEIPT_SCHEMA_VERSION, receiptMessage } from '../contract/contract.js';
import { opaqueHash, recomputeEpoch, stableJson } from '../scripts/recompute-epoch-roots.mjs';
import {
  MemoryStorage,
  execute,
  executeEpochApplyFeature,
  epochApplyFeatureKey,
  makeIdentity,
  makeTxKey,
  makeVerifier,
  seedSpendHoldsForApply,
  signConsent,
} from './helpers/contract.js';

const rulesHash = '6'.repeat(64);
const enclaveId = 'e'.repeat(64);
const modelId = 'meta/llama-3.1-8b-instruct@4bit';
const DAY_SECONDS = 24 * 60 * 60;

const textLockedRateMapFor = (au) => [
  { unit: 'input_token', per_unit_au: String(au), granularity: 350 },
  { unit: 'output_token', per_unit_au: String(au), granularity: 350 },
];

const imageLockedRateMap = [
  { unit: 'image', per_unit_au: '500', granularity: 1 },
  { unit: 'step', per_unit_au: '2', granularity: 1 },
];
const scriptPath = fileURLToPath(new URL('../scripts/recompute-epoch-roots.mjs', import.meta.url));
const epochEnclaveRegistration = {
  op: 'register_enclave',
  enclave_id: enclaveId,
  model_id: modelId,
  model_class: 'text-generation',
  backend: 'llama.cpp',
  artifact_root: '5'.repeat(64),
  artifact_root_kind: 'blake3_merkle_v1',
  artifact_source: {
    kind: 'huggingface',
    repo: 'mayhem-catalog/epoch-fixture-GGUF',
    revision: '7'.repeat(40),
    path: 'epoch-fixture-Q4_K_M.gguf',
  },
  manifest_hash: '3'.repeat(64),
  att_tier: 1,
  quant: 'INT4',
  binary_hash: '4'.repeat(64),
  caps: {
    chat: true,
    embeddings: false,
    tools: false,
    ctx: 32768,
    modality_set: ['text'],
    speciality_levels: {},
  },
};

const seededBalance = (user, au, rail = 'fiat') => ({
  user,
  rail,
  denom: 'au_usd',
  au: String(au),
  updated_epoch: 0,
  updated_at: null,
});

const defaultReceiptBody = (user, provider) => ({
  schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
  session_id: 'session-epoch-1',
  billing_id: 'd'.repeat(64),
  billing_attempt: 0,
  billing_epoch: 1,
  reservation_id: '8'.repeat(64),
  reservation_expires_after_epoch: 25,
  reservation_receipt_grace_epochs: 6,
  payout_revision: '9'.repeat(64),
  billing_prior_usage: {},
  billing_prior_au_owed_cum: '0',
  seq: 1,
  final: true,
  rail: 'fiat',
  user: user.publicKey,
  provider: provider.publicKey,
  enclave_id: enclaveId,
  model_id: modelId,
  price_ver: 1,
  locked_rate_map: textLockedRateMapFor(2_000),
  locked_per_req_au: '0',
  locked_min_session_au: '0',
  served_ctx: 32768,
  ctx_bracket: 'le32k',
  ctx_bracket_table_ver: 1,
  rules_ver: 1,
  usage: { input_token: 100, output_token: 250 },
  au_owed_cum: '2000',
  prompt_hash: 'a'.repeat(64),
  ts: 3_600,
});

const receiptEnvelopeFromEntry = (entry) => {
  if (entry?.receipt?.body) return entry.receipt;
  if (entry?.body) {
    return {
      body: entry.body,
      enclave_sig: entry.enclave_sig ?? 'b'.repeat(128),
      enclave_pubkey: entry.enclave_pubkey ?? '2'.repeat(64),
      user_sig: entry.user_sig ?? 'c'.repeat(128),
    };
  }
  const {
    enclave_sig: enclaveSig = 'b'.repeat(128),
    enclave_pubkey: enclavePubkey = '2'.repeat(64),
    user_sig: userSig = 'c'.repeat(128),
    receipt_ack: _receiptAck,
    voucher: _voucher,
    ...body
  } = entry;
  return {
    body,
    enclave_sig: enclaveSig,
    enclave_pubkey: enclavePubkey,
    user_sig: userSig,
  };
};

const canonicalReceiptHeadFromEntry = async (entry, settlementEpoch) => {
  const receipt = receiptEnvelopeFromEntry(entry);
  const { body } = receipt;
  const receiptHash = await opaqueHash('mayhem-canonical-receipt-v1', receipt);
  return {
    epoch: settlementEpoch,
    billing_epoch: body.billing_epoch,
    billing_id: body.billing_id,
    billing_attempt: body.billing_attempt,
    reservation_id: body.reservation_id,
    payout_revision: body.payout_revision,
    receipt_hash: receiptHash,
    incremental_au: (
      BigInt(body.au_owed_cum) - BigInt(body.billing_prior_au_owed_cum ?? '0')
    ).toString(),
    receipt,
  };
};

const canonicalReceiptSnapshotHash = (snapshot) => crypto
  .createHash('sha256')
  .update(stableJson({
    schema_version: snapshot.schema_version,
    type: snapshot.type,
    settlement_epoch: snapshot.settlement_epoch,
    metadata: snapshot.metadata,
    identities: snapshot.identities,
    heads: snapshot.heads,
  }))
  .digest('hex');

const canonicalReceiptSnapshot = (heads, settlementEpoch) => {
  const count = heads.length;
  const snapshot = {
    schema_version: 1,
    type: 'canonical_epoch_receipt_snapshot',
    settlement_epoch: settlementEpoch,
    metadata: {
      type: 'canonical_receipt_epoch_index',
      epoch: settlementEpoch,
      count,
      page_size: 128,
      page_count: Math.ceil(count / 128),
      revision: count,
      updated_at: 'f'.repeat(64),
    },
    identities: heads.map((head) => ({
      billing_id: head.billing_id,
      billing_attempt: head.billing_attempt,
    })),
    heads,
  };
  snapshot.snapshot_sha256 = canonicalReceiptSnapshotHash(snapshot);
  return snapshot;
};

async function setupEpochContract() {
  const admin = await makeIdentity();
  const provider = await makeIdentity();
  const user = await makeIdentity();
  const submitter = await makeIdentity();
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
    {
      type: 'registerEnclave',
      value: epochEnclaveRegistration,
      sender: admin.publicKey,
      txNo: 4,
    },
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }

  await storage.put(`bal/${user.publicKey}/fiat`, seededBalance(user.publicKey, 1_000_000));
  return { admin, provider, user, submitter, storage, contract };
}

const receiptBundle = async (user, provider, overrides = {}) => {
  const settlementEpoch = overrides.epoch ?? 1;
  const receiptEntries = overrides.receipts ?? [defaultReceiptBody(user, provider)];
  const heads = [];
  for (const entry of receiptEntries) {
    heads.push(await canonicalReceiptHeadFromEntry(entry, settlementEpoch));
  }
  const snapshot = canonicalReceiptSnapshot(heads, settlementEpoch);
  const {
    receipts: _receipts,
    receipt_snapshot: _receiptSnapshot,
    ...restOverrides
  } = overrides;
  return {
    epoch: settlementEpoch,
    prior_burn_cum_au: '0',
    params: {
      fee_bps: 1_500,
      max_apply_batch: 2_000,
      max_market_usage_entries: 5_000,
    },
    deposits: [],
    receipts: heads,
    receipt_snapshot: snapshot,
    payouts: [],
    price_derivations: [],
    prior_fee_cum_au: '0',
    ...restOverrides,
  };
};

test('epoch recompute accepts only current canonical metered usage maps', async () => {
  const { provider, user } = await setupEpochContract();
  const canonicalTextBody = defaultReceiptBody(user, provider);
  const canonicalText = await receiptBundle(user, provider, {
    receipts: [
      {
        ...canonicalTextBody,
        locked_rate_map: textLockedRateMapFor(2_000),
        usage: { input_token: 100, output_token: 250 },
      },
    ],
  });

  const canonicalRoll = await recomputeEpoch(canonicalText);
  assert.equal(canonicalRoll.totals.use_count, 1);
  assert.equal(canonicalRoll.totals.use_au, '2000');

  await assert.rejects(
    async () => recomputeEpoch(await receiptBundle(user, provider, {
      receipts: [
        {
          ...canonicalTextBody,
          usage: { in: 100, out: 250 },
        },
      ],
    })),
    /receipt usage must be canonical/
  );

  const imageAliasBundle = await receiptBundle(user, provider, {
    receipts: [
      {
        ...canonicalTextBody,
        model_id: 'admin/image-small@fp16',
        locked_rate_map: imageLockedRateMap,
        usage: { images: 2, steps: 60 },
        au_owed_cum: '1120',
      },
    ],
  });
  const imageCanonicalBundle = await receiptBundle(user, provider, {
    receipts: [
      {
        ...canonicalTextBody,
        model_id: 'admin/image-small@fp16',
        locked_rate_map: imageLockedRateMap,
        usage: { image: 2, step: 60 },
        au_owed_cum: '1120',
      },
    ],
  });
  const imageCanonicalRoll = await recomputeEpoch(imageCanonicalBundle);

  await assert.rejects(() => recomputeEpoch(imageAliasBundle), /receipt usage must be canonical/);
  assert.equal(imageCanonicalRoll.totals.use_count, 1);
  assert.equal(imageCanonicalRoll.totals.use_au, '1120');
});

test('epoch recompute nets one logical bill across provider redispatch attempts', async () => {
  const { provider: providerA, user } = await setupEpochContract();
  const providerB = await makeIdentity();
  const base = defaultReceiptBody(user, providerA);
  const billingId = '7'.repeat(64);
  const lockedRateMap = [
    { unit: 'input_token', per_unit_au: '100', granularity: 1 },
    { unit: 'output_token', per_unit_au: '50', granularity: 1 },
  ];
  const first = {
    ...base,
    session_id: 'logical-attempt-a',
    billing_id: billingId,
    billing_attempt: 0,
    billing_prior_usage: {},
    billing_prior_au_owed_cum: '0',
    seq: 1,
    final: false,
    provider: providerA.publicKey,
    locked_rate_map: lockedRateMap,
    usage: { input_token: 1 },
    au_owed_cum: '100',
  };
  const second = {
    ...base,
    session_id: 'logical-attempt-b',
    billing_id: billingId,
    billing_attempt: 1,
    billing_prior_usage: first.usage,
    billing_prior_au_owed_cum: first.au_owed_cum,
    seq: 1,
    final: true,
    provider: providerB.publicKey,
    locked_rate_map: lockedRateMap,
    usage: { input_token: 1, output_token: 2 },
    au_owed_cum: '200',
  };

  const roll = await recomputeEpoch(await receiptBundle(user, providerA, {
    receipts: [second, first],
  }));
  assert.equal(roll.totals.use_count, 2);
  assert.equal(roll.totals.use_au, '200');
  assert.equal(roll.totals.provider_count, 2);
  assert.deepEqual(
    Object.fromEntries(roll.earnings.map((entry) => [entry.provider, entry.gross_au])),
    {
      [providerA.publicKey]: '100',
      [providerB.publicKey]: '100',
    }
  );

  await assert.rejects(
    async () => recomputeEpoch(await receiptBundle(user, providerA, {
      receipts: [first, { ...second, billing_prior_au_owed_cum: '99' }],
    })),
    /redispatch baseline does not match prior logical settlement/
  );
  await assert.rejects(
    async () => recomputeEpoch(await receiptBundle(user, providerA, {
      receipts: [first, { ...second, billing_prior_usage: {} }],
    })),
    /redispatch baseline does not match prior logical settlement/
  );
  await assert.rejects(
    async () => recomputeEpoch(await receiptBundle(user, providerB, {
      receipts: [second],
    })),
    /logical billing baseline has no prior signed receipt/
  );
});

test('epoch recompute applies TAP 75/15/10 without burning fiat or TNK', async () => {
  const { provider, user } = await setupEpochContract();
  const base = defaultReceiptBody(user, provider);
  const roll = await recomputeEpoch(await receiptBundle(user, provider, {
    prior_burn_cum_au: '200',
    receipts: ['fiat', 'tap', 'tnk'].map((rail, index) => ({
      ...base,
      session_id: `session-${rail}`,
      billing_id: String(index + 1).repeat(64),
      seq: index + 1,
      rail,
      au_owed_cum: '10000',
    })),
  }));

  assert.deepEqual(roll.params, {
    epoch_seconds: 3_600,
    fee_bps: 1_500,
    tap_burn_bps: 1_000,
  });
  assert.equal(roll.totals.use_au, '30000');
  assert.equal(roll.totals.earn_au, '24500');
  assert.equal(roll.totals.fee_au, '4500');
  assert.equal(roll.totals.fee_cum_au, '4500');
  assert.equal(roll.totals.burn_au, '1000');
  assert.equal(roll.totals.burn_cum_au, '1200');
});

test('epoch recompute binds enclave public key into the canonical receipt head hash', async () => {
  const { provider, user } = await setupEpochContract();
  const withoutKey = await receiptBundle(user, provider);
  const withKey = await receiptBundle(user, provider, {
    receipts: withoutKey.receipts.map((head) => ({
      ...head.receipt,
      enclave_pubkey: 'd'.repeat(64),
    })),
  });

  const withoutKeyRoll = await recomputeEpoch(withoutKey);
  const withKeyRoll = await recomputeEpoch(withKey);
  assert.equal(withKeyRoll.totals.use_au, withoutKeyRoll.totals.use_au);
  assert.equal(withKeyRoll.totals.earn_au, withoutKeyRoll.totals.earn_au);
  assert.notEqual(withKeyRoll.allocations[0].receipt_hash, withoutKeyRoll.allocations[0].receipt_hash);
  assert.notEqual(withKeyRoll.apply_pages[0].page_sha256, withoutKeyRoll.apply_pages[0].page_sha256);
});

const signedReceipt = (user, provider, enclave, overrides = {}) => {
  const body = {
    schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
    session_id: 'session-epoch-1',
    billing_id: 'd'.repeat(64),
    billing_attempt: 0,
    billing_epoch: 1,
    reservation_id: '8'.repeat(64),
    reservation_expires_after_epoch: 25,
    reservation_receipt_grace_epochs: 6,
    payout_revision: '9'.repeat(64),
    billing_prior_usage: {},
    billing_prior_au_owed_cum: '0',
    seq: 1,
    final: true,
    rail: 'fiat',
    user: user.publicKey,
    provider: provider.publicKey,
    enclave_id: enclaveId,
    model_id: modelId,
    price_ver: 1,
    locked_rate_map: textLockedRateMapFor(1_000),
    locked_per_req_au: '0',
    locked_min_session_au: '0',
    served_ctx: 32768,
    ctx_bracket: 'le32k',
    ctx_bracket_table_ver: 1,
    rules_ver: 1,
    usage: { input_token: 100, output_token: 250 },
    au_owed_cum: '1000',
    prompt_hash: 'a'.repeat(64),
    ts: 3_600,
    ...overrides,
  };
  const message = b4a.from(receiptMessage(body));
  return {
    body,
    enclave_pubkey: enclave.publicKey,
    enclave_sig: b4a.toString(enclave.wallet.sign(message), 'hex'),
    user_sig: b4a.toString(user.wallet.sign(message), 'hex'),
  };
};

test('MayhemContract anchors epoch roots permissionlessly and applies matching evidence roots', async () => {
  const { admin, provider, user, submitter, storage, contract } = await setupEpochContract();
  const bundle = await receiptBundle(user, provider);
  const roll = await recomputeEpoch(bundle);

  const commit = await execute(
    contract,
    storage,
    'epochCommit',
    {
      op: 'epoch_commit',
      epoch: 1,
      at: 3_600,
      roots: roll.roots,
      totals: roll.totals,
    },
    submitter.publicKey,
    4
  );
  assert.equal(commit.ok, true, commit.message);
  assert.equal(commit.op, 'epochCommit');
  assert.equal(commit.idempotent, false);
  assert.equal(commit.commit_hash.length, 64);

  const commitRecord = await storage.get('epoch/commit/1');
  assert.equal(commitRecord.value.submitted_by, submitter.publicKey);
  assert.equal(commitRecord.value.status, 'provisional');
  assert.equal(commitRecord.value.provisional_until_epoch, 7);
  assert.deepEqual(commitRecord.value.roots, roll.roots);
  assert.deepEqual(commitRecord.value.totals, roll.totals);

  const duplicate = await execute(
    contract,
    storage,
    'epochCommit',
    {
      op: 'epoch_commit',
      epoch: 1,
      at: 3_600,
      roots: roll.roots,
      totals: roll.totals,
    },
    admin.publicKey,
    5
  );
  assert.deepEqual(duplicate, {
    ok: true,
    op: 'epochCommit',
    epoch: 1,
    idempotent: true,
    commit_hash: commit.commit_hash,
  });

  const changed = await execute(
    contract,
    storage,
    'epochCommit',
    {
      op: 'epoch_commit',
      epoch: 1,
      at: 3_600,
      roots: { ...roll.roots, fee: 'f'.repeat(64) },
      totals: roll.totals,
    },
    submitter.publicKey,
    6
  );
  assert.match(changed.message, /already exists/i);

  const applyValue = {
    op: 'epoch_apply',
    epoch: 1,
    at: 3_600,
    debits: roll.debits,
    earnings: roll.earnings,
    roots: roll.roots,
    totals: roll.totals,
  };
  const applyKey = await epochApplyFeatureKey(contract, applyValue);
  await seedSpendHoldsForApply(storage, applyValue);
  const applied = await executeEpochApplyFeature(
    contract,
    storage,
    applyValue,
    admin.publicKey
  );
  assert.deepEqual(applied, {
    ok: true,
    op: 'epochApply',
    epoch: 1,
    idempotent: false,
    debited_au: '2000',
    earned_au: '1700',
    fee_au: '300',
    burn_au: '0',
    rails: ['fiat'],
  });

  assert.deepEqual((await storage.get(`bal/${user.publicKey}/fiat`)).value, {
    user: user.publicKey,
    rail: 'fiat',
    denom: 'au_usd',
    au: '998000',
    updated_epoch: 1,
    updated_at: applyKey,
  });
  assert.deepEqual((await storage.get(`earn/fiat/${provider.publicKey}`)).value, {
    provider: provider.publicKey,
    rail: 'fiat',
    denom: 'au_usd',
    total_au: '1700',
    held_au: '1700',
    paid_cum_au: '0',
    holdbacks: [{ epoch: 1, au: '1700', locked_epochs: 168 }],
    updated_epoch: 1,
    updated_at: applyKey,
    last_holdback_release_epoch: 1,
  });
  assert.deepEqual((await storage.get('ev/dep/1')).value, {
    type: 'deposit_root',
    epoch: 1,
    epoch_seconds: 3_600,
    merkle_root: roll.roots.dep,
    count: 0,
    au_total: '0',
    ts: 3_600,
    updated_at: applyKey,
  });
  assert.deepEqual((await storage.get('ev/use/1')).value, {
    type: 'usage_root',
    epoch: 1,
    epoch_seconds: 3_600,
    merkle_root: roll.roots.use,
    sessions: 1,
    au_total: '2000',
    providers: 1,
    ts: 3_600,
    updated_at: applyKey,
  });
  assert.deepEqual((await storage.get('ev/earn/1')).value, {
    type: 'earn_root',
    epoch: 1,
    epoch_seconds: 3_600,
    merkle_root: roll.roots.earn,
    provider_count: 1,
    au_cum_total: '1700',
    ts: 3_600,
    updated_at: applyKey,
  });
  assert.deepEqual((await storage.get('ev/fee/1')).value, {
    type: 'fee_root',
    epoch: 1,
    epoch_seconds: 3_600,
    merkle_root: roll.roots.fee,
    au_fee_epoch: '300',
    au_fee_cum: '300',
    au_burn_epoch: '0',
    au_burn_cum: '0',
    tap_burn_bps: 1_000,
    sweep_msb_tx_hash: null,
    ts: 3_600,
    updated_at: applyKey,
  });
  assert.equal(await storage.get('ev/pay/1'), null);
});

test('MayhemContract admin can seal one elapsed empty epoch and unblock later settlement', async () => {
  const { admin, provider, user, submitter, storage, contract } = await setupEpochContract();
  const reasonHash = 'a'.repeat(64);

  const nonAdmin = await execute(
    contract,
    storage,
    'epochSealEmpty',
    { op: 'epoch_seal_empty', epoch: 1, at: 3_600, reason_hash: reasonHash },
    submitter.publicKey,
    5
  );
  assert.match(nonAdmin.message, /admin required/i);

  const early = await execute(
    contract,
    storage,
    'epochSealEmpty',
    { op: 'epoch_seal_empty', epoch: 1, at: 3_599, reason_hash: reasonHash },
    admin.publicKey,
    6
  );
  assert.match(early.message, /not active/i);

  const jump = await execute(
    contract,
    storage,
    'epochSealEmpty',
    { op: 'epoch_seal_empty', epoch: 2, at: 7_200, reason_hash: reasonHash },
    admin.publicKey,
    7
  );
  assert.match(jump.message, /contiguous/i);

  const sealed = await execute(
    contract,
    storage,
    'epochSealEmpty',
    { op: 'epoch_seal_empty', epoch: 1, at: 3_600, reason_hash: reasonHash },
    admin.publicKey,
    8
  );
  assert.equal(sealed.ok, true, sealed.message);
  assert.equal(sealed.op, 'epochSealEmpty');
  assert.equal(sealed.idempotent, false);
  assert.equal(sealed.seal_hash.length, 64);
  assert.deepEqual((await storage.get('epoch/seal/1')).value, {
    type: 'epoch_empty_seal',
    epoch: 1,
    at: 3_600,
    epoch_seconds: 3_600,
    previous_apply_hash: null,
    reason_hash: reasonHash,
    sealed_by: admin.publicKey,
    sealed_by_role: 'admin',
    totals: {
      debited_au: '0',
      earned_au: '0',
      fee_au: '0',
      burn_au: '0',
    },
    seal_hash: sealed.seal_hash,
    sealed_at: makeTxKey(8),
  });
  assert.deepEqual((await storage.get('epoch/apply/state')).value, {
    updated_epoch: 1,
    updated_at: makeTxKey(8),
    last_apply_hash: sealed.seal_hash,
    last_apply_previous_hash: null,
    last_epoch_seconds: 3_600,
    last_settlement_unix: 3_600,
    pending_epoch: null,
    pending_next_page: 0,
    pending_settlement_unix: null,
    pending_reserved_debits: null,
    last_page: 0,
  });
  assert.equal((await storage.get(`bal/${user.publicKey}/fiat`)).value.au, '1000000');
  assert.equal(await storage.get(`earn/fiat/${provider.publicKey}`), null);
  assert.equal(await storage.get('fee/fiat/cum'), null);

  const replay = await execute(
    contract,
    storage,
    'epochSealEmpty',
    { op: 'epoch_seal_empty', epoch: 1, at: 3_600, reason_hash: reasonHash },
    admin.publicKey,
    9
  );
  assert.deepEqual(replay, {
    ok: true,
    op: 'epochSealEmpty',
    epoch: 1,
    idempotent: true,
    seal_hash: sealed.seal_hash,
  });

  const laterSettlementValue = {
    op: 'epoch_apply',
    epoch: 2,
    at: 7_200,
    debits: [{ rail: 'fiat', user: user.publicKey, au: '1000' }],
    earnings: [{ rail: 'fiat', provider: provider.publicKey, gross_au: '1000' }],
  };
  await seedSpendHoldsForApply(storage, laterSettlementValue);
  const laterSettlement = await executeEpochApplyFeature(
    contract,
    storage,
    laterSettlementValue,
    admin.publicKey
  );
  assert.equal(laterSettlement.ok, true, laterSettlement.message);
  assert.equal(laterSettlement.epoch, 2);
  assert.equal((await storage.get('epoch/apply/state')).value.updated_epoch, 2);
  assert.equal((await storage.get(`bal/${user.publicKey}/fiat`)).value.au, '999000');
  assert.equal((await storage.get(`earn/fiat/${provider.publicKey}`)).value.total_au, '850');
  assert.equal((await storage.get('fee/fiat/cum')).value.cum_au, '150');
});

test('MayhemContract applies consecutive fee-bearing epochs (settled_cum_au advances with cum_au)', async () => {
  const { admin, provider, user, storage, contract } = await setupEpochContract();
  const first = {
    op: 'epoch_apply',
    epoch: 1,
    at: 3_600,
    debits: [{ rail: 'fiat', user: user.publicKey, au: '2000' }],
    earnings: [{ rail: 'fiat', provider: provider.publicKey, gross_au: '2000' }],
  };
  await seedSpendHoldsForApply(storage, first);
  const firstApplied = await executeEpochApplyFeature(contract, storage, first, admin.publicKey);
  assert.equal(firstApplied.ok, true, firstApplied.message);
  const feeAfterFirst = (await storage.get('fee/fiat/cum')).value;
  assert.equal(feeAfterFirst.cum_au, '300');
  assert.equal(feeAfterFirst.settled_cum_au, '2000');

  // Regression: the second fee-bearing epoch used to be rejected with
  // "Guardian conservation invariant failed" because epochApply built nextFee
  // with the new cum_au but the stale settled_cum_au.
  const second = {
    op: 'epoch_apply',
    epoch: 2,
    at: 7_200,
    debits: [{ rail: 'fiat', user: user.publicKey, au: '20000' }],
    earnings: [{ rail: 'fiat', provider: provider.publicKey, gross_au: '20000' }],
  };
  await seedSpendHoldsForApply(storage, second);
  const secondApplied = await executeEpochApplyFeature(contract, storage, second, admin.publicKey);
  assert.equal(secondApplied.ok, true, secondApplied.message);
  const feeAfterSecond = (await storage.get('fee/fiat/cum')).value;
  assert.equal(feeAfterSecond.cum_au, '3300');
  assert.equal(feeAfterSecond.settled_cum_au, '22000');
});

test('MayhemContract binds active admin epoch timing into commit and apply evidence', async () => {
  const { admin, provider, user, submitter, storage, contract } = await setupEpochContract();
  const tuned = await execute(
    contract,
    storage,
    'setParams',
    {
      op: 'set_params',
      submitted_at: 0,
      effective_at: 86_400,
      values: {
        epoch_seconds: 7_200,
        challenge_epochs: 2,
      },
    },
    admin.publicKey,
    4
  );
  assert.equal(tuned.ok, true, tuned.message);

  const roll = await recomputeEpoch(await receiptBundle(user, provider));
  const commit = await execute(
    contract,
    storage,
    'epochCommit',
    {
      op: 'epoch_commit',
      epoch: 1,
      at: 86_400,
      roots: roll.roots,
      totals: roll.totals,
    },
    submitter.publicKey,
    5
  );
  assert.equal(commit.ok, true, commit.message);
  assert.deepEqual((await storage.get('epoch/commit/1')).value, {
    type: 'epoch_commit',
    epoch: 1,
    epoch_seconds: 7_200,
    roots: roll.roots,
    totals: roll.totals,
    status: 'provisional',
    challenge_epochs: 2,
    provisional_until_epoch: 3,
    commit_hash: commit.commit_hash,
    submitted_by: submitter.publicKey,
    submitted_at: makeTxKey(5),
    at: 86_400,
  });

  const staleTimingValue = {
    op: 'epoch_apply',
    epoch: 1,
    at: 86_399,
    debits: roll.debits,
    earnings: roll.earnings,
    roots: roll.roots,
    totals: roll.totals,
  };
  await seedSpendHoldsForApply(storage, staleTimingValue);
  const staleTimingApply = await executeEpochApplyFeature(
    contract,
    storage,
    staleTimingValue,
    admin.publicKey
  );
  assert.match(staleTimingApply.message, /epoch_seconds does not match/i);
  assert.equal((await storage.get('ev/use/1')), null);

  const applyValue = {
    op: 'epoch_apply',
    epoch: 1,
    at: 86_400,
    debits: roll.debits,
    earnings: roll.earnings,
    roots: roll.roots,
    totals: roll.totals,
  };
  const applyKey = await epochApplyFeatureKey(contract, applyValue);
  await seedSpendHoldsForApply(storage, applyValue);
  const applied = await executeEpochApplyFeature(contract, storage, applyValue, admin.publicKey);
  assert.equal(applied.ok, true, applied.message);
  assert.equal((await storage.get('epoch/apply/state')).value.last_epoch_seconds, 7_200);
  assert.equal((await storage.get('ev/dep/1')).value.epoch_seconds, 7_200);
  assert.equal((await storage.get('ev/use/1')).value.epoch_seconds, 7_200);
  assert.equal((await storage.get('ev/earn/1')).value.epoch_seconds, 7_200);
  assert.equal((await storage.get('ev/fee/1')).value.epoch_seconds, 7_200);
  assert.equal((await storage.get('ev/use/1')).value.updated_at, applyKey);
});

test('MayhemContract fraudProof voids an inflated single-receipt commit and bans submitter', async () => {
  const { admin, provider, user, submitter, storage, contract } = await setupEpochContract();
  const enclave = await makeIdentity();
  const prover = await makeIdentity();
  const otherSubmitter = await makeIdentity();
  const receipt = signedReceipt(user, provider, enclave, { au_owed_cum: '1000' });
  const inflatedReceipt = {
    ...receipt,
    body: {
      ...receipt.body,
      au_owed_cum: '2000',
    },
  };
  const inflatedRoll = await recomputeEpoch(await receiptBundle(user, provider, {
    receipts: [inflatedReceipt],
  }));

  const commit = await execute(
    contract,
    storage,
    'epochCommit',
    {
      op: 'epoch_commit',
      epoch: 1,
      at: 3_600,
      roots: inflatedRoll.roots,
      totals: inflatedRoll.totals,
    },
    submitter.publicKey,
    4
  );
  assert.equal(commit.ok, true, commit.message);
  assert.equal(inflatedRoll.totals.use_au, '2000');

  const proof = await execute(
    contract,
    storage,
    'fraudProof',
    {
      op: 'fraud_proof',
      epoch: 1,
      proof_epoch: 2,
      at: 7_200,
      reason: 'over_credit',
      receipt,
      claimed_au_owed_cum: '2000',
    },
    prover.publicKey,
    5
  );
  assert.equal(proof.ok, true, proof.message);
  assert.equal(proof.op, 'fraudProof');
  assert.equal(proof.idempotent, false);
  assert.equal(proof.voided_commit, commit.commit_hash);
  assert.equal(proof.banned_submitter, submitter.publicKey);

  const commitRecord = (await storage.get('epoch/commit/1')).value;
  assert.equal(commitRecord.status, 'void');
  assert.equal(commitRecord.voided_by, prover.publicKey);
  assert.equal(commitRecord.fraud_reason, 'over_credit');
  assert.equal(commitRecord.fraud_proof_hash, proof.proof_hash);

  const fraudRecord = (await storage.get(`ev/fraud/1/${proof.proof_hash}`)).value;
  assert.equal(fraudRecord.actual_au, '1000');
  assert.equal(fraudRecord.claimed_au, '2000');
  assert.equal(fraudRecord.voided_commit, commit.commit_hash);

  assert.deepEqual((await storage.get(`committer/ban/${submitter.publicKey}`)).value, {
    submitter: submitter.publicKey,
    status: 'banned',
    reason: 'fraud_proof',
    epoch: 1,
    proof_hash: proof.proof_hash,
    banned_at: makeTxKey(5),
    banned_by: prover.publicKey,
  });

  const duplicateProof = await execute(
    contract,
    storage,
    'fraudProof',
    {
      op: 'fraud_proof',
      epoch: 1,
      proof_epoch: 2,
      at: 7_200,
      reason: 'over_credit',
      receipt,
      claimed_au_owed_cum: '2000',
    },
    otherSubmitter.publicKey,
    6
  );
  assert.deepEqual(duplicateProof, {
    ok: true,
    op: 'fraudProof',
    epoch: 1,
    idempotent: true,
    proof_hash: proof.proof_hash,
    voided_commit: commit.commit_hash,
    banned_submitter: submitter.publicKey,
  });

  const voidApplyValue = {
    op: 'epoch_apply',
    epoch: 1,
    at: 3_600,
    debits: inflatedRoll.debits,
    earnings: inflatedRoll.earnings,
    roots: inflatedRoll.roots,
    totals: inflatedRoll.totals,
  };
  await seedSpendHoldsForApply(storage, voidApplyValue);
  const voidApply = await executeEpochApplyFeature(
    contract,
    storage,
    voidApplyValue,
    admin.publicKey
  );
  assert.match(voidApply.message, /commit is void|matching provisional epoch commit required/i);
  assert.equal((await storage.get('ev/use/1')), null);
  assert.equal((await storage.get(`bal/${user.publicKey}/fiat`)).value.au, '1000000');

  const nextEpoch = await recomputeEpoch(await receiptBundle(user, provider, {
    epoch: 2,
    receipts: [{
      ...defaultReceiptBody(user, provider),
      billing_epoch: 2,
      session_id: 'ban-check-epoch-2',
      billing_id: '2'.repeat(64),
    }],
  }));
  const bannedCommit = await execute(
    contract,
    storage,
    'epochCommit',
    {
      op: 'epoch_commit',
      epoch: 2,
      at: 7_200,
      roots: nextEpoch.roots,
      totals: nextEpoch.totals,
    },
    submitter.publicKey,
    8
  );
  assert.match(bannedCommit.message, /committer is banned/i);

  const allowedCommit = await execute(
    contract,
    storage,
    'epochCommit',
    {
      op: 'epoch_commit',
      epoch: 2,
      at: 7_200,
      roots: nextEpoch.roots,
      totals: nextEpoch.totals,
    },
    otherSubmitter.publicKey,
    9
  );
  assert.equal(allowedCommit.ok, true, allowedCommit.message);

  const unbanned = await execute(
    contract,
    storage,
    'unban',
    {
      op: 'unban',
      target_type: 'committer',
      target: submitter.publicKey,
      reason_hash: 'a'.repeat(64),
    },
    admin.publicKey,
    10
  );
  assert.equal(unbanned.ok, true, unbanned.message);
  assert.equal((await storage.get(`committer/ban/${submitter.publicKey}`)).value.status, 'unbanned');

  const nextEpoch3 = await recomputeEpoch(await receiptBundle(user, provider, {
    epoch: 3,
    receipts: [{
      ...defaultReceiptBody(user, provider),
      billing_epoch: 3,
      session_id: 'ban-check-epoch-3',
      billing_id: '3'.repeat(64),
    }],
  }));
  const restoredCommit = await execute(
    contract,
    storage,
    'epochCommit',
    {
      op: 'epoch_commit',
      epoch: 3,
      at: 10_800,
      roots: nextEpoch3.roots,
      totals: nextEpoch3.totals,
    },
    submitter.publicKey,
    11
  );
  assert.equal(restoredCommit.ok, true, restoredCommit.message);
});

test('MayhemContract fraudProof voids an inflated workflow receipt commit', async () => {
  const { admin, provider, user, submitter, storage, contract } = await setupEpochContract();
  const enclave = await makeIdentity();
  const prover = await makeIdentity();
  const workflow = {
    endpoint_family: 'comfy_workflow',
    graph_hash: 'a1'.repeat(32),
    runtime_id: 'comfyui-portable-0.3.43',
    outcome_class: 'image',
    quoted_usage: { image: 1, step: 250 },
  };
  const receipt = signedReceipt(user, provider, enclave, {
    session_id: 'workflow-fraud-session',
    billing_id: 'a2'.repeat(32),
    locked_rate_map: imageLockedRateMap,
    usage: { image: 1, step: 250 },
    au_owed_cum: '1000',
    workflow,
    workflow_output: {
      output_modalities: ['image'],
      metrics: {
        bytes: 512_000,
        height: 512,
        image: 1,
        width: 512,
      },
    },
  });
  const inflatedReceipt = {
    ...receipt,
    body: {
      ...receipt.body,
      au_owed_cum: '2000',
    },
  };
  const inflatedRoll = await recomputeEpoch(await receiptBundle(user, provider, {
    receipts: [inflatedReceipt],
  }));

  const commit = await execute(
    contract,
    storage,
    'epochCommit',
    {
      op: 'epoch_commit',
      epoch: 1,
      at: 3_600,
      roots: inflatedRoll.roots,
      totals: inflatedRoll.totals,
    },
    submitter.publicKey,
    4
  );
  assert.equal(commit.ok, true, commit.message);
  assert.equal(inflatedRoll.totals.use_au, '2000');

  const proof = await execute(
    contract,
    storage,
    'fraudProof',
    {
      op: 'fraud_proof',
      epoch: 1,
      proof_epoch: 2,
      at: 7_200,
      reason: 'over_credit',
      receipt,
      claimed_au_owed_cum: '2000',
    },
    prover.publicKey,
    5
  );
  assert.equal(proof.ok, true, proof.message);
  assert.equal(proof.voided_commit, commit.commit_hash);
  assert.equal((await storage.get('epoch/commit/1')).value.status, 'void');
  const fraudRecord = (await storage.get(`ev/fraud/1/${proof.proof_hash}`)).value;
  assert.equal(fraudRecord.actual_au, '1000');
  assert.equal(fraudRecord.claimed_au, '2000');
  assert.equal(fraudRecord.receipt_hash.length, 64);

  const voidApplyValue = {
    op: 'epoch_apply',
    epoch: 1,
    at: 3_600,
    debits: inflatedRoll.debits,
    earnings: inflatedRoll.earnings,
    roots: inflatedRoll.roots,
    totals: inflatedRoll.totals,
  };
  await seedSpendHoldsForApply(storage, voidApplyValue);
  const voidApply = await executeEpochApplyFeature(
    contract,
    storage,
    voidApplyValue,
    admin.publicKey
  );
  assert.match(voidApply.message, /commit is void|matching provisional epoch commit required/i);
});

test('MayhemContract fraudProof slashes a registered provider committer', async () => {
  const { admin, provider, user, storage, contract } = await setupEpochContract();
  const enclave = await makeIdentity();
  const prover = await makeIdentity();
  const receipt = signedReceipt(user, provider, enclave, { au_owed_cum: '1000' });
  const inflatedReceipt = {
    ...receipt,
    body: {
      ...receipt.body,
      au_owed_cum: '2000',
    },
  };
  const inflatedRoll = await recomputeEpoch(await receiptBundle(user, provider, {
    receipts: [inflatedReceipt],
  }));
  await storage.put(`earn/fiat/${provider.publicKey}`, {
    provider: provider.publicKey,
    rail: 'fiat',
    denom: 'au_usd',
    total_au: '8500',
    held_au: '8500',
    paid_cum_au: '0',
    holdbacks: [{ epoch: 1, au: '8500' }],
    updated_epoch: 1,
    updated_at: null,
  });
  await storage.put(`serve/${provider.publicKey}/${enclaveId}`, {
    provider: provider.publicKey,
    enclave_id: enclaveId,
    model_id: modelId,
    status: 'active',
    joined_at: makeTxKey(4),
    updated_at: makeTxKey(4),
    left_at: null,
    rooms: [],
  });

  const commit = await execute(
    contract,
    storage,
    'epochCommit',
    {
      op: 'epoch_commit',
      epoch: 1,
      at: 3_600,
      roots: inflatedRoll.roots,
      totals: inflatedRoll.totals,
    },
    provider.publicKey,
    4
  );
  assert.equal(commit.ok, true, commit.message);

  const scheduled = await execute(
    contract,
    storage,
    'setParams',
    {
      op: 'set_params',
      submitted_at: 0,
      effective_at: DAY_SECONDS,
      values: { fraud_slash_bps: 5_000 },
    },
    admin.publicKey,
    5
  );
  assert.equal(scheduled.ok, true, scheduled.message);

  const proof = await execute(
    contract,
    storage,
    'fraudProof',
    {
      op: 'fraud_proof',
      epoch: 1,
      proof_epoch: 2,
      at: DAY_SECONDS,
      reason: 'over_credit',
      receipt,
      claimed_au_owed_cum: '2000',
    },
    prover.publicKey,
    6
  );
  assert.equal(proof.ok, true, proof.message);
  assert.equal(proof.slash.reason, 'receipt_forgery');
  assert.equal(proof.slash.slash_bps, 5_000);
  assert.equal(proof.slash.forfeited_au, '4250');
  assert.equal(proof.slash.beneficiary_au, '2125');
  assert.equal(proof.slash.treasury_au, '2125');

  assert.deepEqual((await storage.get(`earn/fiat/${provider.publicKey}`)).value, {
    provider: provider.publicKey,
    rail: 'fiat',
    denom: 'au_usd',
    total_au: '4250',
    held_au: '4250',
    paid_cum_au: '0',
    holdbacks: [{ epoch: 1, au: '4250' }],
    updated_epoch: 1,
    updated_at: makeTxKey(6),
    slashed_cum_au: '4250',
    last_slash_at: makeTxKey(6),
  });
  assert.equal((await storage.get(`bal/${prover.publicKey}/fiat`)).value.au, '2125');
  assert.equal((await storage.get('fee/fiat/cum')).value.cum_au, '2125');
  assert.equal((await storage.get(`prov/${provider.publicKey}`)).value.status, 'banned');
  assert.equal((await storage.get(`serve/${provider.publicKey}/${enclaveId}`)).value.status, 'tombstoned');

  const fraudRecord = (await storage.get(`ev/fraud/1/${proof.proof_hash}`)).value;
  assert.equal(fraudRecord.slash.reason, 'receipt_forgery');
  const slash = (await storage.get(`ev/slash/${provider.publicKey}/${makeTxKey(6)}`)).value;
  assert.equal(slash.source, 'fraud_proof');
  assert.equal(slash.provider_banned, true);
});

test('MayhemContract fraudProof rejects proofs after the challenge window', async () => {
  const { provider, user, submitter, storage, contract } = await setupEpochContract();
  const enclave = await makeIdentity();
  const prover = await makeIdentity();
  const receipt = signedReceipt(user, provider, enclave, { au_owed_cum: '1000' });
  const inflatedRoll = await recomputeEpoch(await receiptBundle(user, provider, {
    receipts: [{
      ...receipt,
      body: {
        ...receipt.body,
        au_owed_cum: '2000',
      },
    }],
  }));

  const commit = await execute(
    contract,
    storage,
    'epochCommit',
    {
      op: 'epoch_commit',
      epoch: 1,
      at: 3_600,
      roots: inflatedRoll.roots,
      totals: inflatedRoll.totals,
    },
    submitter.publicKey,
    4
  );
  assert.equal(commit.ok, true, commit.message);

  const expired = await execute(
    contract,
    storage,
    'fraudProof',
    {
      op: 'fraud_proof',
      epoch: 1,
      proof_epoch: 8,
      at: 28_800,
      reason: 'over_credit',
      receipt,
      claimed_au_owed_cum: '2000',
    },
    prover.publicKey,
    5
  );
  assert.match(expired.message, /challenge window has closed/i);
  assert.equal((await storage.get('epoch/commit/1')).value.status, 'provisional');
  assert.equal(await storage.get(`committer/ban/${submitter.publicKey}`), null);
});

test('MayhemContract refuses evidence-root apply without a matching epoch commit', async () => {
  const { admin, provider, user, storage, contract } = await setupEpochContract();
  const roll = await recomputeEpoch(await receiptBundle(user, provider));

  const noCommitValue = {
    op: 'epoch_apply',
    epoch: 1,
    at: 3_600,
    debits: roll.debits,
    earnings: roll.earnings,
    roots: roll.roots,
    totals: roll.totals,
  };
  await seedSpendHoldsForApply(storage, noCommitValue);
  const noCommit = await executeEpochApplyFeature(
    contract,
    storage,
    noCommitValue,
    admin.publicKey
  );
  assert.match(noCommit.message, /commit required/i);
  assert.equal((await storage.get('fee/fiat/cum')), null);
  assert.equal((await storage.get('ev/use/1')), null);

  const commit = await execute(
    contract,
    storage,
    'epochCommit',
    {
      op: 'epoch_commit',
      epoch: 1,
      at: 3_600,
      roots: roll.roots,
      totals: roll.totals,
    },
    admin.publicKey,
    5
  );
  assert.equal(commit.ok, true, commit.message);

  const mismatch = await executeEpochApplyFeature(
    contract,
    storage,
    {
      op: 'epoch_apply',
      epoch: 1,
      at: 3_600,
      debits: roll.debits,
      earnings: roll.earnings,
      roots: { ...roll.roots, use: 'd'.repeat(64) },
      totals: roll.totals,
    },
    admin.publicKey
  );
  assert.match(mismatch.message, /do not match committed roots/i);
  assert.equal((await storage.get('fee/fiat/cum')), null);
  assert.equal((await storage.get('ev/use/1')), null);
  assert.equal((await storage.get(`bal/${user.publicKey}/fiat`)).value.au, '1000000');
});

test('epoch root recompute script CLI matches the imported independent recompute function', async () => {
  const { provider, user } = await setupEpochContract();
  const bundle = await receiptBundle(user, provider);
  const expected = await recomputeEpoch(bundle);
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), 'mayhem-epoch-'));
  const bundlePath = path.join(dir, 'bundle.json');
  await fs.writeFile(bundlePath, JSON.stringify(bundle, null, 2));

  const result = spawnSync(process.execPath, [scriptPath, bundlePath], {
    encoding: 'utf8',
  });
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(JSON.parse(result.stdout), expected);
});

test('epoch root recompute requires admin params fee_bps and rejects loose fee_bps', async () => {
  const { provider, user } = await setupEpochContract();
  const bundle = await receiptBundle(user, provider);
  const { prior_burn_cum_au: _priorBurnCumAu, ...missingBurnBundle } = bundle;

  await assert.rejects(
    recomputeEpoch(missingBurnBundle),
    /prior_burn_cum_au is required/
  );
  await assert.rejects(
    recomputeEpoch({
      ...bundle,
      params: undefined,
    }),
    /params\.fee_bps is required/
  );
  await assert.rejects(
    recomputeEpoch({
      ...bundle,
      fee_bps: 1_500,
    }),
    /top-level fee_bps is not accepted/
  );
  await assert.rejects(
    recomputeEpoch({
      ...bundle,
      params: { fee_bps: 1_501 },
    }),
    /fee_bps must be <= 1500/
  );
});

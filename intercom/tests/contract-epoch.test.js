import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import b4a from 'b4a';
import MayhemContract, { receiptMessage } from '../contract/contract.js';
import { recomputeEpoch } from '../scripts/recompute-epoch-roots.mjs';
import {
  MemoryStorage,
  execute,
  executeEpochApplyFeature,
  epochApplyFeatureKey,
  makeIdentity,
  makeTxKey,
  makeVerifier,
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

const receiptBundle = (user, provider, overrides = {}) => ({
  epoch: 1,
  params: {
    fee_bps: 1_500,
  },
  deposits: [],
  receipts: [
    {
      schema_version: 1,
      session_id: 'session-epoch-1',
      seq: 1,
      final: true,
      rail: 'fiat',
      user: user.publicKey,
      provider: provider.publicKey,
      enclave_id: enclaveId,
      model_id: modelId,
      price_ver: 1,
      locked_rate_map: textLockedRateMapFor(2_000),
      rules_ver: 1,
      usage: { in: 100, out: 250 },
      au_owed_cum: '2000',
      prompt_hash: 'a'.repeat(64),
      ts: 3_600,
      enclave_sig: 'b'.repeat(128),
      user_sig: 'c'.repeat(128),
    },
  ],
  payouts: [],
  ...overrides,
});

test('epoch recompute canonicalizes metered usage maps', async () => {
  const { provider, user } = await setupEpochContract();
  const legacyText = receiptBundle(user, provider);
  const canonicalText = receiptBundle(user, provider, {
    receipts: [
      {
        ...legacyText.receipts[0],
        schema_version: 2,
        locked_rate_map: textLockedRateMapFor(2_000),
        usage: { input_token: 100, output_token: 250 },
      },
    ],
  });

  assert.deepEqual((await recomputeEpoch(canonicalText)).roots, (await recomputeEpoch(legacyText)).roots);

  const imageAliasBundle = receiptBundle(user, provider, {
    receipts: [
      {
        ...legacyText.receipts[0],
        schema_version: 2,
        model_id: 'admin/image-small@fp16',
        locked_rate_map: imageLockedRateMap,
        usage: { images: 2, steps: 60 },
        au_owed_cum: '1120',
      },
    ],
  });
  const imageCanonicalBundle = receiptBundle(user, provider, {
    receipts: [
      {
        ...legacyText.receipts[0],
        schema_version: 2,
        model_id: 'admin/image-small@fp16',
        locked_rate_map: imageLockedRateMap,
        usage: { image: 2, step: 60 },
        au_owed_cum: '1120',
      },
    ],
  });
  const imageAliasRoll = await recomputeEpoch(imageAliasBundle);
  const imageCanonicalRoll = await recomputeEpoch(imageCanonicalBundle);

  assert.deepEqual(imageAliasRoll.roots, imageCanonicalRoll.roots);
  assert.equal(imageAliasRoll.totals.use_count, 1);
  assert.equal(imageAliasRoll.totals.use_au, '1120');
});

const signedReceipt = (user, provider, enclave, overrides = {}) => {
  const body = {
    schema_version: 1,
    session_id: 'session-epoch-1',
    seq: 1,
    final: true,
    rail: 'fiat',
    user: user.publicKey,
    provider: provider.publicKey,
    enclave_id: enclaveId,
    model_id: modelId,
    price_ver: 1,
    locked_rate_map: textLockedRateMapFor(1_000),
    rules_ver: 1,
    usage: { in: 100, out: 250 },
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
  const bundle = receiptBundle(user, provider);
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
    sweep_msb_tx_hash: null,
    ts: 3_600,
    updated_at: applyKey,
  });
  assert.equal(await storage.get('ev/pay/1'), null);
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

  const roll = await recomputeEpoch(receiptBundle(user, provider));
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

  const staleTimingApply = await executeEpochApplyFeature(
    contract,
    storage,
    {
      op: 'epoch_apply',
      epoch: 1,
      at: 86_399,
      debits: roll.debits,
      earnings: roll.earnings,
      roots: roll.roots,
      totals: roll.totals,
    },
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
  const inflatedRoll = await recomputeEpoch(receiptBundle(user, provider, {
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

  const voidApply = await executeEpochApplyFeature(
    contract,
    storage,
    {
      op: 'epoch_apply',
      epoch: 1,
      at: 3_600,
      debits: inflatedRoll.debits,
      earnings: inflatedRoll.earnings,
      roots: inflatedRoll.roots,
      totals: inflatedRoll.totals,
    },
    admin.publicKey
  );
  assert.match(voidApply.message, /commit is void/i);
  assert.equal((await storage.get('ev/use/1')), null);
  assert.equal((await storage.get(`bal/${user.publicKey}/fiat`)).value.au, '1000000');

  const emptyEpoch = await recomputeEpoch({
    epoch: 2,
    params: {
      fee_bps: 1_500,
    },
    deposits: [],
    receipts: [],
    payouts: [],
  });
  const bannedCommit = await execute(
    contract,
    storage,
    'epochCommit',
    {
      op: 'epoch_commit',
      epoch: 2,
      at: 7_200,
      roots: emptyEpoch.roots,
      totals: emptyEpoch.totals,
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
      roots: emptyEpoch.roots,
      totals: emptyEpoch.totals,
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

  const emptyEpoch3 = await recomputeEpoch({
    epoch: 3,
    params: {
      fee_bps: 1_500,
    },
    deposits: [],
    receipts: [],
    payouts: [],
  });
  const restoredCommit = await execute(
    contract,
    storage,
    'epochCommit',
    {
      op: 'epoch_commit',
      epoch: 3,
      at: 10_800,
      roots: emptyEpoch3.roots,
      totals: emptyEpoch3.totals,
    },
    submitter.publicKey,
    11
  );
  assert.equal(restoredCommit.ok, true, restoredCommit.message);
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
  const inflatedRoll = await recomputeEpoch(receiptBundle(user, provider, {
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
  const inflatedRoll = await recomputeEpoch(receiptBundle(user, provider, {
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
  const roll = await recomputeEpoch(receiptBundle(user, provider));

  const noCommit = await executeEpochApplyFeature(
    contract,
    storage,
    {
      op: 'epoch_apply',
      epoch: 1,
      at: 3_600,
      debits: roll.debits,
      earnings: roll.earnings,
      roots: roll.roots,
      totals: roll.totals,
    },
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
  const bundle = receiptBundle(user, provider);
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
  const bundle = receiptBundle(user, provider);

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
      params: { fee_bps: 5_001 },
    }),
    /fee_bps must be <= 5000/
  );
});

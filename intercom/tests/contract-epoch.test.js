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
  makeIdentity,
  makeTxKey,
  makeVerifier,
  signConsent,
} from './helpers/contract.js';

const rulesHash = '6'.repeat(64);
const enclaveId = 'e'.repeat(64);
const modelId = 'meta/llama-3.1-8b-instruct@4bit';
const scriptPath = fileURLToPath(new URL('../scripts/recompute-epoch-roots.mjs', import.meta.url));

const seededBalance = (user, mu) => ({
  user,
  denom: 'mu_usd',
  mu,
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
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }

  await storage.put(`bal/${user.publicKey}`, seededBalance(user.publicKey, 1_000_000));
  return { admin, provider, user, submitter, storage, contract };
}

const receiptBundle = (user, provider, overrides = {}) => ({
  epoch: 1,
  fee_bps: 1_500,
  deposits: [],
  receipts: [
    {
      schema_version: 1,
      session_id: 'session-epoch-1',
      seq: 1,
      final: true,
      user: user.publicKey,
      provider: provider.publicKey,
      enclave_id: enclaveId,
      model_id: modelId,
      price_ver: 1,
      rules_ver: 1,
      usage: { in: 100, out: 250 },
      mu_owed_cum: 2_000,
      prompt_hash: 'a'.repeat(64),
      ts: 3_600,
      enclave_sig: 'b'.repeat(128),
      user_sig: 'c'.repeat(128),
    },
  ],
  payouts: [],
  ...overrides,
});

const signedReceipt = (user, provider, enclave, overrides = {}) => {
  const body = {
    schema_version: 1,
    session_id: 'session-epoch-1',
    seq: 1,
    final: true,
    user: user.publicKey,
    provider: provider.publicKey,
    enclave_id: enclaveId,
    model_id: modelId,
    price_ver: 1,
    rules_ver: 1,
    usage: { in: 100, out: 250 },
    mu_owed_cum: 1_000,
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
      roots: { ...roll.roots, pay: 'f'.repeat(64) },
      totals: roll.totals,
    },
    submitter.publicKey,
    6
  );
  assert.match(changed.message, /already exists/i);

  const applied = await execute(
    contract,
    storage,
    'epochApply',
    {
      op: 'epoch_apply',
      epoch: 1,
      at: 3_600,
      debits: roll.debits,
      earnings: roll.earnings,
      roots: roll.roots,
      totals: roll.totals,
    },
    admin.publicKey,
    7
  );
  assert.deepEqual(applied, {
    ok: true,
    op: 'epochApply',
    epoch: 1,
    idempotent: false,
    debited_mu: 2_000,
    earned_mu: 1_700,
    fee_mu: 300,
  });

  assert.deepEqual((await storage.get(`bal/${user.publicKey}`)).value, {
    user: user.publicKey,
    denom: 'mu_usd',
    mu: 998_000,
    updated_epoch: 1,
    updated_at: makeTxKey(7),
  });
  assert.deepEqual((await storage.get(`earn/${provider.publicKey}`)).value, {
    provider: provider.publicKey,
    denom: 'mu_usd',
    total_mu: 1_700,
    held_mu: 1_700,
    paid_cum_mu: 0,
    holdbacks: [{ epoch: 1, mu: 1_700 }],
    updated_epoch: 1,
    updated_at: makeTxKey(7),
    last_holdback_release_epoch: 1,
  });
  assert.deepEqual((await storage.get('ev/dep/1')).value, {
    type: 'deposit_root',
    epoch: 1,
    merkle_root: roll.roots.dep,
    count: 0,
    mu_total: 0,
    ts: 3_600,
    updated_at: makeTxKey(7),
  });
  assert.deepEqual((await storage.get('ev/use/1')).value, {
    type: 'usage_root',
    epoch: 1,
    merkle_root: roll.roots.use,
    sessions: 1,
    mu_total: 2_000,
    providers: 1,
    ts: 3_600,
    updated_at: makeTxKey(7),
  });
  assert.deepEqual((await storage.get('ev/earn/1')).value, {
    type: 'earn_root',
    epoch: 1,
    merkle_root: roll.roots.earn,
    provider_count: 1,
    mu_cum_total: 1_700,
    ts: 3_600,
    updated_at: makeTxKey(7),
  });
  assert.deepEqual((await storage.get('ev/fee/1')).value, {
    type: 'fee_root',
    epoch: 1,
    merkle_root: roll.roots.fee,
    mu_fee_epoch: 300,
    mu_fee_cum: 300,
    sweep_msb_tx_hash: null,
    ts: 3_600,
    updated_at: makeTxKey(7),
  });
  assert.deepEqual((await storage.get('ev/pay/1')).value, {
    type: 'payout_root',
    epoch: 1,
    merkle_root: roll.roots.pay,
    count: 0,
    mu_total: 0,
    ts: 3_600,
    updated_at: makeTxKey(7),
  });
});

test('MayhemContract fraudProof voids an inflated single-receipt commit and bans submitter', async () => {
  const { admin, provider, user, submitter, storage, contract } = await setupEpochContract();
  const enclave = await makeIdentity();
  const prover = await makeIdentity();
  const otherSubmitter = await makeIdentity();
  const receipt = signedReceipt(user, provider, enclave, { mu_owed_cum: 1_000 });
  const inflatedReceipt = {
    ...receipt,
    body: {
      ...receipt.body,
      mu_owed_cum: 2_000,
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
  assert.equal(inflatedRoll.totals.use_mu, 2_000);

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
      claimed_mu_owed_cum: 2_000,
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
  assert.equal(fraudRecord.actual_mu, 1_000);
  assert.equal(fraudRecord.claimed_mu, 2_000);
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
      claimed_mu_owed_cum: 2_000,
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

  const voidApply = await execute(
    contract,
    storage,
    'epochApply',
    {
      op: 'epoch_apply',
      epoch: 1,
      at: 3_600,
      debits: inflatedRoll.debits,
      earnings: inflatedRoll.earnings,
      roots: inflatedRoll.roots,
      totals: inflatedRoll.totals,
    },
    admin.publicKey,
    7
  );
  assert.match(voidApply.message, /commit is void/i);
  assert.equal((await storage.get('ev/use/1')), null);
  assert.equal((await storage.get(`bal/${user.publicKey}`)).value.mu, 1_000_000);

  const emptyEpoch = await recomputeEpoch({
    epoch: 2,
    fee_bps: 1_500,
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
});

test('MayhemContract fraudProof rejects proofs after the challenge window', async () => {
  const { provider, user, submitter, storage, contract } = await setupEpochContract();
  const enclave = await makeIdentity();
  const prover = await makeIdentity();
  const receipt = signedReceipt(user, provider, enclave, { mu_owed_cum: 1_000 });
  const inflatedRoll = await recomputeEpoch(receiptBundle(user, provider, {
    receipts: [{
      ...receipt,
      body: {
        ...receipt.body,
        mu_owed_cum: 2_000,
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
      claimed_mu_owed_cum: 2_000,
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

  const noCommit = await execute(
    contract,
    storage,
    'epochApply',
    {
      op: 'epoch_apply',
      epoch: 1,
      at: 3_600,
      debits: roll.debits,
      earnings: roll.earnings,
      roots: roll.roots,
      totals: roll.totals,
    },
    admin.publicKey,
    4
  );
  assert.match(noCommit.message, /commit required/i);
  assert.equal((await storage.get('fee/cum')), null);
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

  const mismatch = await execute(
    contract,
    storage,
    'epochApply',
    {
      op: 'epoch_apply',
      epoch: 1,
      at: 3_600,
      debits: roll.debits,
      earnings: roll.earnings,
      roots: { ...roll.roots, use: 'd'.repeat(64) },
      totals: roll.totals,
    },
    admin.publicKey,
    6
  );
  assert.match(mismatch.message, /do not match committed roots/i);
  assert.equal((await storage.get('fee/cum')), null);
  assert.equal((await storage.get('ev/use/1')), null);
  assert.equal((await storage.get(`bal/${user.publicKey}`)).value.mu, 1_000_000);
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

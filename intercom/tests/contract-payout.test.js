import assert from 'node:assert/strict';
import test from 'node:test';
import MayhemContract from '../contract/contract.js';
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

const rulesHash = '9'.repeat(64);
const enclaveId = '7'.repeat(64);
const modelId = 'mayhem/payout-rollup-test@q4';
const retiredPayoutMessage = /unknown contract operation type|function not registered/i;

async function setupPayoutContract() {
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
      type: 'setProviderPayout',
      value: {
        op: 'set_provider_payout',
        provider: provider.publicKey,
        payout_addr: 'trac1providerpayouttarget',
        payout_method: 'tnk',
      },
      sender: admin.publicKey,
      txNo: 4,
    },
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }

  await storage.put(`bal/${user.publicKey}`, {
    user: user.publicKey,
    denom: 'mu_usd',
    mu: 5_000_000,
    updated_epoch: 0,
    updated_at: null,
  });
  return { admin, provider, user, submitter, storage, contract };
}

const epochApply = (epoch, user, provider, grossMu, overrides = {}) => ({
  op: 'epoch_apply',
  epoch,
  at: epoch * 3_600,
  debits: [{ user, mu: grossMu }],
  earnings: [{ provider, gross_mu: grossMu }],
  ...overrides,
});

const receiptBundle = (user, provider, overrides = {}) => ({
  epoch: 1,
  params: {
    fee_bps: 1_500,
  },
  deposits: [],
  receipts: [
    {
      schema_version: 1,
      session_id: 'session-payout-rollup-1',
      seq: 1,
      final: true,
      user,
      provider,
      enclave_id: enclaveId,
      model_id: modelId,
      price_ver: 1,
      rules_ver: 1,
      usage: { in: 100, out: 250 },
      mu_owed_cum: 2_000_000,
      prompt_hash: 'a'.repeat(64),
      ts: 3_600,
      enclave_sig: 'b'.repeat(128),
      user_sig: 'c'.repeat(128),
    },
  ],
  payouts: [],
  ...overrides,
});

const payoutConfirm = (provider, overrides = {}) => ({
  op: 'payout_confirm',
  epoch: 169,
  who: provider,
  mu: 1_000_000,
  tnk_e18: '500000000000000000',
  msb_tx_hash: 'a'.repeat(64),
  at: 1_900,
  ...overrides,
});

test('MayhemContract setProviderPayout stamps admin authority evidence', async () => {
  const { admin, provider, storage, contract } = await setupPayoutContract();
  const registered = (await storage.get(`prov/${provider.publicKey}`)).value;
  assert.deepEqual(registered.payout, {
    addr: 'trac1providerpayouttarget',
    method: 'tnk',
    set_by: admin.publicKey,
    set_by_role: 'admin',
    set_at: makeTxKey(4),
  });

  const retargeted = await execute(
    contract,
    storage,
    'setProviderPayout',
    {
      op: 'set_provider_payout',
      provider: provider.publicKey,
      payout_addr: 'acct_test_provider',
      payout_method: 'stripe',
      payout_currency: 'eur',
    },
    admin.publicKey,
    6
  );
  assert.equal(retargeted.ok, true, retargeted.message);
  const updated = (await storage.get(`prov/${provider.publicKey}`)).value;
  assert.deepEqual(updated.payout, {
    addr: 'acct_test_provider',
    method: 'stripe',
    currency: 'eur',
    set_by: admin.publicKey,
    set_by_role: 'admin',
    set_at: makeTxKey(6),
  });

  const tapTarget = await execute(
    contract,
    storage,
    'setProviderPayout',
    {
      op: 'set_provider_payout',
      provider: provider.publicKey,
      payout_addr: '0x' + '3'.repeat(40),
      payout_method: 'tap',
    },
    admin.publicKey,
    7
  );
  assert.equal(tapTarget.ok, true, tapTarget.message);
  const tapUpdated = (await storage.get(`prov/${provider.publicKey}`)).value;
  assert.deepEqual(tapUpdated.payout, {
    addr: '0x' + '3'.repeat(40),
    method: 'tap',
    set_by: admin.publicKey,
    set_by_role: 'admin',
    set_at: makeTxKey(7),
  });
});

test('MayhemContract epoch roots commit provider entitlements without ev/pay evidence', async () => {
  const { admin, provider, user, submitter, storage, contract } = await setupPayoutContract();
  const roll = await recomputeEpoch(receiptBundle(user.publicKey, provider.publicKey));

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
    5
  );
  assert.equal(commit.ok, true, commit.message);

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
    debited_mu: 2_000_000,
    earned_mu: 1_700_000,
    fee_mu: 300_000,
  });

  assert.deepEqual((await storage.get(`earn/${provider.publicKey}`)).value, {
    provider: provider.publicKey,
    denom: 'mu_usd',
    total_mu: 1_700_000,
    held_mu: 1_700_000,
    paid_cum_mu: 0,
    holdbacks: [{ epoch: 1, mu: 1_700_000 }],
    updated_epoch: 1,
    updated_at: applyKey,
    last_holdback_release_epoch: 1,
  });
  assert.deepEqual((await storage.get('ev/earn/1')).value, {
    type: 'earn_root',
    epoch: 1,
    merkle_root: roll.roots.earn,
    provider_count: 1,
    mu_cum_total: 1_700_000,
    ts: 3_600,
    updated_at: applyKey,
  });
  assert.deepEqual((await storage.get('ev/fee/1')).value, {
    type: 'fee_root',
    epoch: 1,
    merkle_root: roll.roots.fee,
    mu_fee_epoch: 300_000,
    mu_fee_cum: 300_000,
    sweep_msb_tx_hash: null,
    ts: 3_600,
    updated_at: applyKey,
  });
  assert.equal(await storage.get('ev/pay/1'), null);
});

test('MayhemContract provider payoutConfirm is retired and cannot mutate state', async () => {
  const { admin, provider, user, storage, contract } = await setupPayoutContract();
  const applyValue = epochApply(1, user.publicKey, provider.publicKey, 2_000_000);
  const applied = await executeEpochApplyFeature(
    contract,
    storage,
    applyValue,
    admin.publicKey
  );
  assert.equal(applied.ok, true, applied.message);

  const before = storage.snapshotBytes();
  const rejected = await execute(
    contract,
    storage,
    'payoutConfirm',
    payoutConfirm(provider.publicKey),
    admin.publicKey,
    7
  );
  assert.match(rejected.message, retiredPayoutMessage);
  assert.equal(storage.snapshotBytes(), before);
  assert.equal(await storage.get('ev/pay/169'), null);
});

test('MayhemContract fee-sweep payoutConfirm is retired and cannot mutate fee evidence', async () => {
  const { admin, storage, contract } = await setupPayoutContract();
  await storage.put('fee/cum', {
    denom: 'mu_usd',
    cum_mu: 2_000_000,
    swept_cum_mu: 0,
    settled_cum_mu: 2_000_000,
    updated_epoch: 1,
    updated_at: null,
    last_apply_hash: null,
    last_fee_bps: 1_500,
  });
  await storage.put('ev/fee/169', {
    type: 'fee_root',
    epoch: 169,
    merkle_root: 'f'.repeat(64),
    mu_fee_epoch: 0,
    mu_fee_cum: 2_000_000,
    sweep_msb_tx_hash: null,
    ts: 1_900,
    updated_at: null,
  });

  const before = storage.snapshotBytes();
  const rejected = await execute(
    contract,
    storage,
    'payoutConfirm',
    payoutConfirm('treasury', { kind: 'fee_sweep', msb_tx_hash: 'b'.repeat(64) }),
    admin.publicKey,
    6
  );
  assert.match(rejected.message, retiredPayoutMessage);
  assert.equal(storage.snapshotBytes(), before);
});

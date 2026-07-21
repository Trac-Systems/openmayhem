import assert from 'node:assert/strict';
import test from 'node:test';
import MayhemContract from '../contract/contract.js';
import {
  MemoryStorage,
  execute,
  executeEpochApplyFeature,
  makeIdentity,
  makeVerifier,
  seedSpendHold,
  seedSpendHoldsForApply,
  signConsent,
} from './helpers/contract.js';

const rulesHash = '8'.repeat(64);

const seededBalance = (user, au, rail = 'fiat') => ({
  user,
  rail,
  denom: 'au_usd',
  au: String(au),
  updated_epoch: 0,
  updated_at: null,
});

async function setupGuardianContract() {
  const admin = await makeIdentity();
  const provider = await makeIdentity();
  const user = await makeIdentity();
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

  const payments = await execute(
    contract,
    storage,
    'setPayments',
    {
      op: 'set_payments',
      ver: 1,
      fiat: {
        processor: 'stripe',
        integration_currency: 'usd',
        adaptive_pricing: true,
        payout_currencies: ['eur', 'gbp', 'usd'],
        locale: 'en',
      },
      tap: {
        chain_id: 61_000,
        token_address: `0x${'1'.repeat(40)}`,
        pool_address: `0x${'2'.repeat(40)}`,
      },
      tnk: { network: 'testnet1', treasury_address: `testtrac1${'1'.repeat(40)}` },
    },
    admin.publicKey,
    100_000
  );
  assert.equal(payments.ok, true, payments.message);

  await storage.put(`bal/${user.publicKey}/fiat`, seededBalance(user.publicKey, 10_000));
  return { admin, provider, user, storage, contract };
}

const epochApply = (epoch, user, provider, grossAu = 1_000) => ({
  op: 'epoch_apply',
  epoch,
  at: epoch * 3_600,
  debits: [{ rail: 'fiat', user, au: String(grossAu) }],
  earnings: [{ rail: 'fiat', provider, gross_au: String(grossAu) }],
});

test('MayhemContract guardian halts epochApply on conservation failure', async () => {
  const { admin, provider, user, storage, contract } = await setupGuardianContract();
  await storage.put('fee/fiat/cum', {
    rail: 'fiat',
    denom: 'au_usd',
    cum_au: '5000',
    swept_cum_au: '0',
    settled_cum_au: '4000',
    updated_epoch: 0,
    updated_at: null,
    last_apply_hash: null,
    last_fee_bps: null,
  });
  const applyValue = epochApply(1, user.publicKey, provider.publicKey);
  await seedSpendHoldsForApply(storage, applyValue);
  const before = storage.snapshotBytes();

  const result = await executeEpochApplyFeature(
    contract,
    storage,
    applyValue,
    admin.publicKey
  );
  assert.match(result.message, /guardian conservation/i);
  assert.equal(storage.snapshotBytes(), before);
});

test('MayhemContract guardian halts epochApply on non-monotonic epochs', async () => {
  const { admin, provider, user, storage, contract } = await setupGuardianContract();
  await seedSpendHold(storage, { user: user.publicKey, epoch: 1, au: '2000' });
  const first = await executeEpochApplyFeature(
    contract,
    storage,
    epochApply(1, user.publicKey, provider.publicKey),
    admin.publicKey
  );
  assert.equal(first.ok, true, first.message);
  const before = storage.snapshotBytes();

  const replayWithChangedDelta = await executeEpochApplyFeature(
    contract,
    storage,
    epochApply(1, user.publicKey, provider.publicKey, 2_000),
    admin.publicKey
  );
  assert.match(replayWithChangedDelta.message, /guardian monotonic epoch/i);
  assert.equal(storage.snapshotBytes(), before);
});

test('MayhemContract guardian halts epochApply on negative balances', async () => {
  const { admin, provider, user, storage, contract } = await setupGuardianContract();
  await storage.put(`bal/${user.publicKey}/fiat`, seededBalance(user.publicKey, -1));
  const applyValue = epochApply(1, user.publicKey, provider.publicKey);
  await seedSpendHoldsForApply(storage, applyValue);
  const before = storage.snapshotBytes();

  const result = await executeEpochApplyFeature(
    contract,
    storage,
    applyValue,
    admin.publicKey
  );
  assert.match(result.message, /guardian non-negative balance/i);
  assert.equal(storage.snapshotBytes(), before);
});

test('MayhemContract guardian requires pool scope for every nonzero TAP monetary record', async () => {
  const contract = new MayhemContract({}, {});
  const balance = {
    user: '1'.repeat(64),
    rail: 'tap',
    denom: 'au_usd',
    au: '1',
    updated_epoch: 0,
    updated_at: null,
  };
  assert.match(
    contract.guardianValidateBalanceRecord(balance, balance.user, 'tap').message,
    /TAP balance scope invariant/i
  );
  assert.equal(
    contract.guardianValidateBalanceRecord({ ...balance, au: '0' }, balance.user, 'tap'),
    null
  );

  const earning = {
    provider: '2'.repeat(64),
    rail: 'tap',
    denom: 'au_usd',
    total_au: '1',
    held_au: '1',
    paid_cum_au: '0',
    updated_epoch: 0,
    updated_at: null,
  };
  assert.match(
    contract.guardianValidateEarningRecord(earning, earning.provider, 'tap').message,
    /TAP earning scope invariant/i
  );

  const fee = {
    rail: 'tap',
    denom: 'au_usd',
    cum_au: '1',
    swept_cum_au: '0',
    settled_cum_au: '1',
    updated_epoch: 0,
    updated_at: null,
    last_apply_hash: null,
    last_fee_bps: 1_500,
  };
  assert.match(
    contract.guardianValidateFeeRecord(fee, 'tap').message,
    /TAP fee scope invariant/i
  );

  const burn = {
    rail: 'tap',
    denom: 'au_usd',
    cum_au: '1',
    updated_epoch: 0,
    updated_at: null,
    last_apply_hash: null,
    burn_bps: 1_000,
  };
  assert.match(
    contract.guardianValidateBurnRecord(burn, 'tap').message,
    /TAP burn scope invariant/i
  );
});

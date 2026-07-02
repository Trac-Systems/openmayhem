import assert from 'node:assert/strict';
import test from 'node:test';
import MayhemContract from '../contract/contract.js';
import {
  MemoryStorage,
  execute,
  makeIdentity,
  makeVerifier,
  signConsent,
} from './helpers/contract.js';

const rulesHash = '8'.repeat(64);

const seededBalance = (user, mu) => ({
  user,
  denom: 'mu_usd',
  mu,
  updated_epoch: 0,
  updated_at: null,
});

const earningRecord = (provider, overrides = {}) => ({
  provider,
  denom: 'mu_usd',
  total_mu: 5_000,
  held_mu: 0,
  paid_cum_mu: 0,
  updated_epoch: 0,
  updated_at: null,
  ...overrides,
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
    {
      type: 'setProviderPayout',
      value: {
        op: 'set_provider_payout',
        provider: provider.publicKey,
        payout_addr: 'trac1guardianprovider',
        payout_method: 'tnk',
      },
      sender: admin.publicKey,
      txNo: 4,
    },
    {
      type: 'rateOracle',
      value: {
        op: 'rate_oracle',
        tnk_usd_e6: 2_000_000,
        source: 'coinbase-spot',
        ts: 1_000,
      },
      sender: admin.publicKey,
      txNo: 5,
    },
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }

  await storage.put(`bal/${user.publicKey}`, seededBalance(user.publicKey, 10_000));
  return { admin, provider, user, storage, contract };
}

const epochApply = (epoch, user, provider, grossMu = 1_000) => ({
  op: 'epoch_apply',
  epoch,
  at: epoch * 3_600,
  debits: [{ user, mu: grossMu }],
  earnings: [{ provider, gross_mu: grossMu }],
});

const payoutConfirm = (provider, overrides = {}) => ({
  op: 'payout_confirm',
  who: provider,
  epoch: 200,
  mu: 1_000,
  tnk_e18: '500000000000000',
  msb_tx_hash: 'a'.repeat(64),
  at: 1_500,
  ...overrides,
});

test('MayhemContract guardian halts epochApply on conservation failure', async () => {
  const { admin, provider, user, storage, contract } = await setupGuardianContract();
  await storage.put('fee/cum', {
    denom: 'mu_usd',
    cum_mu: 5_000,
    swept_cum_mu: 0,
    settled_cum_mu: 4_000,
    updated_epoch: 0,
    updated_at: null,
    last_apply_hash: null,
    last_fee_bps: null,
  });
  const before = storage.snapshotBytes();

  const result = await execute(
    contract,
    storage,
    'epochApply',
    epochApply(1, user.publicKey, provider.publicKey),
    admin.publicKey,
    5
  );
  assert.match(result.message, /guardian conservation/i);
  assert.equal(storage.snapshotBytes(), before);
});

test('MayhemContract guardian halts epochApply on non-monotonic epochs', async () => {
  const { admin, provider, user, storage, contract } = await setupGuardianContract();
  const first = await execute(
    contract,
    storage,
    'epochApply',
    epochApply(1, user.publicKey, provider.publicKey),
    admin.publicKey,
    5
  );
  assert.equal(first.ok, true, first.message);
  const before = storage.snapshotBytes();

  const replayWithChangedDelta = await execute(
    contract,
    storage,
    'epochApply',
    epochApply(1, user.publicKey, provider.publicKey, 2_000),
    admin.publicKey,
    6
  );
  assert.match(replayWithChangedDelta.message, /guardian monotonic epoch/i);
  assert.equal(storage.snapshotBytes(), before);
});

test('MayhemContract guardian halts epochApply on negative balances', async () => {
  const { admin, provider, user, storage, contract } = await setupGuardianContract();
  await storage.put(`bal/${user.publicKey}`, seededBalance(user.publicKey, -1));
  const before = storage.snapshotBytes();

  const result = await execute(
    contract,
    storage,
    'epochApply',
    epochApply(1, user.publicKey, provider.publicKey),
    admin.publicKey,
    5
  );
  assert.match(result.message, /guardian non-negative balance/i);
  assert.equal(storage.snapshotBytes(), before);
});

test('MayhemContract guardian halts payoutConfirm on stale rates', async () => {
  const { admin, provider, storage, contract } = await setupGuardianContract();
  await storage.put(`earn/${provider.publicKey}`, earningRecord(provider.publicKey, {
    total_mu: 2_000_000,
  }));
  const before = storage.snapshotBytes();

  const result = await execute(
    contract,
    storage,
    'payoutConfirm',
    payoutConfirm(provider.publicKey, {
      at: 1_901,
      mu: 1_000_000,
      tnk_e18: '500000000000000000',
    }),
    admin.publicKey,
    5
  );
  assert.match(result.message, /guardian rate freshness.*rate oracle is stale/i);
  assert.equal(storage.snapshotBytes(), before);
});

test('MayhemContract guardian halts payoutConfirm on earnings conservation failure', async () => {
  const { admin, provider, storage, contract } = await setupGuardianContract();
  await storage.put(`earn/${provider.publicKey}`, earningRecord(provider.publicKey, {
    total_mu: 1_000,
    held_mu: 900,
    paid_cum_mu: 200,
  }));
  const before = storage.snapshotBytes();

  const result = await execute(
    contract,
    storage,
    'payoutConfirm',
    payoutConfirm(provider.publicKey),
    admin.publicKey,
    5
  );
  assert.match(result.message, /guardian earnings conservation/i);
  assert.equal(storage.snapshotBytes(), before);
});

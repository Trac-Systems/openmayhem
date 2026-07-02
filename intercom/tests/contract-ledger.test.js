import assert from 'node:assert/strict';
import test from 'node:test';
import MayhemContract from '../contract/contract.js';
import {
  MemoryStorage,
  execute,
  makeIdentity,
  makeTxKey,
  makeVerifier,
  signConsent,
} from './helpers/contract.js';

const rulesHash = '7'.repeat(64);

const providerRegistration = {
  op: 'register_provider',
  payout_addr: 'trac1providerpayouttarget',
  payout_method: 'tnk',
};

const seededBalance = (user, mu) => ({
  user,
  denom: 'mu_usd',
  mu,
  updated_epoch: 0,
  updated_at: null,
});

const paymentKeys = (storage) =>
  Array.from(storage.values.keys())
    .filter((key) => key.startsWith('bal/') || key.startsWith('earn/') || key === 'fee/cum')
    .sort();

const makeEpochApply = (epoch, user, provider, grossMu) => ({
  op: 'epoch_apply',
  epoch,
  at: epoch * 3600,
  debits: [{ user, mu: grossMu }],
  earnings: [{ provider, gross_mu: grossMu }],
});

async function setupLedgerContract(identities = null) {
  const admin = identities?.admin ?? await makeIdentity();
  const provider = identities?.provider ?? await makeIdentity();
  const user = identities?.user ?? await makeIdentity();
  const outsider = identities?.outsider ?? await makeIdentity();
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
      value: providerRegistration,
      sender: provider.publicKey,
      txNo: 3,
    },
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }

  await storage.put(`bal/${user.publicKey}`, seededBalance(user.publicKey, 1_000_000));
  return { admin, provider, user, outsider, storage, contract };
}

test('MayhemContract epochApply mutates credit, earning, and fee state in place', async () => {
  const { admin, provider, user, outsider, storage, contract } = await setupLedgerContract();

  const nonAdmin = await execute(
    contract,
    storage,
    'epochApply',
    makeEpochApply(1, user.publicKey, provider.publicKey, 1_500),
    outsider.publicKey,
    4
  );
  assert.match(nonAdmin.message, /admin required/i);

  const mismatch = await execute(
    contract,
    storage,
    'epochApply',
    {
      op: 'epoch_apply',
      epoch: 1,
      at: 3600,
      debits: [{ user: user.publicKey, mu: 1_500 }],
      earnings: [{ provider: provider.publicKey, gross_mu: 1_400 }],
    },
    admin.publicKey,
    5
  );
  assert.match(mismatch.message, /must equal/i);

  const firstApply = {
    op: 'epoch_apply',
    epoch: 1,
    at: 3600,
    debits: [
      { user: user.publicKey, mu: 1_000 },
      { user: user.publicKey, mu: 500 },
    ],
    earnings: [
      { provider: provider.publicKey, gross_mu: 1_250 },
      { provider: provider.publicKey, gross_mu: 250 },
    ],
  };
  const first = await execute(
    contract,
    storage,
    'epochApply',
    firstApply,
    admin.publicKey,
    6
  );
  assert.deepEqual(first, {
    ok: true,
    op: 'epochApply',
    epoch: 1,
    idempotent: false,
    debited_mu: 1_500,
    earned_mu: 1_275,
    fee_mu: 225,
  });

  assert.deepEqual((await storage.get(`bal/${user.publicKey}`)).value, {
    user: user.publicKey,
    denom: 'mu_usd',
    mu: 998_500,
    updated_epoch: 1,
    updated_at: makeTxKey(6),
  });
  assert.deepEqual((await storage.get(`earn/${provider.publicKey}`)).value, {
    provider: provider.publicKey,
    denom: 'mu_usd',
    total_mu: 1_275,
    held_mu: 1_275,
    paid_cum_mu: 0,
    updated_epoch: 1,
    updated_at: makeTxKey(6),
  });
  const feeAfterFirst = (await storage.get('fee/cum')).value;
  assert.equal(feeAfterFirst.denom, 'mu_usd');
  assert.equal(feeAfterFirst.cum_mu, 225);
  assert.equal(feeAfterFirst.swept_cum_mu, 0);
  assert.equal(feeAfterFirst.updated_epoch, 1);
  assert.equal(feeAfterFirst.updated_at, makeTxKey(6));
  assert.equal(feeAfterFirst.last_fee_bps, 1_500);
  assert.equal(feeAfterFirst.last_apply_hash.length, 64);

  const snapshotBeforeReplay = storage.snapshotBytes();
  const replay = await execute(
    contract,
    storage,
    'epochApply',
    firstApply,
    admin.publicKey,
    7
  );
  assert.deepEqual(replay, {
    ok: true,
    op: 'epochApply',
    epoch: 1,
    idempotent: true,
    debited_mu: 0,
    earned_mu: 0,
    fee_mu: 0,
  });
  assert.equal(storage.snapshotBytes(), snapshotBeforeReplay);

  const changedReplay = await execute(
    contract,
    storage,
    'epochApply',
    makeEpochApply(1, user.publicKey, provider.publicKey, 2_000),
    admin.publicKey,
    8
  );
  assert.match(changedReplay.message, /monotonic/i);

  const gap = await execute(
    contract,
    storage,
    'epochApply',
    makeEpochApply(3, user.publicKey, provider.publicKey, 1_000),
    admin.publicKey,
    9
  );
  assert.match(gap.message, /contiguous/i);

  const insufficientSnapshot = storage.snapshotBytes();
  const insufficient = await execute(
    contract,
    storage,
    'epochApply',
    makeEpochApply(2, user.publicKey, provider.publicKey, 2_000_000),
    admin.publicKey,
    10
  );
  assert.match(insufficient.message, /insufficient credit balance/i);
  assert.equal(storage.snapshotBytes(), insufficientSnapshot);
});

test('MayhemContract epochApply is deterministic and payment key growth stays flat over 100 epochs', async () => {
  const identities = {
    admin: await makeIdentity(),
    provider: await makeIdentity(),
    user: await makeIdentity(),
    outsider: await makeIdentity(),
  };
  const left = await setupLedgerContract(identities);
  const right = await setupLedgerContract(identities);
  let expectedDebited = 0;
  let expectedFee = 0;

  let totalKeysAfterFirst = null;
  let paymentKeysAfterFirst = null;
  for (let epoch = 1; epoch <= 100; epoch++) {
    const grossMu = 1_000 + (epoch % 7);
    expectedDebited += grossMu;
    expectedFee += Math.floor((grossMu * 1_500) / 10_000);
    for (const ctx of [left, right]) {
      const result = await execute(
        ctx.contract,
        ctx.storage,
        'epochApply',
        makeEpochApply(epoch, identities.user.publicKey, identities.provider.publicKey, grossMu),
        identities.admin.publicKey,
        10 + epoch
      );
      assert.equal(result.ok, true, result.message);
      assert.equal(result.epoch, epoch);
    }

    if (epoch === 1) {
      totalKeysAfterFirst = left.storage.values.size;
      paymentKeysAfterFirst = paymentKeys(left.storage);
    }
  }

  assert.equal(left.storage.snapshotBytes(), right.storage.snapshotBytes());
  assert.equal(left.storage.values.size, totalKeysAfterFirst);
  assert.deepEqual(paymentKeys(left.storage), paymentKeysAfterFirst);
  assert.deepEqual(paymentKeysAfterFirst, [
    `bal/${identities.user.publicKey}`,
    `earn/${identities.provider.publicKey}`,
    'fee/cum',
  ].sort());

  const balance = (await left.storage.get(`bal/${identities.user.publicKey}`)).value;
  const earning = (await left.storage.get(`earn/${identities.provider.publicKey}`)).value;
  const fee = (await left.storage.get('fee/cum')).value;
  assert.equal(balance.mu, 1_000_000 - expectedDebited);
  assert.equal(earning.total_mu, expectedDebited - expectedFee);
  assert.equal(earning.held_mu, expectedDebited - expectedFee);
  assert.equal(earning.paid_cum_mu, 0);
  assert.equal(fee.cum_mu, expectedFee);
  assert.equal(fee.updated_epoch, 100);
});

test('MayhemContract epochApply enforces max_apply_batch before writing', async () => {
  const { admin, provider, storage, contract } = await setupLedgerContract();
  const before = storage.snapshotBytes();
  const tooManyDebits = Array.from({ length: 501 }, (_, i) => ({
    user: `user-${i}`,
    mu: 1,
  }));
  const tooLarge = await execute(
    contract,
    storage,
    'epochApply',
    {
      op: 'epoch_apply',
      epoch: 1,
      at: 3600,
      debits: tooManyDebits,
      earnings: [{ provider: provider.publicKey, gross_mu: 501 }],
    },
    admin.publicKey,
    4
  );
  assert.match(tooLarge.message, /max_apply_batch/i);
  assert.equal(storage.snapshotBytes(), before);
});

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

const rulesHash = '5'.repeat(64);
const oneTnkE18 = '1000000000000000000';
const halfTnkE18 = '500000000000000000';

async function setupDepositContract(identities = null) {
  const admin = identities?.admin ?? await makeIdentity();
  const user = identities?.user ?? await makeIdentity();
  const outsider = identities?.outsider ?? await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const protocol = { peer: { wallet: makeVerifier(user.wallet) } };
  const contract = new MayhemContract(protocol, {});

  const rules = await execute(
    contract,
    storage,
    'setRules',
    { op: 'set_rules', ver: 1, hash: rulesHash },
    admin.publicKey,
    1
  );
  assert.equal(rules.ok, true, rules.message);

  return { admin, user, outsider, storage, contract };
}

async function consentUser(ctx, txNo = 2) {
  const result = await execute(
    ctx.contract,
    ctx.storage,
    'consent',
    {
      op: 'consent',
      ver: 1,
      hash: rulesHash,
      sig: signConsent(ctx.user.wallet, 1, rulesHash),
    },
    ctx.user.publicKey,
    txNo
  );
  assert.equal(result.ok, true, result.message);
}

async function setRate(ctx, txNo = 3) {
  const result = await execute(
    ctx.contract,
    ctx.storage,
    'rateOracle',
    {
      op: 'rate_oracle',
      tnk_usd_e6: 2_000_000,
      source: 'coinbase-spot',
      ts: 1_000,
    },
    ctx.admin.publicKey,
    txNo
  );
  assert.equal(result.ok, true, result.message);
}

async function depositIntent(ctx, memoHash, txNo) {
  return await execute(
    ctx.contract,
    ctx.storage,
    'depositTnk',
    {
      op: 'deposit_tnk',
      memo_hash: memoHash,
    },
    ctx.user.publicKey,
    txNo
  );
}

async function confirmDeposit(ctx, memoHash, tnkE18, msbTxHash, txNo, epoch = 1) {
  return await execute(
    ctx.contract,
    ctx.storage,
    'tnkDeposit',
    {
      op: 'tnk_deposit',
      memo_hash: memoHash,
      tnk_e18: tnkE18,
      msb_tx_hash: msbTxHash,
      epoch,
      at: 1_500,
    },
    ctx.admin.publicKey,
    txNo
  );
}

test('MayhemContract binds TNK deposits to a user memo intent and credits balance', async () => {
  const ctx = await setupDepositContract();

  const noConsent = await depositIntent(ctx, 'memo-a', 2);
  assert.match(noConsent.message, /consent required/i);

  await consentUser(ctx, 3);
  await setRate(ctx, 4);

  const intent = await depositIntent(ctx, 'memo-a', 5);
  assert.deepEqual(intent, {
    ok: true,
    op: 'depositTnk',
    memo_hash: 'memo-a',
    user: ctx.user.publicKey,
  });
  assert.deepEqual((await ctx.storage.get('dep/pending/memo-a')).value, {
    memo_hash: 'memo-a',
    user: ctx.user.publicKey,
    status: 'pending',
    requested_at: makeTxKey(5),
  });

  const duplicateIntent = await depositIntent(ctx, 'memo-a', 6);
  assert.match(duplicateIntent.message, /already pending/i);

  const nonAdminConfirm = await execute(
    ctx.contract,
    ctx.storage,
    'tnkDeposit',
    {
      op: 'tnk_deposit',
      memo_hash: 'memo-a',
      tnk_e18: oneTnkE18,
      msb_tx_hash: 'a'.repeat(64),
      epoch: 1,
      at: 1_500,
    },
    ctx.outsider.publicKey,
    7
  );
  assert.match(nonAdminConfirm.message, /admin required/i);

  const missingIntent = await confirmDeposit(ctx, 'memo-missing', oneTnkE18, 'b'.repeat(64), 8);
  assert.match(missingIntent.message, /intent not found/i);

  const confirmed = await confirmDeposit(ctx, 'memo-a', oneTnkE18, 'c'.repeat(64), 9);
  assert.equal(confirmed.ok, true, confirmed.message);
  assert.equal(confirmed.op, 'tnkDeposit');
  assert.equal(confirmed.who, ctx.user.publicKey);
  assert.equal(confirmed.mu, 2_000_000);
  assert.equal(confirmed.epoch, 1);
  assert.equal(confirmed.deposit_root.length, 64);
  assert.equal(confirmed.rate_ts, 1_000);
  assert.equal(await ctx.storage.get('dep/pending/memo-a'), null);

  assert.deepEqual((await ctx.storage.get(`bal/${ctx.user.publicKey}`)).value, {
    user: ctx.user.publicKey,
    denom: 'mu_usd',
    mu: 2_000_000,
    updated_epoch: 0,
    updated_at: makeTxKey(9),
    last_deposit_rate_ts: 1_000,
  });
  assert.deepEqual((await ctx.storage.get('ev/dep/1')).value, {
    type: 'deposit_root',
    epoch: 1,
    merkle_root: confirmed.deposit_root,
    count: 1,
    mu_total: 2_000_000,
    ts: 1_500,
    updated_at: makeTxKey(9),
  });

  const replay = await confirmDeposit(ctx, 'memo-a', oneTnkE18, 'c'.repeat(64), 10);
  assert.match(replay.message, /intent not found/i);
});

test('MayhemContract deposit root accumulation is deterministic and root-only', async () => {
  const identities = {
    admin: await makeIdentity(),
    user: await makeIdentity(),
    outsider: await makeIdentity(),
  };
  const left = await setupDepositContract(identities);
  const right = await setupDepositContract(identities);

  let firstRoot = null;
  for (const ctx of [left, right]) {
    await consentUser(ctx, 2);
    await setRate(ctx, 3);

    const firstIntent = await depositIntent(ctx, 'memo-1', 4);
    assert.equal(firstIntent.ok, true, firstIntent.message);
    const first = await confirmDeposit(ctx, 'memo-1', oneTnkE18, '1'.repeat(64), 5);
    assert.equal(first.ok, true, first.message);
    if (!firstRoot) firstRoot = first.deposit_root;

    const secondIntent = await depositIntent(ctx, 'memo-2', 6);
    assert.equal(secondIntent.ok, true, secondIntent.message);
    const second = await confirmDeposit(ctx, 'memo-2', halfTnkE18, '2'.repeat(64), 7);
    assert.equal(second.ok, true, second.message);
    assert.notEqual(second.deposit_root, first.deposit_root);
  }

  assert.equal(left.storage.snapshotBytes(), right.storage.snapshotBytes());
  const root = (await left.storage.get('ev/dep/1')).value;
  assert.equal(root.count, 2);
  assert.equal(root.mu_total, 3_000_000);
  assert.equal(root.merkle_root.length, 64);
  assert.notEqual(root.merkle_root, firstRoot);

  const rootJson = JSON.stringify(root);
  assert.equal(rootJson.includes(identities.user.publicKey), false);
  assert.equal(rootJson.includes('1'.repeat(64)), false);
  assert.equal(rootJson.includes('2'.repeat(64)), false);
});

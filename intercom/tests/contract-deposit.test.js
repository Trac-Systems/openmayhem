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
      source: 'gate-spot',
      ts: 1_000,
    },
    ctx.admin.publicKey,
    txNo
  );
  assert.equal(result.ok, true, result.message);
}

async function depositIntent(ctx, memoHash, txNo, extra = {}) {
  const quoted = {
    treasury_address: 'testtrac1treasury',
    tnk_e18: oneTnkE18,
    quoted_mu: 2_000_000,
    rate_tnk_usd_e6: 2_000_000,
    rate_source: 'gate-spot',
    ...extra,
  };
  return await execute(
    ctx.contract,
    ctx.storage,
    'depositTnk',
    {
      op: 'deposit_tnk',
      memo_hash: memoHash,
      ...quoted,
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
    treasury_address: 'testtrac1treasury',
    tnk_e18: oneTnkE18,
    quoted_mu: 2_000_000,
    rate_tnk_usd_e6: 2_000_000,
    rate_source: 'gate-spot',
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

test('MayhemContract stores canonical TNK quote fields and enforces them', async () => {
  const ctx = await setupDepositContract();
  await consentUser(ctx, 2);
  await setRate(ctx, 3);

  const intent = await depositIntent(ctx, 'memo-quoted', 4, {
    treasury_address: 'testtrac1treasury',
    tnk_e18: oneTnkE18,
    quoted_mu: 2_000_000,
    rate_tnk_usd_e6: 2_000_000,
    rate_source: 'gate-spot',
  });
  assert.equal(intent.ok, true, intent.message);
  assert.deepEqual((await ctx.storage.get('dep/pending/memo-quoted')).value, {
    memo_hash: 'memo-quoted',
    user: ctx.user.publicKey,
    status: 'pending',
    requested_at: makeTxKey(4),
    treasury_address: 'testtrac1treasury',
    tnk_e18: oneTnkE18,
    quoted_mu: 2_000_000,
    rate_tnk_usd_e6: 2_000_000,
    rate_source: 'gate-spot',
  });

  const wrongAmount = await confirmDeposit(ctx, 'memo-quoted', halfTnkE18, 'd'.repeat(64), 5);
  assert.match(wrongAmount.message, /amount does not match/i);
  assert.notEqual(await ctx.storage.get('dep/pending/memo-quoted'), null);

  const confirmed = await confirmDeposit(ctx, 'memo-quoted', oneTnkE18, 'e'.repeat(64), 6);
  assert.equal(confirmed.ok, true, confirmed.message);
  assert.equal(confirmed.mu, 2_000_000);
  assert.equal(await ctx.storage.get('dep/pending/memo-quoted'), null);
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

    const secondIntent = await depositIntent(ctx, 'memo-2', 6, {
      tnk_e18: halfTnkE18,
      quoted_mu: 1_000_000,
    });
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

test('MayhemContract tapDeposit credits a finalized event exactly once under replay', async () => {
  const ctx = await setupDepositContract();
  const buyer = '0x1111111111111111111111111111111111111111';
  const pool = '0x2222222222222222222222222222222222222222';
  const ethTxHash = `0x${'a'.repeat(64)}`;
  const value = {
    op: 'tap_deposit',
    who: buyer,
    tap_wei: oneTnkE18,
    eth_tx_hash: ethTxHash,
    log_index: 0,
    block_number: 123,
    pool_address: pool,
    chain_id: 61_000,
    epoch: 1,
    at: 1_800,
  };

  const nonAdmin = await execute(
    ctx.contract,
    ctx.storage,
    'tapDeposit',
    value,
    ctx.outsider.publicKey,
    2
  );
  assert.match(nonAdmin.message, /admin required/i);

  const rate = await execute(
    ctx.contract,
    ctx.storage,
    'tapRateOracle',
    {
      op: 'tap_rate_oracle',
      tap_usd_e6: 2_000_000,
      source: 'uniswap-v2',
      ts: 1_000,
    },
    ctx.admin.publicKey,
    3
  );
  assert.equal(rate.ok, true, rate.message);

  const confirmed = await execute(
    ctx.contract,
    ctx.storage,
    'tapDeposit',
    value,
    ctx.admin.publicKey,
    4
  );
  assert.equal(confirmed.ok, true, confirmed.message);
  assert.equal(confirmed.op, 'tapDeposit');
  assert.equal(confirmed.duplicate, false);
  assert.equal(confirmed.who, buyer);
  assert.equal(confirmed.mu, 2_000_000);
  assert.equal(confirmed.eth_tx_hash, ethTxHash);
  assert.equal(confirmed.log_index, 0);
  assert.equal(confirmed.rate_ts, 1_000);
  assert.equal(confirmed.deposit_root.length, 64);

  const seenKey = `dep/tap/${ethTxHash}/0`;
  assert.deepEqual((await ctx.storage.get(seenKey)).value, {
    rail: 'tap',
    who: buyer,
    tap_wei: oneTnkE18,
    tap_usd_e6: 2_000_000,
    rate_ts: 1_000,
    rate_source: 'uniswap-v2',
    mu: 2_000_000,
    eth_tx_hash: ethTxHash,
    log_index: 0,
    block_number: 123,
    pool_address: pool,
    chain_id: 61_000,
    epoch: 1,
    at: 1_800,
    credited_at: makeTxKey(4),
    credited_by: ctx.admin.publicKey,
    credited_by_role: 'admin',
  });
  assert.deepEqual((await ctx.storage.get(`bal/${buyer}`)).value, {
    user: buyer,
    denom: 'mu_usd',
    mu: 2_000_000,
    updated_epoch: 1,
    updated_at: makeTxKey(4),
    last_deposit_rail: 'tap',
    last_deposit_rate_ts: 1_000,
    last_deposit_rate_source: 'uniswap-v2',
    last_deposit_tap_usd_e6: 2_000_000,
  });
  const root = (await ctx.storage.get('ev/dep/1')).value;
  assert.equal(root.type, 'deposit_root');
  assert.equal(root.count, 1);
  assert.equal(root.mu_total, 2_000_000);
  assert.equal(root.merkle_root, confirmed.deposit_root);
  assert.equal(JSON.stringify(root).includes(buyer), false);
  assert.equal(JSON.stringify(root).includes(ethTxHash), false);

  const replay = await execute(
    ctx.contract,
    ctx.storage,
    'tapDeposit',
    value,
    ctx.admin.publicKey,
    5
  );
  assert.equal(replay.ok, true, replay.message);
  assert.equal(replay.duplicate, true);
  assert.equal(replay.mu, 0);
  assert.equal(replay.credited_mu, 2_000_000);
  assert.equal(replay.deposit_root, confirmed.deposit_root);
  assert.deepEqual((await ctx.storage.get(`bal/${buyer}`)).value.mu, 2_000_000);
  assert.deepEqual((await ctx.storage.get('ev/dep/1')).value, root);
});

test('MayhemContract fiatDeposit credits mu_usd and folds root-only evidence', async () => {
  const ctx = await setupDepositContract();
  await consentUser(ctx, 2);

  const nonAdmin = await execute(
    ctx.contract,
    ctx.storage,
    'fiatDeposit',
    {
      op: 'fiat_deposit',
      rail: 'stripe',
      who: ctx.user.publicKey,
      mu: 2_500_000,
      ext_ref_hash: 'a'.repeat(64),
      fiat_currency: 'usd',
      fiat_amount_minor: 250,
      epoch: 1,
      at: 1_800,
    },
    ctx.outsider.publicKey,
    3
  );
  assert.match(nonAdmin.message, /admin required/i);

  const unsupported = await execute(
    ctx.contract,
    ctx.storage,
    'fiatDeposit',
    {
      op: 'fiat_deposit',
      rail: 'provider-rail',
      who: ctx.user.publicKey,
      mu: 2_500_000,
      ext_ref_hash: 'a'.repeat(64),
      fiat_currency: 'usd',
      fiat_amount_minor: 250,
      epoch: 1,
      at: 1_800,
    },
    ctx.admin.publicKey,
    4
  );
  assert.match(unsupported.message, /unsupported/i);

  const confirmed = await execute(
    ctx.contract,
    ctx.storage,
    'fiatDeposit',
    {
      op: 'fiat_deposit',
      rail: 'stripe',
      who: ctx.user.publicKey,
      mu: 2_500_000,
      ext_ref_hash: 'a'.repeat(64),
      fiat_currency: 'usd',
      fiat_amount_minor: 250,
      epoch: 1,
      at: 1_800,
    },
    ctx.admin.publicKey,
    5
  );
  assert.equal(confirmed.ok, true, confirmed.message);
  assert.equal(confirmed.op, 'fiatDeposit');
  assert.equal(confirmed.rail, 'stripe');
  assert.equal(confirmed.who, ctx.user.publicKey);
  assert.equal(confirmed.mu, 2_500_000);
  assert.equal(confirmed.fiat_currency, 'usd');
  assert.equal(confirmed.fiat_amount_minor, 250);
  assert.equal(confirmed.deposit_root.length, 64);

  assert.deepEqual((await ctx.storage.get(`bal/${ctx.user.publicKey}`)).value, {
    user: ctx.user.publicKey,
    denom: 'mu_usd',
    mu: 2_500_000,
    updated_epoch: 1,
    updated_at: makeTxKey(5),
    last_deposit_rail: 'stripe',
    last_deposit_fiat_currency: 'usd',
  });
  const root = (await ctx.storage.get('ev/dep/1')).value;
  assert.equal(root.type, 'deposit_root');
  assert.equal(root.count, 1);
  assert.equal(root.mu_total, 2_500_000);
  assert.equal(root.merkle_root, confirmed.deposit_root);
  assert.equal(JSON.stringify(root).includes(ctx.user.publicKey), false);
  assert.equal(JSON.stringify(root).includes('a'.repeat(64)), false);
});

test('MayhemContract fiatChargeback claws back remaining credits and freezes buyer', async () => {
  const ctx = await setupDepositContract();
  await consentUser(ctx, 2);

  const deposit = await execute(
    ctx.contract,
    ctx.storage,
    'fiatDeposit',
    {
      op: 'fiat_deposit',
      rail: 'stripe',
      who: ctx.user.publicKey,
      mu: 2_500_000,
      ext_ref_hash: 'b'.repeat(64),
      fiat_currency: 'eur',
      fiat_amount_minor: 250,
      epoch: 1,
      at: 1_800,
    },
    ctx.admin.publicKey,
    3
  );
  assert.equal(deposit.ok, true, deposit.message);

  const chargeback = await execute(
    ctx.contract,
    ctx.storage,
    'fiatChargeback',
    {
      op: 'fiat_chargeback',
      rail: 'stripe',
      who: ctx.user.publicKey,
      mu: 3_000_000,
      ext_ref_hash: 'b'.repeat(64),
      dispute_ref_hash: 'c'.repeat(64),
      fiat_currency: 'eur',
      fiat_amount_minor: 300,
      epoch: 2,
      at: 3_600,
    },
    ctx.admin.publicKey,
    4
  );
  assert.equal(chargeback.ok, true, chargeback.message);
  assert.equal(chargeback.op, 'fiatChargeback');
  assert.equal(chargeback.clawback_mu, 2_500_000);
  assert.equal(chargeback.network_absorbed_mu, 500_000);
  assert.equal(chargeback.fiat_currency, 'eur');
  assert.equal(chargeback.fiat_amount_minor, 300);
  assert.equal(chargeback.frozen, true);
  assert.equal(chargeback.deposit_root.length, 64);

  assert.deepEqual((await ctx.storage.get(`bal/${ctx.user.publicKey}`)).value, {
    user: ctx.user.publicKey,
    denom: 'mu_usd',
    mu: 0,
    updated_epoch: 2,
    updated_at: makeTxKey(4),
    last_deposit_rail: 'stripe',
    last_deposit_fiat_currency: 'eur',
    last_chargeback_rail: 'stripe',
    last_chargeback_fiat_currency: 'eur',
  });
  assert.deepEqual((await ctx.storage.get(`frozen/${ctx.user.publicKey}`)).value, {
    user: ctx.user.publicKey,
    status: 'frozen',
    reason: 'fiat_chargeback',
    rail: 'stripe',
    first_frozen_at: makeTxKey(4),
    first_frozen_at_seconds: 3_600,
    updated_at: makeTxKey(4),
    updated_at_seconds: 3_600,
    updated_epoch: 2,
    dispute_count: 1,
    disputed_mu_cum: 3_000_000,
    clawback_mu_cum: 2_500_000,
    network_absorbed_mu_cum: 500_000,
    last_ext_ref_hash: 'b'.repeat(64),
    last_dispute_ref_hash: 'c'.repeat(64),
    last_fiat_currency: 'eur',
  });

  const reversalRoot = (await ctx.storage.get('ev/dep/2')).value;
  assert.equal(reversalRoot.type, 'deposit_root');
  assert.equal(reversalRoot.reversed, true);
  assert.equal(reversalRoot.count, 1);
  assert.equal(reversalRoot.reversal_count, 1);
  assert.equal(reversalRoot.mu_total, 0);
  assert.equal(reversalRoot.reversed_mu_total, 3_000_000);
  assert.equal(reversalRoot.clawback_mu_total, 2_500_000);
  assert.equal(reversalRoot.network_absorbed_mu_total, 500_000);
  assert.equal(JSON.stringify(reversalRoot).includes(ctx.user.publicKey), false);
  assert.equal(JSON.stringify(reversalRoot).includes('c'.repeat(64)), false);

  const frozenDeposit = await depositIntent(ctx, 'memo-frozen', 5);
  assert.match(frozenDeposit.message, /frozen/i);
});

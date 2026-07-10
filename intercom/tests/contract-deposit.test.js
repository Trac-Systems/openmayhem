import assert from 'node:assert/strict';
import test from 'node:test';
import MayhemContract from '../contract/contract.js';
import {
  MemoryStorage,
  depositFeatureKey,
  execute,
  executeDepositFeature,
  executeRateFeature,
  executeTapAccountBindingFeature,
  makeEthereumIdentity,
  makeIdentity,
  makeTxKey,
  makeVerifier,
  signConsent,
  signDepositTnkIntent,
  signTapAccountBinding,
} from './helpers/contract.js';

const rulesHash = '5'.repeat(64);
const oneTnkE18 = '1000000000000000000';
const halfTnkE18 = '500000000000000000';
const TAP_DEPOSIT_EVENT_SIGNATURE = '0xe1fffcc4923d04b559f4d29a8bfc6cda04eb5b0d3c460751c2402c5c5cc9109c';
const TAP_DEPOSIT_WATCHER_ID = 'tap-deposit-watcher-v1';

function tapWatcherEvidence(overrides = {}) {
  const blockNumber = overrides.block_number ?? 123;
  const finalizedBlockNumber = overrides.finalized_block_number ?? blockNumber + 12;
  return {
    block_hash: `0x${'b'.repeat(64)}`,
    finalized_block_number: finalizedBlockNumber,
    confirmation_depth: finalizedBlockNumber - blockNumber,
    confirmation_policy: 'depth-12',
    event_signature: TAP_DEPOSIT_EVENT_SIGNATURE,
    watcher_id: TAP_DEPOSIT_WATCHER_ID,
    ...overrides,
  };
}

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

async function setRate(ctx) {
  const result = await executeRateFeature(
    ctx.contract,
    ctx.storage,
    {
      op: 'rate_oracle',
      tnk_usd_au: '2000000000000000000',
      source: 'gate-spot',
      ts: 1_000,
    },
    ctx.admin.publicKey,
  );
  assert.equal(result.ok, true, result.message);
}

async function depositIntent(ctx, memoHash, txNo, extra = {}) {
  const quoted = {
    treasury_address: 'testtrac1treasury',
    tnk_e18: oneTnkE18,
    quoted_au: '2000000000000000000',
    rate_tnk_usd_au: '2000000000000000000',
    rate_source: 'gate-spot',
    ...extra,
  };
  const intent = {
    op: 'deposit_tnk',
    memo_hash: memoHash,
    ...quoted,
  };
  const value = {
    op: 'deposit_tnk',
    sender: ctx.user.publicKey,
    intent,
    sig: signDepositTnkIntent(ctx.user.wallet, intent),
  };
  ctx.lastDepositIntentKey = await depositFeatureKey(ctx.contract, value);
  return await executeDepositFeature(ctx.contract, ctx.storage, value, ctx.user.publicKey);
}

async function confirmDeposit(ctx, memoHash, tnkE18, msbTxHash, txNo, epoch = 1, sender = ctx.admin.publicKey) {
  const value = {
    op: 'tnk_deposit',
    memo_hash: memoHash,
    tnk_e18: tnkE18,
    msb_tx_hash: msbTxHash,
    epoch,
    at: 1_500,
  };
  ctx.lastDepositConfirmKey = await depositFeatureKey(ctx.contract, value);
  return await executeDepositFeature(ctx.contract, ctx.storage, value, sender);
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
    requested_at: ctx.lastDepositIntentKey,
    treasury_address: 'testtrac1treasury',
    tnk_e18: oneTnkE18,
    quoted_au: '2000000000000000000',
    rate_tnk_usd_au: '2000000000000000000',
    rate_source: 'gate-spot',
  });

  const duplicateIntent = await depositIntent(ctx, 'memo-a', 6);
  assert.match(duplicateIntent.message, /already pending/i);

  const nonAdminConfirm = await confirmDeposit(
    ctx,
    'memo-a',
    oneTnkE18,
    'a'.repeat(64),
    7,
    1,
    ctx.outsider.publicKey
  );
  assert.match(nonAdminConfirm.message, /admin required/i);

  const missingIntent = await confirmDeposit(ctx, 'memo-missing', oneTnkE18, 'b'.repeat(64), 8);
  assert.match(missingIntent.message, /intent not found/i);

  const confirmed = await confirmDeposit(ctx, 'memo-a', oneTnkE18, 'c'.repeat(64), 9);
  const confirmedKey = ctx.lastDepositConfirmKey;
  assert.equal(confirmed.ok, true, confirmed.message);
  assert.equal(confirmed.op, 'tnkDeposit');
  assert.equal(confirmed.who, ctx.user.publicKey);
  assert.equal(confirmed.au, '2000000000000000000');
  assert.equal(confirmed.epoch, 1);
  assert.equal(confirmed.deposit_root.length, 64);
  assert.equal(confirmed.rate_ts, 1_000);
  assert.equal(await ctx.storage.get('dep/pending/memo-a'), null);

  assert.deepEqual((await ctx.storage.get(`bal/${ctx.user.publicKey}/tnk`)).value, {
    user: ctx.user.publicKey,
    rail: 'tnk',
    denom: 'au_usd',
    au: '2000000000000000000',
    updated_epoch: 0,
    updated_at: confirmedKey,
    last_deposit_rail: 'tnk',
    last_deposit_rate_ts: 1_000,
  });
  assert.deepEqual((await ctx.storage.get('ev/dep/1')).value, {
    type: 'deposit_root',
    epoch: 1,
    merkle_root: confirmed.deposit_root,
    count: 1,
    au_total: '2000000000000000000',
    ts: 1_500,
    updated_at: confirmedKey,
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
    quoted_au: '2000000000000000000',
    rate_tnk_usd_au: '2000000000000000000',
    rate_source: 'gate-spot',
  });
  assert.equal(intent.ok, true, intent.message);
  assert.deepEqual((await ctx.storage.get('dep/pending/memo-quoted')).value, {
    memo_hash: 'memo-quoted',
    user: ctx.user.publicKey,
    status: 'pending',
    requested_at: ctx.lastDepositIntentKey,
    treasury_address: 'testtrac1treasury',
    tnk_e18: oneTnkE18,
    quoted_au: '2000000000000000000',
    rate_tnk_usd_au: '2000000000000000000',
    rate_source: 'gate-spot',
  });

  const wrongAmount = await confirmDeposit(ctx, 'memo-quoted', halfTnkE18, 'd'.repeat(64), 5);
  assert.match(wrongAmount.message, /amount does not match/i);
  assert.notEqual(await ctx.storage.get('dep/pending/memo-quoted'), null);

  const confirmed = await confirmDeposit(ctx, 'memo-quoted', oneTnkE18, 'e'.repeat(64), 6);
  assert.equal(confirmed.ok, true, confirmed.message);
  assert.equal(confirmed.au, '2000000000000000000');
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
      quoted_au: '1000000000000000000',
    });
    assert.equal(secondIntent.ok, true, secondIntent.message);
    const second = await confirmDeposit(ctx, 'memo-2', halfTnkE18, '2'.repeat(64), 7);
    assert.equal(second.ok, true, second.message);
    assert.notEqual(second.deposit_root, first.deposit_root);
  }

  assert.equal(left.storage.snapshotBytes(), right.storage.snapshotBytes());
  const root = (await left.storage.get('ev/dep/1')).value;
  assert.equal(root.count, 2);
  assert.equal(root.au_total, '3000000000000000000');
  assert.equal(root.merkle_root.length, 64);
  assert.notEqual(root.merkle_root, firstRoot);

  const rootJson = JSON.stringify(root);
  assert.equal(rootJson.includes(identities.user.publicKey), false);
  assert.equal(rootJson.includes('1'.repeat(64)), false);
  assert.equal(rootJson.includes('2'.repeat(64)), false);
});

test('MayhemContract dual-signs TAP account bindings and claims pre-binding credit once', async () => {
  const ctx = await setupDepositContract();
  const ethereum = makeEthereumIdentity();
  const pool = '0x2222222222222222222222222222222222222222';
  await consentUser(ctx, 2);
  const unsigned = {
    op: 'tap_account_bind',
    user: ctx.user.publicKey,
    ethereum_address: ethereum.address,
    chain_id: 1,
    pool_address: pool,
  };
  const valid = signTapAccountBinding(ctx.user.wallet, ethereum, unsigned);

  const badUser = await executeTapAccountBindingFeature(
    ctx.contract,
    ctx.storage,
    { ...valid, user_sig: '0'.repeat(128) },
    ctx.admin.publicKey
  );
  assert.match(badUser.message, /user signature/i);
  const badEthereum = await executeTapAccountBindingFeature(
    ctx.contract,
    ctx.storage,
    { ...valid, ethereum_sig: `0x${'0'.repeat(130)}` },
    ctx.admin.publicKey
  );
  assert.match(badEthereum.message, /Ethereum signature/i);

  await ctx.storage.put(`bal/${ethereum.address}/tap`, {
    user: ethereum.address,
    rail: 'tap',
    denom: 'au_usd',
    au: '86018945004270602',
    updated_epoch: 1,
    updated_at: makeTxKey(4),
  });
  const bindingFeatureKey = await ctx.contract.tapAccountBindingFeatureKey(valid);
  const bound = await executeTapAccountBindingFeature(
    ctx.contract,
    ctx.storage,
    valid,
    ctx.admin.publicKey
  );
  assert.equal(bound.ok, true, bound.message);
  assert.equal(bound.claimed_au, '86018945004270602');
  assert.equal(bound.balance_au, '86018945004270602');
  assert.equal((await ctx.storage.get(`bal/${ethereum.address}/tap`)).value.au, '0');
  assert.equal(
    (await ctx.storage.get(`bal/${ethereum.address}/tap`)).value.updated_at,
    bindingFeatureKey
  );
  assert.equal(
    (await ctx.storage.get(`bal/${ctx.user.publicKey}/tap`)).value.au,
    '86018945004270602'
  );
  assert.equal(
    (await ctx.storage.get(`bal/${ctx.user.publicKey}/tap`)).value.updated_at,
    bindingFeatureKey
  );
  assert.equal(
    (await ctx.storage.get(`tap/account/1/${pool}/${ctx.user.publicKey}`)).value.bound_at,
    bindingFeatureKey
  );

  const repeated = await executeTapAccountBindingFeature(
    ctx.contract,
    ctx.storage,
    valid,
    ctx.admin.publicKey
  );
  assert.equal(repeated.ok, true, repeated.message);
  assert.equal(repeated.idempotent, true);
  assert.equal(repeated.claimed_au, '0');

  const other = await makeIdentity();
  const otherConsent = await execute(
    ctx.contract,
    ctx.storage,
    'consent',
    { op: 'consent', ver: 1, hash: rulesHash, sig: signConsent(other.wallet, 1, rulesHash) },
    other.publicKey,
    5
  );
  assert.equal(otherConsent.ok, true, otherConsent.message);
  const conflict = await executeTapAccountBindingFeature(
    ctx.contract,
    ctx.storage,
    signTapAccountBinding(other.wallet, ethereum, { ...unsigned, user: other.publicKey }),
    ctx.admin.publicKey
  );
  assert.match(conflict.message, /different Mayhem wallet/i);
});

test('MayhemContract tapDeposit credits a finalized event exactly once under replay', async () => {
  const ctx = await setupDepositContract();
  const ethereum = makeEthereumIdentity();
  const buyer = ethereum.address;
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
    ...tapWatcherEvidence(),
  };

  const nonAdmin = await executeDepositFeature(ctx.contract, ctx.storage, value, ctx.outsider.publicKey);
  assert.match(nonAdmin.message, /admin required/i);
  const missingEvidence = await executeDepositFeature(
    ctx.contract,
    ctx.storage,
    {
      op: 'tap_deposit',
      who: buyer,
      tap_wei: oneTnkE18,
      eth_tx_hash: `0x${'d'.repeat(64)}`,
      log_index: 1,
      block_number: 124,
      pool_address: pool,
      chain_id: 61_000,
      epoch: 1,
      at: 1_800,
    },
    ctx.admin.publicKey
  );
  assert.match(missingEvidence.message, /missing block_hash/i);
  const badEventSignature = await executeDepositFeature(
    ctx.contract,
    ctx.storage,
    { ...value, eth_tx_hash: `0x${'e'.repeat(64)}`, event_signature: `0x${'0'.repeat(64)}` },
    ctx.admin.publicKey
  );
  assert.match(badEventSignature.message, /event signature mismatch/i);
  const shallowConfirmation = await executeDepositFeature(
    ctx.contract,
    ctx.storage,
    {
      ...value,
      eth_tx_hash: `0x${'f'.repeat(64)}`,
      finalized_block_number: 124,
      confirmation_depth: 1,
      confirmation_policy: 'depth-1',
    },
    ctx.admin.publicKey
  );
  assert.match(shallowConfirmation.message, /confirmation depth below minimum/i);
  const mismatchedPolicy = await executeDepositFeature(
    ctx.contract,
    ctx.storage,
    {
      ...value,
      eth_tx_hash: `0x${'1'.repeat(64)}`,
      confirmation_policy: 'manual',
    },
    ctx.admin.publicKey
  );
  assert.match(mismatchedPolicy.message, /confirmation policy must match/i);
  const finalizedTag = await executeDepositFeature(
    ctx.contract,
    ctx.storage,
    {
      ...value,
      eth_tx_hash: `0x${'2'.repeat(64)}`,
      finalized_block_number: 123,
      confirmation_depth: 0,
      confirmation_policy: 'finalized-tag',
    },
    ctx.admin.publicKey
  );
  assert.match(finalizedTag.message, /Fresh TAP rate oracle required/i);

  const rate = await executeRateFeature(
    ctx.contract,
    ctx.storage,
    {
      op: 'tap_rate_oracle',
      tap_usd_au: '2000000000000000000',
      source: 'uniswap-v2',
      ts: 1_000,
    },
    ctx.admin.publicKey,
  );
  assert.equal(rate.ok, true, rate.message);

  const unbound = await executeDepositFeature(ctx.contract, ctx.storage, value, ctx.admin.publicKey);
  assert.match(unbound.message, /account binding required/i);
  await consentUser(ctx, 3);
  const bindingValue = signTapAccountBinding(ctx.user.wallet, ethereum, {
    op: 'tap_account_bind',
    user: ctx.user.publicKey,
    ethereum_address: buyer,
    chain_id: 61_000,
    pool_address: pool,
  });
  const binding = await executeTapAccountBindingFeature(
    ctx.contract,
    ctx.storage,
    bindingValue,
    ctx.admin.publicKey
  );
  assert.equal(binding.ok, true, binding.message);
  assert.equal(binding.user, ctx.user.publicKey);
  assert.equal(binding.ethereum_address, buyer);
  assert.equal(binding.claimed_au, '0');

  const confirmedKey = await depositFeatureKey(ctx.contract, value);
  const confirmed = await executeDepositFeature(ctx.contract, ctx.storage, value, ctx.admin.publicKey);
  assert.equal(confirmed.ok, true, confirmed.message);
  assert.equal(confirmed.op, 'tapDeposit');
  assert.equal(confirmed.duplicate, false);
  assert.equal(confirmed.who, ctx.user.publicKey);
  assert.equal(confirmed.ethereum_address, buyer);
  assert.equal(confirmed.au, '2000000000000000000');
  assert.equal(confirmed.eth_tx_hash, ethTxHash);
  assert.equal(confirmed.log_index, 0);
  assert.equal(confirmed.rate_ts, 1_000);
  assert.equal(confirmed.deposit_root.length, 64);

  const seenKey = `dep/tap/${ethTxHash}/0`;
  assert.deepEqual((await ctx.storage.get(seenKey)).value, {
    rail: 'tap',
    who: ctx.user.publicKey,
    ethereum_address: buyer,
    tap_wei: oneTnkE18,
    tap_usd_au: '2000000000000000000',
    rate_ts: 1_000,
    rate_source: 'uniswap-v2',
    au: '2000000000000000000',
    eth_tx_hash: ethTxHash,
    log_index: 0,
    block_number: 123,
    block_hash: `0x${'b'.repeat(64)}`,
    pool_address: pool,
    chain_id: 61_000,
    finalized_block_number: 135,
    confirmation_depth: 12,
    confirmation_policy: 'depth-12',
    event_signature: TAP_DEPOSIT_EVENT_SIGNATURE,
    watcher_id: TAP_DEPOSIT_WATCHER_ID,
    epoch: 1,
    at: 1_800,
    credited_at: confirmedKey,
    credited_by: ctx.admin.publicKey,
    credited_by_role: 'admin',
  });
  assert.deepEqual((await ctx.storage.get(`bal/${ctx.user.publicKey}/tap`)).value, {
    user: ctx.user.publicKey,
    rail: 'tap',
    denom: 'au_usd',
    au: '2000000000000000000',
    updated_epoch: 1,
    updated_at: confirmedKey,
    last_deposit_rail: 'tap',
    last_deposit_rate_ts: 1_000,
    last_deposit_rate_source: 'uniswap-v2',
    last_deposit_tap_usd_au: '2000000000000000000',
  });
  const root = (await ctx.storage.get('ev/dep/1')).value;
  assert.equal(root.type, 'deposit_root');
  assert.equal(root.count, 1);
  assert.equal(root.au_total, '2000000000000000000');
  assert.equal(root.merkle_root, confirmed.deposit_root);
  assert.equal(JSON.stringify(root).includes(buyer), false);
  assert.equal(root.merkle_root.includes(ethTxHash), false);

  const replay = await executeDepositFeature(ctx.contract, ctx.storage, value, ctx.admin.publicKey);
  assert.equal(replay.ok, true, replay.message);
  assert.equal(replay.duplicate, true);
  assert.equal(replay.au, '0');
  assert.equal(replay.credited_au, '2000000000000000000');
  assert.equal(replay.deposit_root, confirmed.deposit_root);
  assert.deepEqual(
    (await ctx.storage.get(`bal/${ctx.user.publicKey}/tap`)).value.au,
    '2000000000000000000'
  );
  assert.deepEqual((await ctx.storage.get('ev/dep/1')).value, root);
});

test('MayhemContract fiatDeposit credits au_usd and folds root-only evidence', async () => {
  const ctx = await setupDepositContract();
  await consentUser(ctx, 2);

  const fiatValue = {
    op: 'fiat_deposit',
    rail: 'stripe',
    who: ctx.user.publicKey,
    au: '2500000000000000000',
    ext_ref_hash: 'a'.repeat(64),
    fiat_currency: 'usd',
    fiat_amount_minor: 250,
    epoch: 1,
    at: 1_800,
  };
  const nonAdmin = await executeDepositFeature(ctx.contract, ctx.storage, fiatValue, ctx.outsider.publicKey);
  assert.match(nonAdmin.message, /admin required/i);

  const unsupported = await executeDepositFeature(
    ctx.contract,
    ctx.storage,
    { ...fiatValue, rail: 'provider-rail' },
    ctx.admin.publicKey
  );
  assert.match(unsupported.message, /unsupported/i);

  const confirmedKey = await depositFeatureKey(ctx.contract, fiatValue);
  const confirmed = await executeDepositFeature(ctx.contract, ctx.storage, fiatValue, ctx.admin.publicKey);
  assert.equal(confirmed.ok, true, confirmed.message);
  assert.equal(confirmed.op, 'fiatDeposit');
  assert.equal(confirmed.rail, 'fiat');
  assert.equal(confirmed.processor_rail, 'stripe');
  assert.equal(confirmed.who, ctx.user.publicKey);
  assert.equal(confirmed.au, '2500000000000000000');
  assert.equal(confirmed.fiat_currency, 'usd');
  assert.equal(confirmed.fiat_amount_minor, 250);
  assert.equal(confirmed.deposit_root.length, 64);

  assert.deepEqual((await ctx.storage.get(`bal/${ctx.user.publicKey}/fiat`)).value, {
    user: ctx.user.publicKey,
    rail: 'fiat',
    denom: 'au_usd',
    au: '2500000000000000000',
    updated_epoch: 1,
    updated_at: confirmedKey,
    last_deposit_rail: 'fiat',
    last_deposit_processor_rail: 'stripe',
    last_deposit_fiat_currency: 'usd',
  });
  const root = (await ctx.storage.get('ev/dep/1')).value;
  assert.equal(root.type, 'deposit_root');
  assert.equal(root.count, 1);
  assert.equal(root.au_total, '2500000000000000000');
  assert.equal(root.merkle_root, confirmed.deposit_root);
  assert.equal(JSON.stringify(root).includes(ctx.user.publicKey), false);
  assert.equal(root.merkle_root.includes('a'.repeat(64)), false);

  const seenKey = `dep/fiat/${fiatValue.ext_ref_hash}`;
  assert.deepEqual((await ctx.storage.get(seenKey)).value, {
    rail: 'fiat',
    processor_rail: 'stripe',
    who: ctx.user.publicKey,
    au: '2500000000000000000',
    ext_ref_hash: 'a'.repeat(64),
    fiat_currency: 'usd',
    fiat_amount_minor: 250,
    epoch: 1,
    at: 1_800,
    credited_at: confirmedKey,
    credited_by: ctx.admin.publicKey,
    credited_by_role: 'admin',
    chargeback_au_cum: '0',
    network_absorbed_au_cum: '0',
  });

  const replay = await executeDepositFeature(
    ctx.contract,
    ctx.storage,
    {
      ...fiatValue,
      who: ctx.outsider.publicKey,
      au: '3000000000000000000',
      fiat_amount_minor: 300,
      at: 1_900,
    },
    ctx.admin.publicKey
  );
  assert.equal(replay.ok, true, replay.message);
  assert.equal(replay.duplicate, true);
  assert.equal(replay.au, '0');
  assert.equal(replay.credited_au, '2500000000000000000');
  assert.equal(replay.deposit_root, confirmed.deposit_root);
  assert.deepEqual((await ctx.storage.get(`bal/${ctx.user.publicKey}/fiat`)).value.au, '2500000000000000000');
  assert.equal((await ctx.storage.get(`bal/${ctx.outsider.publicKey}/fiat`))?.value ?? null, null);
  assert.deepEqual((await ctx.storage.get('ev/dep/1')).value, root);
});

test('MayhemContract fiatDeposit also accepts a direct admin transaction', async () => {
  const ctx = await setupDepositContract();
  await consentUser(ctx, 2);

  const fiatValue = {
    op: 'fiat_deposit',
    rail: 'stripe',
    who: ctx.user.publicKey,
    au: '1000000000000000000',
    ext_ref_hash: 'd'.repeat(64),
    fiat_currency: 'eur',
    fiat_amount_minor: 100,
    epoch: 1,
    at: 2_400,
  };
  const nonAdmin = await execute(ctx.contract, ctx.storage, 'fiatDeposit', fiatValue, ctx.outsider.publicKey, 5);
  assert.match(nonAdmin.message, /admin required/i);

  const confirmed = await execute(ctx.contract, ctx.storage, 'fiatDeposit', fiatValue, ctx.admin.publicKey, 6);
  assert.equal(confirmed.ok, true, confirmed.message);
  assert.equal(confirmed.op, 'fiatDeposit');
  assert.equal(confirmed.rail, 'fiat');
  assert.equal(confirmed.processor_rail, 'stripe');
  assert.equal(confirmed.au, '1000000000000000000');
  assert.deepEqual((await ctx.storage.get(`bal/${ctx.user.publicKey}/fiat`)).value, {
    user: ctx.user.publicKey,
    rail: 'fiat',
    denom: 'au_usd',
    au: '1000000000000000000',
    updated_epoch: 1,
    updated_at: makeTxKey(6),
    last_deposit_rail: 'fiat',
    last_deposit_processor_rail: 'stripe',
    last_deposit_fiat_currency: 'eur',
  });
});

test('MayhemContract fiatChargeback claws back remaining credits and freezes buyer', async () => {
  const ctx = await setupDepositContract();
  await consentUser(ctx, 2);

  const deposit = await executeDepositFeature(
    ctx.contract,
    ctx.storage,
    {
      op: 'fiat_deposit',
      rail: 'stripe',
      who: ctx.user.publicKey,
      au: '2500000000000000000',
      ext_ref_hash: 'b'.repeat(64),
      fiat_currency: 'eur',
      fiat_amount_minor: 250,
      epoch: 1,
      at: 1_800,
    },
    ctx.admin.publicKey
  );
  assert.equal(deposit.ok, true, deposit.message);

  const chargebackValue = {
    op: 'fiat_chargeback',
    rail: 'stripe',
    who: ctx.user.publicKey,
    au: '2500000000000000000',
    ext_ref_hash: 'b'.repeat(64),
    dispute_ref_hash: 'c'.repeat(64),
    fiat_currency: 'eur',
    fiat_amount_minor: 250,
    epoch: 2,
    at: 3_600,
  };
  const nonAdmin = await executeDepositFeature(
    ctx.contract,
    ctx.storage,
    chargebackValue,
    ctx.outsider.publicKey
  );
  assert.match(nonAdmin.message, /admin required/i);

  const chargebackKey = await depositFeatureKey(ctx.contract, chargebackValue);
  const chargeback = await executeDepositFeature(
    ctx.contract,
    ctx.storage,
    chargebackValue,
    ctx.admin.publicKey
  );
  assert.equal(chargeback.ok, true, chargeback.message);
  assert.equal(chargeback.op, 'fiatChargeback');
  assert.equal(chargeback.rail, 'fiat');
  assert.equal(chargeback.processor_rail, 'stripe');
  assert.equal(chargeback.clawback_au, '2500000000000000000');
  assert.equal(chargeback.network_absorbed_au, '0');
  assert.equal(chargeback.fiat_currency, 'eur');
  assert.equal(chargeback.fiat_amount_minor, 250);
  assert.equal(chargeback.frozen, true);
  assert.equal(chargeback.deposit_root.length, 64);

  assert.deepEqual((await ctx.storage.get(`bal/${ctx.user.publicKey}/fiat`)).value, {
    user: ctx.user.publicKey,
    rail: 'fiat',
    denom: 'au_usd',
    au: '0',
    updated_epoch: 2,
    updated_at: chargebackKey,
    last_deposit_rail: 'fiat',
    last_deposit_processor_rail: 'stripe',
    last_deposit_fiat_currency: 'eur',
    last_chargeback_rail: 'fiat',
    last_chargeback_processor_rail: 'stripe',
    last_chargeback_fiat_currency: 'eur',
  });
  assert.deepEqual((await ctx.storage.get(`frozen/${ctx.user.publicKey}`)).value, {
    user: ctx.user.publicKey,
    status: 'frozen',
    reason: 'fiat_chargeback',
    rail: 'fiat',
    processor_rail: 'stripe',
    first_frozen_at: chargebackKey,
    first_frozen_at_seconds: 3_600,
    updated_at: chargebackKey,
    updated_at_seconds: 3_600,
    updated_epoch: 2,
    dispute_count: 1,
    disputed_au_cum: '2500000000000000000',
    clawback_au_cum: '2500000000000000000',
    network_absorbed_au_cum: '0',
    last_ext_ref_hash: 'b'.repeat(64),
    last_dispute_ref_hash: 'c'.repeat(64),
    last_fiat_currency: 'eur',
  });

  const reversalRoot = (await ctx.storage.get('ev/dep/2')).value;
  assert.equal(reversalRoot.type, 'deposit_root');
  assert.equal(reversalRoot.reversed, true);
  assert.equal(reversalRoot.count, 1);
  assert.equal(reversalRoot.reversal_count, 1);
  assert.equal(reversalRoot.au_total, '0');
  assert.equal(reversalRoot.reversed_au_total, '2500000000000000000');
  assert.equal(reversalRoot.clawback_au_total, '2500000000000000000');
  assert.equal(reversalRoot.network_absorbed_au_total, '0');
  assert.equal(JSON.stringify(reversalRoot).includes(ctx.user.publicKey), false);
  assert.equal(reversalRoot.merkle_root.includes('c'.repeat(64)), false);

  assert.deepEqual((await ctx.storage.get(`dep/fiat/${'b'.repeat(64)}/chargeback/${'c'.repeat(64)}`)).value, {
    rail: 'fiat',
    processor_rail: 'stripe',
    who: ctx.user.publicKey,
    au: '2500000000000000000',
    clawback_au: '2500000000000000000',
    network_absorbed_au: '0',
    ext_ref_hash: 'b'.repeat(64),
    dispute_ref_hash: 'c'.repeat(64),
    fiat_currency: 'eur',
    fiat_amount_minor: 250,
    epoch: 2,
    at: 3_600,
    credited_at: chargebackKey,
    credited_by: ctx.admin.publicKey,
    credited_by_role: 'admin',
  });
  assert.deepEqual((await ctx.storage.get(`dep/fiat/${'b'.repeat(64)}`)).value, {
    rail: 'fiat',
    processor_rail: 'stripe',
    who: ctx.user.publicKey,
    au: '2500000000000000000',
    ext_ref_hash: 'b'.repeat(64),
    fiat_currency: 'eur',
    fiat_amount_minor: 250,
    epoch: 1,
    at: 1_800,
    credited_at: await depositFeatureKey(ctx.contract, {
      op: 'fiat_deposit',
      rail: 'stripe',
      who: ctx.user.publicKey,
      au: '2500000000000000000',
      ext_ref_hash: 'b'.repeat(64),
      fiat_currency: 'eur',
      fiat_amount_minor: 250,
      epoch: 1,
      at: 1_800,
    }),
    credited_by: ctx.admin.publicKey,
    credited_by_role: 'admin',
    chargeback_au_cum: '2500000000000000000',
    network_absorbed_au_cum: '0',
    disputed_au_cum: '2500000000000000000',
    last_dispute_ref_hash: 'c'.repeat(64),
    last_chargeback_at: chargebackKey,
    last_chargeback_at_seconds: 3_600,
    last_chargeback_epoch: 2,
  });
  const chargebackReplay = await executeDepositFeature(
    ctx.contract,
    ctx.storage,
    {
      ...chargebackValue,
      at: 3_700,
    },
    ctx.admin.publicKey
  );
  assert.equal(chargebackReplay.ok, true, chargebackReplay.message);
  assert.equal(chargebackReplay.duplicate, true);
  assert.equal(chargebackReplay.au, '0');
  assert.equal(chargebackReplay.credited_clawback_au, '2500000000000000000');
  assert.deepEqual((await ctx.storage.get('ev/dep/2')).value, reversalRoot);

  const frozenDeposit = await depositIntent(ctx, 'memo-frozen', 5);
  assert.match(frozenDeposit.message, /frozen/i);
});

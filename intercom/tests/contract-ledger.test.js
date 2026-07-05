import assert from 'node:assert/strict';
import test from 'node:test';
import MayhemContract, { CONTRACT_VERSION } from '../contract/contract.js';
import MayhemProtocol from '../contract/protocol.js';
import {
  MemoryStorage,
  execute,
  executeEpochApplyFeature,
  executeFeature,
  executeSpendReservationFeature,
  epochApplyFeatureKey,
  makeIdentity,
  makeTxKey,
  makeVerifier,
  seedCurrentAdminPrice,
  signConsent,
  signSpendReservation,
  signSpendVoucher,
} from './helpers/contract.js';

const rulesHash = '7'.repeat(64);

const providerRegistration = {
  op: 'register_provider',
};

const seededBalance = (user, mu, rail = 'fiat') => ({
  user,
  rail,
  denom: 'mu_usd',
  mu,
  updated_epoch: 0,
  updated_at: null,
});

const paymentKeys = (storage) =>
  Array.from(storage.values.keys())
    .filter((key) => key.startsWith('bal/') || key.startsWith('earn/') || key.startsWith('fee/'))
    .sort();

const makeEpochApply = (epoch, user, provider, grossMu) => ({
  op: 'epoch_apply',
  epoch,
  at: epoch * 3600,
  debits: [{ rail: 'fiat', user, mu: grossMu }],
  earnings: [{ rail: 'fiat', provider, gross_mu: grossMu }],
});

const mapPaidOps = (commands) => {
  const protocol = new MayhemProtocol({}, {});
  return commands
    .map((command) => protocol.mapTxCommand(JSON.stringify(command)))
    .filter(Boolean);
};

async function setupLedgerContract(identities = null) {
  const admin = identities?.admin ?? await makeIdentity();
  const provider = identities?.provider ?? await makeIdentity();
  const provider2 = identities?.provider2 ?? await makeIdentity();
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
    {
      type: 'consent',
      value: {
        op: 'consent',
        ver: 1,
        hash: rulesHash,
        sig: signConsent(provider2.wallet, 1, rulesHash),
      },
      sender: provider2.publicKey,
      txNo: 4,
    },
    {
      type: 'registerProvider',
      value: providerRegistration,
      sender: provider2.publicKey,
      txNo: 5,
    },
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }

  await storage.put(`bal/${user.publicKey}/fiat`, seededBalance(user.publicKey, 1_000_000));
  return { admin, provider, provider2, user, outsider, storage, contract };
}

async function seedReservationServing(ctx, provider = ctx.provider) {
  const enclaveId = 'e1'.repeat(32);
  const modelId = 'test/model@4bit';
  await ctx.storage.put(`enclave/${enclaveId}`, {
    enclave_id: enclaveId,
    model_id: modelId,
    model_class: 'text-generation',
    backend: 'llama.cpp',
    artifact_root: 'a1'.repeat(32),
    artifact_root_kind: 'blake3_merkle_v1',
    artifact_source: 'huggingface://trac-network/test/model.gguf',
    manifest_hash: 'b1'.repeat(32),
    binary_hash: 'c1'.repeat(32),
    att_tier: 1,
    caps: { chat: true, tools: true, json: true, ctx: 8192, ctx_max: 8192 },
    status: 'active',
    created_by: ctx.admin.publicKey,
    created_by_role: 'admin',
    created_at: makeTxKey(10),
    updated_at: makeTxKey(10),
  });
  await ctx.storage.put(`serve/${provider.publicKey}/${enclaveId}`, {
    provider: provider.publicKey,
    enclave_id: enclaveId,
    model_id: modelId,
    status: 'active',
    joined_at: makeTxKey(11),
    updated_at: makeTxKey(11),
    via: 'feature',
  });
  await seedCurrentAdminPrice(ctx.storage, {
    enclaveId,
    modelId,
    admin: ctx.admin.publicKey,
    txNo: 12,
    ver: 1,
    inPer1kMu: 20,
    outPer1kMu: 60,
    minSessionMu: 100,
    effectiveAt: 0,
  });
  return { enclaveId, modelId };
}

function signedSpendReservation(ctx, { provider = ctx.provider, enclaveId, sessionId, maxSpendMu, epoch = 1 } = {}) {
  const voucherBody = {
    session_id: sessionId,
    rail: 'fiat',
    enclave_id: enclaveId,
    price_ver: 1,
    max_spend_mu: maxSpendMu,
    checkpoint_every: { tokens: 8192, ms: 30_000 },
  };
  const unsigned = {
    op: 'spend_reserve',
    contract_version: CONTRACT_VERSION,
    session_id: sessionId,
    epoch,
    at: epoch * 3_600,
    rail: 'fiat',
    user: ctx.user.publicKey,
    provider: provider.publicKey,
    enclave_id: enclaveId,
    price_ver: 1,
    rules_ver: 1,
    max_spend_mu: maxSpendMu,
    voucher: {
      ...voucherBody,
      user_sig: signSpendVoucher(ctx.user.wallet, voucherBody),
    },
    provider_sig: '',
  };
  return {
    ...unsigned,
    provider_sig: signSpendReservation(provider.wallet, unsigned),
  };
}

test('MayhemProtocol keeps epochApply off the paid tx route', () => {
  const protocol = new MayhemProtocol({}, {});
  const paidOps = [
    { op: 'epoch_commit', epoch: 1, at: 3_600, roots: {}, totals: {} },
    makeEpochApply(1, 'user-a', 'provider-a', 1_000),
  ]
    .map((command) => protocol.mapTxCommand(JSON.stringify(command)))
    .filter(Boolean);

  assert.deepEqual(paidOps.map((op) => op.type), ['epochCommit']);
});

test('MayhemProtocol keeps deposit evidence off the paid tx route', () => {
  const protocol = new MayhemProtocol({}, {});
  const paidOps = [
    { op: 'epoch_commit', epoch: 1, at: 3_600, roots: {}, totals: {} },
    { op: 'deposit_tnk', memo_hash: 'memo-1' },
    { op: 'deposit_tnk', memo_hash: 'memo-2' },
    {
      op: 'tnk_deposit',
      memo_hash: 'memo-1',
      tnk_e18: '1000000000000000000',
      msb_tx_hash: 'a'.repeat(64),
      epoch: 1,
      at: 3_600,
    },
    {
      op: 'tap_deposit',
      who: '0x1111111111111111111111111111111111111111',
      tap_wei: '1000000000000000000',
      eth_tx_hash: `0x${'b'.repeat(64)}`,
      log_index: 0,
      block_number: 123,
      pool_address: '0x2222222222222222222222222222222222222222',
      chain_id: 61_000,
      epoch: 1,
      at: 3_600,
    },
    {
      op: 'fiat_deposit',
      rail: 'stripe',
      who: 'user-a',
      mu: 1_000_000,
      ext_ref_hash: 'c'.repeat(64),
      fiat_currency: 'usd',
      fiat_amount_minor: 100,
      epoch: 1,
      at: 3_600,
    },
  ]
    .map((command) => protocol.mapTxCommand(JSON.stringify(command)))
    .filter(Boolean);

  assert.deepEqual(paidOps.map((op) => op.type), ['epochCommit']);
});

test('MayhemProtocol keeps spend reservations off the paid tx route', () => {
  const protocol = new MayhemProtocol({}, {});
  const paidOps = [
    { op: 'epoch_commit', epoch: 1, at: 3_600, roots: {}, totals: {} },
    {
      op: 'spend_reserve',
      contract_version: CONTRACT_VERSION,
      session_id: 'a'.repeat(64),
      epoch: 1,
      at: 3_600,
      rail: 'fiat',
      user: 'b'.repeat(64),
      provider: 'c'.repeat(64),
      enclave_id: 'd'.repeat(64),
      price_ver: 1,
      rules_ver: 1,
      max_spend_mu: 1000,
      voucher: {},
      provider_sig: 'e'.repeat(128),
    },
  ]
    .map((command) => protocol.mapTxCommand(JSON.stringify(command)))
    .filter(Boolean);

  assert.deepEqual(paidOps.map((op) => op.type), ['epochCommit']);
});

test('MayhemProtocol keeps payout claims off the paid tx route', () => {
  const protocol = new MayhemProtocol({}, {});
  const paidOps = [
    { op: 'epoch_commit', epoch: 1, at: 3_600, roots: {}, totals: {} },
    {
      op: 'payout_confirm',
      epoch: 1,
      who: 'provider-a',
      mu: 100,
      tnk_e18: '1000000000000000000',
      msb_tx_hash: 'a'.repeat(64),
      at: 3_600,
    },
    {
      op: 'tnk_settlement',
      epoch: 1,
      at: 3_600,
      rail: 'tnk',
      network: 'testnet1',
      treasury_from: 'testtrac1treasury',
      operator_to: 'testtrac1operator',
      epoch_apply_hash: 'a'.repeat(64),
      rate_tnk_usd_e6: 50_000,
      rate_source: 'gate-spot',
      rate_ts: 3_600,
      msb_tx_hash: 'b'.repeat(64),
      transfer_root: 'c'.repeat(64),
      provider_count: 0,
      provider_mu: 0,
      operator_fee_mu: 1,
      gross_mu: 1,
      tnk_e18: '20000000000000',
      outputs: [
        {
          role: 'operator_fee',
          to: 'testtrac1operator',
          mu: 1,
          tnk_e18: '20000000000000',
        },
      ],
    },
    {
      op: 'payout_confirm',
      epoch: 1,
      who: 'provider-b',
      mu: 200,
      rail: 'stripe',
      external_ref: 'tr_provider_b',
      fiat_currency: 'usd',
      fiat_amount_minor: 1,
      at: 3_601,
    },
  ]
    .map((command) => protocol.mapTxCommand(JSON.stringify(command)))
    .filter(Boolean);

  assert.deepEqual(paidOps.map((op) => op.type), ['epochCommit']);
});

test('MayhemProtocol keeps rate oracle updates off the paid tx route', () => {
  const commands = [
    { op: 'epoch_commit', epoch: 1, at: 3_600, roots: {}, totals: {} },
    { op: 'rate_oracle', tnk_usd_e6: 50_000, source: 'gate-spot', ts: 3_600 },
    { op: 'rate_oracle', tnk_usd_e6: 51_000, source: 'mexc-spot', ts: 5_400 },
    { op: 'tap_rate_oracle', tap_usd_e6: 50_000, source: 'uniswap-v2', ts: 3_600 },
    { op: 'tap_rate_oracle', tap_usd_e6: 52_000, source: 'config', ts: 5_400 },
  ];

  assert.deepEqual(mapPaidOps(commands).map((op) => op.type), ['epochCommit']);
});

test('MayhemProtocol steady-state sponsorship stays at one paid tx per active epoch', () => {
  const activeEpochs = 3;
  const commands = [];

  for (let epoch = 1; epoch <= activeEpochs; epoch += 1) {
    commands.push({ op: 'epoch_commit', epoch, at: epoch * 3_600, roots: {}, totals: {} });
    commands.push(makeEpochApply(epoch, `user-${epoch}`, `provider-${epoch}`, 1_000 + epoch));
    commands.push({ op: 'rate_oracle', tnk_usd_e6: 50_000 + epoch, source: 'gate-spot', ts: epoch * 3_600 });
    commands.push({ op: 'tap_rate_oracle', tap_usd_e6: 50_000 + epoch, source: 'uniswap-v2', ts: epoch * 3_600 });

    for (let i = 0; i < 4; i += 1) {
      commands.push({
        op: 'deposit_tnk',
        sender: `user-${epoch}-${i}`,
        intent: { memo_hash: `memo-${epoch}-${i}` },
        sig: 'sig',
      });
      commands.push({
        op: 'tnk_deposit',
        memo_hash: `memo-${epoch}-${i}`,
        tnk_e18: '1000000000000000000',
        msb_tx_hash: `${epoch}${i}`.padEnd(64, 'a'),
        epoch,
        at: epoch * 3_600 + i,
      });
      commands.push({
        op: 'tap_deposit',
        who: `0x${String(epoch).repeat(40).slice(0, 40)}`,
        tap_wei: '1000000000000000000',
        eth_tx_hash: `0x${`${epoch}${i}`.padEnd(64, 'b')}`,
        log_index: i,
        block_number: 100 + i,
        pool_address: '0x2222222222222222222222222222222222222222',
        chain_id: 61_000,
        epoch,
        at: epoch * 3_600 + i,
      });
      commands.push({
        op: 'fiat_deposit',
        rail: 'stripe',
        who: `user-${epoch}-${i}`,
        mu: 1_000_000,
        ext_ref_hash: `${epoch}${i}`.padEnd(64, 'c'),
        fiat_currency: 'usd',
        fiat_amount_minor: 100,
        epoch,
        at: epoch * 3_600 + i,
      });
      commands.push({
        op: 'payout_confirm',
        epoch,
        who: `provider-${epoch}-${i}`,
        mu: 100 + i,
        tnk_e18: '1000000000000000000',
        msb_tx_hash: `${epoch}${i}`.padEnd(64, 'd'),
        at: epoch * 3_600 + i,
      });
    }
  }

  const paidOps = mapPaidOps(commands);
  assert.deepEqual(paidOps.map((op) => op.type), Array(activeEpochs).fill('epochCommit'));
  assert.equal(paidOps.length, activeEpochs);

  const paidByEpoch = new Map();
  for (const op of paidOps) {
    paidByEpoch.set(op.value.epoch, (paidByEpoch.get(op.value.epoch) ?? 0) + 1);
  }
  for (let epoch = 1; epoch <= activeEpochs; epoch += 1) {
    assert.equal(paidByEpoch.get(epoch), 1, `epoch ${epoch} must have exactly one paid anchor`);
  }
});

test('MayhemContract spend reservation enforces active-epoch unreserved user balance', async () => {
  const ctx = await setupLedgerContract();
  const { enclaveId } = await seedReservationServing(ctx, ctx.provider);
  await seedReservationServing(ctx, ctx.provider2);

  const first = signedSpendReservation(ctx, {
    provider: ctx.provider,
    enclaveId,
    sessionId: 'a1'.repeat(32),
    maxSpendMu: 700_000,
  });
  const firstKey = await ctx.contract.spendReservationFeatureKey(first);
  const firstResult = await executeSpendReservationFeature(
    ctx.contract,
    ctx.storage,
    first,
    ctx.provider.publicKey
  );
  assert.equal(firstResult.ok, true, firstResult.message);
  assert.equal(firstResult.idempotent, false);
  assert.equal(firstResult.available_mu, 300_000);

  const hold = (await ctx.storage.get(`hold/fiat/${ctx.user.publicKey}/1`)).value;
  assert.equal(hold.user, ctx.user.publicKey);
  assert.equal(hold.rail, 'fiat');
  assert.equal(hold.epoch, 1);
  assert.equal(hold.reserved_mu, 700_000);
  assert.equal(hold.balance_mu_at_last_reserve, 1_000_000);
  assert.equal(hold.sessions.length, 1);
  assert.equal(hold.sessions[0].session_id, 'a1'.repeat(32));
  assert.equal(hold.sessions[0].provider, ctx.provider.publicKey);
  assert.equal(hold.sessions[0].feature_key, firstKey);
  assert.match(hold.sessions[0].voucher_hash, /^[0-9a-f]{64}$/);

  const replay = await executeSpendReservationFeature(
    ctx.contract,
    ctx.storage,
    first,
    ctx.provider.publicKey
  );
  assert.equal(replay.ok, true, replay.message);
  assert.equal(replay.idempotent, true);
  assert.equal(replay.reserved_mu, 700_000);

  const second = signedSpendReservation(ctx, {
    provider: ctx.provider2,
    enclaveId,
    sessionId: 'a2'.repeat(32),
    maxSpendMu: 400_000,
  });
  const secondResult = await executeSpendReservationFeature(
    ctx.contract,
    ctx.storage,
    second,
    ctx.provider2.publicKey
  );
  assert.match(secondResult.message, /Insufficient unreserved credit balance/);
  assert.equal((await ctx.storage.get(`hold/fiat/${ctx.user.publicKey}/1`)).value.reserved_mu, 700_000);
});

test('MayhemContract spend reservation moves to next epoch after epochApply', async () => {
  const ctx = await setupLedgerContract();
  const { enclaveId } = await seedReservationServing(ctx, ctx.provider);

  const first = signedSpendReservation(ctx, {
    provider: ctx.provider,
    enclaveId,
    sessionId: 'b1'.repeat(32),
    maxSpendMu: 900_000,
  });
  const firstResult = await executeSpendReservationFeature(
    ctx.contract,
    ctx.storage,
    first,
    ctx.provider.publicKey
  );
  assert.equal(firstResult.ok, true, firstResult.message);

  const applyValue = makeEpochApply(1, ctx.user.publicKey, ctx.provider.publicKey, 100_000);
  const applied = await executeEpochApplyFeature(ctx.contract, ctx.storage, applyValue, ctx.admin.publicKey);
  assert.equal(applied.ok, true, applied.message);
  assert.equal((await ctx.storage.get(`bal/${ctx.user.publicKey}/fiat`)).value.mu, 900_000);

  const nextEpoch = signedSpendReservation(ctx, {
    provider: ctx.provider,
    enclaveId,
    sessionId: 'b2'.repeat(32),
    maxSpendMu: 900_000,
    epoch: 2,
  });
  const nextResult = await executeSpendReservationFeature(
    ctx.contract,
    ctx.storage,
    nextEpoch,
    ctx.provider.publicKey
  );
  assert.equal(nextResult.ok, true, nextResult.message);
  assert.equal(nextResult.available_mu, 0);
});

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
  assert.match(nonAdmin.message, /unknown contract operation type|function not registered/i);

  const nonAdminFeature = await executeEpochApplyFeature(
    contract,
    storage,
    makeEpochApply(1, user.publicKey, provider.publicKey, 1_500),
    outsider.publicKey
  );
  assert.match(nonAdminFeature.message, /admin required/i);

  const mismatch = await executeEpochApplyFeature(
    contract,
    storage,
    {
      op: 'epoch_apply',
      epoch: 1,
      at: 3600,
      debits: [{ rail: 'fiat', user: user.publicKey, mu: 1_500 }],
      earnings: [{ rail: 'fiat', provider: provider.publicKey, gross_mu: 1_400 }],
    },
    admin.publicKey
  );
  assert.match(mismatch.message, /must equal/i);

  const firstApply = {
    op: 'epoch_apply',
    epoch: 1,
    at: 3600,
    debits: [
      { rail: 'fiat', user: user.publicKey, mu: 1_000 },
      { rail: 'fiat', user: user.publicKey, mu: 500 },
    ],
    earnings: [
      { rail: 'fiat', provider: provider.publicKey, gross_mu: 1_250 },
      { rail: 'fiat', provider: provider.publicKey, gross_mu: 250 },
    ],
  };
  const firstApplyKey = await epochApplyFeatureKey(contract, firstApply);
  const wrongKeySnapshot = storage.snapshotBytes();
  const wrongKey = await executeFeature(
    contract,
    storage,
    'mayhem_feature',
    `epoch/apply/1/${'0'.repeat(64)}`,
    firstApply,
    admin.publicKey
  );
  assert.equal(wrongKey, undefined);
  assert.equal(storage.snapshotBytes(), wrongKeySnapshot);

  const first = await executeEpochApplyFeature(
    contract,
    storage,
    firstApply,
    admin.publicKey
  );
  assert.deepEqual(first, {
    ok: true,
    op: 'epochApply',
    epoch: 1,
    idempotent: false,
    debited_mu: 1_500,
    earned_mu: 1_275,
    fee_mu: 225,
    rails: ['fiat'],
  });

  assert.deepEqual((await storage.get(`bal/${user.publicKey}/fiat`)).value, {
    user: user.publicKey,
    rail: 'fiat',
    denom: 'mu_usd',
    mu: 998_500,
    updated_epoch: 1,
    updated_at: firstApplyKey,
  });
  assert.deepEqual((await storage.get(`earn/fiat/${provider.publicKey}`)).value, {
    provider: provider.publicKey,
    rail: 'fiat',
    denom: 'mu_usd',
    total_mu: 1_275,
    held_mu: 1_275,
    paid_cum_mu: 0,
    holdbacks: [{ epoch: 1, mu: 1_275 }],
    updated_epoch: 1,
    updated_at: firstApplyKey,
    last_holdback_release_epoch: 1,
  });
  const feeAfterFirst = (await storage.get('fee/fiat/cum')).value;
  assert.equal(feeAfterFirst.denom, 'mu_usd');
  assert.equal(feeAfterFirst.cum_mu, 225);
  assert.equal(feeAfterFirst.swept_cum_mu, 0);
  assert.equal(feeAfterFirst.updated_epoch, 1);
  assert.equal(feeAfterFirst.updated_at, firstApplyKey);
  assert.equal(feeAfterFirst.last_fee_bps, 1_500);
  assert.equal(feeAfterFirst.last_apply_hash.length, 64);

  const snapshotBeforeReplay = storage.snapshotBytes();
  const replay = await executeEpochApplyFeature(
    contract,
    storage,
    firstApply,
    admin.publicKey
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

  const changedReplay = await executeEpochApplyFeature(
    contract,
    storage,
    makeEpochApply(1, user.publicKey, provider.publicKey, 2_000),
    admin.publicKey
  );
  assert.match(changedReplay.message, /monotonic/i);

  const gap = await executeEpochApplyFeature(
    contract,
    storage,
    makeEpochApply(3, user.publicKey, provider.publicKey, 1_000),
    admin.publicKey
  );
  assert.match(gap.message, /contiguous/i);

  const insufficientSnapshot = storage.snapshotBytes();
  const insufficient = await executeEpochApplyFeature(
    contract,
    storage,
    makeEpochApply(2, user.publicKey, provider.publicKey, 2_000_000),
    admin.publicKey
  );
  assert.match(insufficient.message, /insufficient credit balance/i);
  assert.equal(storage.snapshotBytes(), insufficientSnapshot);
});

test('MayhemContract epochApply is deterministic and payment key growth stays flat over 100 epochs', async () => {
  const identities = {
    admin: await makeIdentity(),
    provider: await makeIdentity(),
    provider2: await makeIdentity(),
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
      const result = await executeEpochApplyFeature(
        ctx.contract,
        ctx.storage,
        makeEpochApply(epoch, identities.user.publicKey, identities.provider.publicKey, grossMu),
        identities.admin.publicKey
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
    `bal/${identities.user.publicKey}/fiat`,
    `earn/fiat/${identities.provider.publicKey}`,
    'fee/fiat/cum',
  ].sort());

  const balance = (await left.storage.get(`bal/${identities.user.publicKey}/fiat`)).value;
  const earning = (await left.storage.get(`earn/fiat/${identities.provider.publicKey}`)).value;
  const fee = (await left.storage.get('fee/fiat/cum')).value;
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
    rail: 'fiat',
    user: `user-${i}`,
    mu: 1,
  }));
  const tooLarge = await executeEpochApplyFeature(
    contract,
    storage,
    {
      op: 'epoch_apply',
      epoch: 1,
      at: 3600,
      debits: tooManyDebits,
      earnings: [{ rail: 'fiat', provider: provider.publicKey, gross_mu: 501 }],
    },
    admin.publicKey
  );
  assert.match(tooLarge.message, /max_apply_batch/i);
  assert.equal(storage.snapshotBytes(), before);
});

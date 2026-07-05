import assert from 'node:assert/strict';
import test from 'node:test';
import MayhemContract from '../contract/contract.js';
import MayhemProtocol from '../contract/protocol.js';
import {
  MemoryStorage,
  execute,
  executeEpochApplyFeature,
  executeFeature,
  epochApplyFeatureKey,
  makeIdentity,
  makeVerifier,
  signConsent,
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

  await storage.put(`bal/${user.publicKey}/fiat`, seededBalance(user.publicKey, 1_000_000));
  return { admin, provider, user, outsider, storage, contract };
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

import assert from 'node:assert/strict';
import test from 'node:test';
import MayhemContract, { CONTRACT_VERSION } from '../contract/contract.js';
import MayhemProtocol from '../contract/protocol.js';
import {
  MemoryStorage,
  execute,
  executeEpochApplyFeature,
  executePreparedEpochApplyFeature,
  prepareEpochApplyFeature,
  executeFeature,
  epochApplyFeatureKey,
  makeIdentity,
  makeTxKey,
  makeVerifier,
  seedSpendHold,
  seedSpendHoldsForApply,
  signConsent,
} from './helpers/contract.js';

const rulesHash = '7'.repeat(64);
const TEST_STRIPE_PROCESSOR_REVISION = 'c'.repeat(64);

const providerRegistration = {
  op: 'register_provider',
};

const auString = (value) => String(value);

const seededBalance = (user, au, rail = 'fiat') => ({
  user,
  rail,
  denom: 'au_usd',
  au: auString(au),
  updated_epoch: 0,
  updated_at: null,
  ...(rail === 'tap' ? {
    chain_id: 61_000,
    pool_address: `0x${'2'.repeat(40)}`,
  } : {}),
});

const paymentKeys = (storage) =>
  Array.from(storage.values.keys())
    .filter((key) => (
      key.startsWith('bal/') ||
      key.startsWith('earn/') ||
      key.startsWith('fee/') ||
      key.startsWith('burn/')
    ))
    .sort();

const makeEpochApply = (epoch, user, provider, grossAu) => {
  const au = auString(grossAu);
  return {
    op: 'epoch_apply',
    epoch,
    at: epoch * 3600,
    debits: [{ rail: 'fiat', user, au }],
    earnings: [{ rail: 'fiat', provider, gross_au: au }],
  };
};

const mapPaidOps = (commands) => {
  const protocol = new MayhemProtocol({}, {});
  return commands
    .map((command) => protocol.mapTxCommand(JSON.stringify(command)))
    .filter(Boolean);
};

async function seedTargetedFiatPayout(storage, provider, revision, target) {
  const bindingKey = `payout/binding/fiat/${provider}/${revision}`;
  const verificationKey = `payout/stripe-verified/${provider}/${revision}`;
  await storage.put(bindingKey, {
    type: 'provider_payout_binding',
    provider,
    rail: 'fiat',
    revision,
    target,
    currency: 'usd',
    chain_id: null,
    stripe_processor_revision: TEST_STRIPE_PROCESSOR_REVISION,
    activation_epoch: 1,
    verified: true,
  });
  await storage.put(`payout/current/fiat/${provider}`, {
    provider,
    rail: 'fiat',
    current_revision: revision,
    pending_revision: null,
    pending_activation_epoch: null,
  });
  await storage.put(verificationKey, {
    type: 'stripe_payout_verification',
    revision,
    provider,
    target,
    processor_revision: TEST_STRIPE_PROCESSOR_REVISION,
    ready: true,
  });
  const verificationPointer = {
    provider,
    revision,
    record_key: verificationKey,
    target,
    processor_revision: TEST_STRIPE_PROCESSOR_REVISION,
    ready: true,
  };
  await storage.put(`payout/stripe-verified/current/${provider}`, verificationPointer);
  await storage.put(
    `payout/stripe-verified/target/${provider}/${target}`,
    verificationPointer
  );
}

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

  const payoutRevisions = new Map([
    [provider.publicKey, 'a'.repeat(64)],
    [provider2.publicKey, 'b'.repeat(64)],
  ]);
  await seedTargetedFiatPayout(
    storage,
    provider.publicKey,
    payoutRevisions.get(provider.publicKey),
    'acct_ledger_provider_1'
  );
  await seedTargetedFiatPayout(
    storage,
    provider2.publicKey,
    payoutRevisions.get(provider2.publicKey),
    'acct_ledger_provider_2'
  );
  await storage.put(`bal/${user.publicKey}/fiat`, seededBalance(user.publicKey, 1_000_000));
  return { admin, provider, provider2, user, outsider, storage, contract, payoutRevisions };
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

test('MayhemProtocol admits bounded canonical admin feature payloads without changing core defaults', () => {
  const protocol = new MayhemProtocol({}, {});
  assert.equal(protocol.featMaxBytes(), 64_000);
  assert.equal(protocol.txMaxBytes(), 64_000);
});

test('MayhemProtocol maps empty epoch seals to the paid admin tx route', () => {
  const protocol = new MayhemProtocol({}, {});
  const op = protocol.mapTxCommand(JSON.stringify({
    op: 'epoch_seal_empty',
    epoch: 1,
    at: 3_600,
    reason_hash: 'a'.repeat(64),
  }));

  assert.deepEqual(op, {
    type: 'epochSealEmpty',
    value: {
      op: 'epoch_seal_empty',
      epoch: 1,
      at: 3_600,
      reason_hash: 'a'.repeat(64),
    },
  });
});

test('MayhemProtocol keeps deposit evidence off the paid tx route', () => {
  const protocol = new MayhemProtocol({}, {});
  const paidOps = [
    { op: 'epoch_commit', epoch: 1, at: 3_600, roots: {}, totals: {} },
    { op: 'deposit_tnk', memo_hash: 'memo-1' },
    { op: 'deposit_tnk', memo_hash: 'memo-2' },
    {
      op: 'tap_account_bind',
      user: '11'.repeat(32),
      ethereum_address: `0x${'22'.repeat(20)}`,
      chain_id: 1,
      pool_address: `0x${'33'.repeat(20)}`,
      user_sig: '44'.repeat(64),
      ethereum_sig: `0x${'55'.repeat(65)}`,
    },
    {
      op: 'tnk_deposit',
      memo_hash: 'memo-1',
      msb_transfer: {
        schema_version: 1,
        network: 'testnet1',
        tx_hash: 'a'.repeat(64),
        confirmed_length: 10,
        observed_signed_length: 12,
        from: 'testtrac1sender',
        to: 'testtrac1treasury',
        amount_e18: '1000000000000000000',
      },
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
      au: '1000000',
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
      max_spend_au: '1000',
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
      au: '100',
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
      rate_tnk_usd_au: '50000000000000000',
      rate_source: 'gate-spot',
      rate_ts: 3_600,
      msb_transfers: [{
        schema_version: 1,
        network: 'testnet1',
        tx_hash: 'b'.repeat(64),
        confirmed_length: 10,
        observed_signed_length: 12,
        from: 'testtrac1treasury',
        to: 'testtrac1operator',
        amount_e18: '20000000000000',
      }],
      transfer_root: 'c'.repeat(64),
      provider_count: 0,
      provider_au: '0',
      operator_fee_au: '1',
      gross_au: '1',
      tnk_e18: '20000000000000',
      outputs: [
        {
          role: 'operator_fee',
          to: 'testtrac1operator',
          au: '1',
          tnk_e18: '20000000000000',
        },
      ],
    },
    {
      op: 'payout_confirm',
      epoch: 1,
      who: 'provider-b',
      au: '200',
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
    { op: 'rate_oracle', tnk_usd_au: '50000000000000000', source: 'gate-spot', ts: 3_600 },
    { op: 'rate_oracle', tnk_usd_au: '51000000000000000', source: 'mexc-spot', ts: 5_400 },
    { op: 'tap_rate_oracle', tap_usd_au: '50000000000000000', source: 'uniswap-v2-twap-median', ts: 3_600 },
    { op: 'tap_rate_oracle', tap_usd_au: '52000000000000000', source: 'config', ts: 5_400 },
  ];

  assert.deepEqual(mapPaidOps(commands).map((op) => op.type), ['epochCommit']);
});

test('MayhemProtocol steady-state sponsorship stays at one paid tx per active epoch', () => {
  const activeEpochs = 3;
  const commands = [];

  for (let epoch = 1; epoch <= activeEpochs; epoch += 1) {
    commands.push({ op: 'epoch_commit', epoch, at: epoch * 3_600, roots: {}, totals: {} });
    commands.push(makeEpochApply(epoch, `user-${epoch}`, `provider-${epoch}`, 1_000 + epoch));
    commands.push({ op: 'rate_oracle', tnk_usd_au: `${50_000n + BigInt(epoch)}000000000000`, source: 'gate-spot', ts: epoch * 3_600 });
    commands.push({ op: 'tap_rate_oracle', tap_usd_au: `${50_000n + BigInt(epoch)}000000000000`, source: 'uniswap-v2-twap-median', ts: epoch * 3_600 });

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
        msb_transfer: {
          schema_version: 1,
          network: 'testnet1',
          tx_hash: `${epoch}${i}`.padEnd(64, 'a'),
          confirmed_length: epoch * 100 + i + 1,
          observed_signed_length: epoch * 100 + 10,
          from: `testtrac1user${epoch}${i}`,
          to: 'testtrac1treasury',
          amount_e18: '1000000000000000000',
        },
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
        au: '1000000',
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
        au: '100' + i,
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

// Native spend invariants migrated to contract-receipt-settlement.test.js:
// signed targeted vouchers replace the retired aggregate reservation shape.
// The eight current cases cover balance across providers, speciality, non-text
// brackets, locked rates, scheduled tables, next epochs and receipt-bound caps
// both within one page and across pages. Keep aggregate API rejection there too.

test('MayhemContract epochApply mutates credit, earning, and fee state in place', async () => {
  const { admin, provider, user, outsider, storage, contract } = await setupLedgerContract();
  // Invalid trial vectors must not seed the successful epoch's canonical commit.
  const invalid = await setupLedgerContract({ admin, provider, user, outsider });

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
    invalid.contract,
    invalid.storage,
    makeEpochApply(1, user.publicKey, provider.publicKey, 1_500),
    outsider.publicKey
  );
  assert.match(nonAdminFeature.message, /admin required/i);

  const mismatch = await executeEpochApplyFeature(
    invalid.contract,
    invalid.storage,
    {
      op: 'epoch_apply',
      epoch: 1,
      at: 3600,
      debits: [{ rail: 'fiat', user: user.publicKey, au: '1500' }],
      earnings: [{ rail: 'fiat', provider: provider.publicKey, gross_au: '1400' }],
    },
    admin.publicKey
  );
  assert.match(mismatch.message, /must equal/i);

  const firstApply = {
    op: 'epoch_apply',
    epoch: 1,
    at: 3600,
    debits: [
      { rail: 'fiat', user: user.publicKey, au: '1000' },
      { rail: 'fiat', user: user.publicKey, au: '500' },
    ],
    earnings: [
      { rail: 'fiat', provider: provider.publicKey, gross_au: '1250' },
      { rail: 'fiat', provider: provider.publicKey, gross_au: '250' },
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

  await seedSpendHoldsForApply(storage, firstApply);
  const prepared = await prepareEpochApplyFeature(contract, storage, firstApply, admin.publicKey);
  const first = await executePreparedEpochApplyFeature(contract, storage, prepared, admin.publicKey);
  assert.deepEqual(first, {
    ok: true,
    op: 'epochApply',
    epoch: 1,
    idempotent: false,
    debited_au: '1500',
    earned_au: '1275',
    fee_au: '225',
    burn_au: '0',
    rails: ['fiat'],
  });

  assert.deepEqual((await storage.get(`bal/${user.publicKey}/fiat`)).value, {
    user: user.publicKey,
    rail: 'fiat',
    denom: 'au_usd',
    au: '998500',
    updated_epoch: 1,
    updated_at: firstApplyKey,
  });
  assert.deepEqual((await storage.get(`earn/fiat/${provider.publicKey}`)).value, {
    provider: provider.publicKey,
    rail: 'fiat',
    denom: 'au_usd',
    total_au: '1275',
    held_au: '1275',
    paid_cum_au: '0',
    holdbacks: [{ epoch: 1, au: '1275', locked_epochs: 168 }],
    updated_epoch: 1,
    updated_at: firstApplyKey,
    last_holdback_release_epoch: 1,
  });
  const feeAfterFirst = (await storage.get('fee/fiat/cum')).value;
  assert.equal(feeAfterFirst.denom, 'au_usd');
  assert.equal(feeAfterFirst.cum_au, '225');
  assert.equal(feeAfterFirst.swept_cum_au, '0');
  assert.equal(feeAfterFirst.updated_epoch, 1);
  assert.equal(feeAfterFirst.updated_at, firstApplyKey);
  assert.equal(feeAfterFirst.last_fee_bps, 1_500);
  assert.equal(feeAfterFirst.last_apply_hash.length, 64);

  const snapshotBeforeReplay = storage.snapshotBytes();
  const replay = await executePreparedEpochApplyFeature(
    contract,
    storage,
    prepared,
    admin.publicKey
  );
  assert.deepEqual(replay, {
    ok: true,
    op: 'epochApply',
    epoch: 1,
    idempotent: true,
    debited_au: '0',
    earned_au: '0',
    fee_au: '0',
    burn_au: '0',
  });
  assert.equal(storage.snapshotBytes(), snapshotBeforeReplay);

  const changedReplay = await executePreparedEpochApplyFeature(
    contract,
    storage,
    { ...prepared, value: { ...prepared.value, ...makeEpochApply(1, user.publicKey, provider.publicKey, 2_000), at: 7200 } },
    admin.publicKey
  );
  assert.match(changedReplay.message, /monotonic/i);
  assert.equal(storage.snapshotBytes(), snapshotBeforeReplay);

  const gap = await executeEpochApplyFeature(
    contract,
    storage,
    makeEpochApply(3, user.publicKey, provider.publicKey, 1_000),
    admin.publicKey
  );
  assert.match(gap.message, /contiguous/i);

  await seedSpendHold(storage, { user: user.publicKey, epoch: 2, au: '2000000' });
  const insufficientPrepared = await prepareEpochApplyFeature(contract, storage,
    makeEpochApply(2, user.publicKey, provider.publicKey, 2_000_000), admin.publicKey);
  const insufficientSnapshot = storage.snapshotBytes();
  const insufficient = await executePreparedEpochApplyFeature(
    contract,
    storage,
    insufficientPrepared,
    admin.publicKey
  );
  assert.match(insufficient.message, /insufficient credit balance/i);
  assert.equal(storage.snapshotBytes(), insufficientSnapshot);
});

test('MayhemContract graduated holdback steps down without shortening older buckets', async () => {
  const contract = new MayhemContract({}, {});
  const params = {
    holdback_epochs: 24,
    challenge_epochs: 6,
    new_provider_holdback_epochs: 168,
    probation_successful_sessions: 50,
  };

  assert.equal(
    contract.providerLockedEarningEpochs({ probation: { successful_sessions: 0 } }, params),
    168
  );
  assert.equal(
    contract.providerLockedEarningEpochs({ probation: { successful_sessions: 49 } }, params),
    168
  );
  assert.equal(
    contract.providerLockedEarningEpochs({ probation: { successful_sessions: 50 } }, params),
    24
  );

  const refreshed = contract.refreshEarningHoldback(
    {
      provider: 'provider-a',
      rail: 'fiat',
      denom: 'au_usd',
      total_au: '1700',
      held_au: '1700',
      paid_cum_au: '0',
      updated_epoch: 2,
      holdbacks: [
        { epoch: 1, au: '850', locked_epochs: 168 },
        { epoch: 2, au: '850', locked_epochs: 24 },
      ],
    },
    26,
    24
  );
  assert.equal(refreshed.held_au, '850');
  assert.deepEqual(refreshed.holdbacks, [{ epoch: 1, au: '850', locked_epochs: 168 }]);
  assert.equal(refreshed.last_holdback_release_epoch, 26);
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
  let expectedDebited = 0n;
  let expectedFee = 0n;
  const netEarningsByEpoch = [];

  let paymentKeysAfterFirst = null;
  for (let epoch = 1; epoch <= 100; epoch++) {
    const grossAu = 1_000 + (epoch % 7);
    const feeAu = (BigInt(grossAu) * 1_500n) / 10_000n;
    expectedDebited += BigInt(grossAu);
    expectedFee += feeAu;
    netEarningsByEpoch.push(BigInt(grossAu) - feeAu);
    for (const ctx of [left, right]) {
      const value = makeEpochApply(epoch, identities.user.publicKey, identities.provider.publicKey, grossAu);
      await seedSpendHoldsForApply(ctx.storage, value);
      const result = await executeEpochApplyFeature(
        ctx.contract,
        ctx.storage,
        value,
        identities.admin.publicKey
      );
      assert.equal(result.ok, true, result.message);
      assert.equal(result.epoch, epoch);
    }

    if (epoch === 1) {
      paymentKeysAfterFirst = paymentKeys(left.storage);
    }
  }

  assert.equal(left.storage.snapshotBytes(), right.storage.snapshotBytes());
  assert.deepEqual(paymentKeys(left.storage), paymentKeysAfterFirst);
  assert.deepEqual(paymentKeysAfterFirst, [
    `bal/${identities.user.publicKey}/fiat`,
    'burn/fiat/cum',
    `earn/fiat/${identities.provider.publicKey}`,
    'fee/fiat/cum',
  ].sort());

  const balance = (await left.storage.get(`bal/${identities.user.publicKey}/fiat`)).value;
  const earning = (await left.storage.get(`earn/fiat/${identities.provider.publicKey}`)).value;
  const fee = (await left.storage.get('fee/fiat/cum')).value;
  const expectedHeld = netEarningsByEpoch.slice(-168).reduce((sum, au) => sum + au, 0n);
  assert.equal(balance.au, (1_000_000n - expectedDebited).toString());
  assert.equal(earning.total_au, (expectedDebited - expectedFee).toString());
  assert.equal(earning.held_au, expectedHeld.toString());
  assert.equal(earning.paid_cum_au, '0');
  assert.equal(fee.cum_au, expectedFee.toString());
  assert.equal(fee.updated_epoch, 100);
});

test('MayhemContract epochApply replays codepoint-sorted varied keys deterministically', async () => {
  const identities = {
    admin: await makeIdentity(),
    provider: await makeIdentity(),
    provider2: await makeIdentity(),
    user: await makeIdentity(),
    outsider: await makeIdentity(),
  };
  const left = await setupLedgerContract(identities);
  const right = await setupLedgerContract(identities);
  const providers = ['ProviderA', 'providera'];
  const users = ['UserA', 'usera'];

  for (const ctx of [left, right]) {
    for (const provider of providers) {
      await ctx.storage.put(`prov/${provider}`, {
        provider,
        status: 'active',
        accepted_rails: ['fiat'],
      });
    }
    await ctx.storage.put(`bal/${users[0]}/fiat`, seededBalance(users[0], 500));
    await ctx.storage.put(`bal/${users[1]}/fiat`, seededBalance(users[1], 700));
  }

  const leftApply = {
    op: 'epoch_apply',
    epoch: 1,
    at: 3600,
    debits: [
      { rail: 'fiat', user: users[1], au: '700' },
      { rail: 'fiat', user: users[0], au: '500' },
    ],
    earnings: [
      { rail: 'fiat', provider: providers[1], gross_au: '700' },
      { rail: 'fiat', provider: providers[0], gross_au: '500' },
    ],
  };
  const rightApply = {
    ...leftApply,
    debits: [...leftApply.debits].reverse(),
    earnings: [...leftApply.earnings].reverse(),
  };

  // Ordering is varied over the SAME receipt identities and epoch commitment.
  // Independently synthesizing receipts from each input order would create two
  // economically different canonical epochs, whose hashes must differ.
  const canonical = await prepareEpochApplyFeature(left.contract, left.storage, leftApply, identities.admin.publicKey);
  await right.storage.put(right.contract.receiptEpochIndexKey(1), canonical.value.receipt_index);
  await right.storage.put('epoch/commit/1', (await left.storage.get('epoch/commit/1')).value);
  for (const [ctx, value] of [[left, leftApply], [right, rightApply]]) {
    await seedSpendHoldsForApply(ctx.storage, value);
    const input = { ...canonical.value, debits: value.debits, earnings: value.earnings };
    const result = await executePreparedEpochApplyFeature(ctx.contract, ctx.storage, {
      key: await epochApplyFeatureKey(ctx.contract, input), value: input, allocations: canonical.allocations,
    }, identities.admin.publicKey);
    assert.equal(result.ok, true, result.message);
    assert.equal(result.fee_au, '180');
    assert.equal(result.earned_au, '1020');
  }

  const stripTxStamps = (record) => {
    const value = { ...record };
    delete value.updated_at;
    return value;
  };
  assert.equal(
    (await left.storage.get('epoch/apply/state')).value.last_apply_hash,
    (await right.storage.get('epoch/apply/state')).value.last_apply_hash
  );
  for (const key of [
    `bal/${users[0]}/fiat`,
    `bal/${users[1]}/fiat`,
    `earn/fiat/${providers[0]}`,
    `earn/fiat/${providers[1]}`,
    'fee/fiat/cum',
  ]) {
    assert.deepEqual(stripTxStamps((await left.storage.get(key)).value), stripTxStamps((await right.storage.get(key)).value));
  }
});

test('MayhemContract epochApply computes large fee bps with exact BigInt math', async () => {
  const { admin, provider, user, storage, contract } = await setupLedgerContract();
  const grossAu = '2000000000000000000000000';
  await storage.put(`bal/${user.publicKey}/fiat`, seededBalance(user.publicKey, grossAu));
  await seedSpendHoldsForApply(storage, makeEpochApply(1, user.publicKey, provider.publicKey, grossAu));

  const result = await executeEpochApplyFeature(
    contract,
    storage,
    makeEpochApply(1, user.publicKey, provider.publicKey, grossAu),
    admin.publicKey
  );
  assert.equal(result.ok, true, result.message);

  const expectedFee = (BigInt(grossAu) * 1_500n) / 10_000n;
  const expectedProvider = BigInt(grossAu) - expectedFee;
  assert.equal(result.fee_au, expectedFee.toString());
  assert.equal(result.earned_au, expectedProvider.toString());
  assert.equal((await storage.get(`bal/${user.publicKey}/fiat`)).value.au, '0');
  assert.equal((await storage.get(`earn/fiat/${provider.publicKey}`)).value.total_au, expectedProvider.toString());
  assert.equal((await storage.get('fee/fiat/cum')).value.cum_au, expectedFee.toString());
});

test('MayhemContract applies TAP 75/15/10 without burning fiat or TNK', async () => {
  const { admin, provider, user, storage, contract } = await setupLedgerContract();
  const rails = ['fiat', 'tap', 'tnk'];
  const setRails = await execute(
    contract,
    storage,
    'setProviderRails',
    { op: 'set_provider_rails', rails },
    provider.publicKey,
    6
  );
  assert.equal(setRails.ok, true, setRails.message);
  for (const rail of rails) {
    await storage.put(`bal/${user.publicKey}/${rail}`, seededBalance(user.publicKey, 10_000, rail));
  }
  const apply = {
    op: 'epoch_apply',
    epoch: 1,
    at: 3_600,
    debits: rails.map((rail) => ({ rail, user: user.publicKey, au: '10000' })),
    earnings: rails.map((rail) => ({ rail, provider: provider.publicKey, gross_au: '10000' })),
  };
  await seedSpendHoldsForApply(storage, apply);

  const result = await executeEpochApplyFeature(contract, storage, apply, admin.publicKey);
  assert.deepEqual(result, {
    ok: true,
    op: 'epochApply',
    epoch: 1,
    idempotent: false,
    debited_au: '30000',
    earned_au: '24500',
    fee_au: '4500',
    burn_au: '1000',
    rails,
  });
  assert.equal((await storage.get(`earn/fiat/${provider.publicKey}`)).value.total_au, '8500');
  assert.equal((await storage.get(`earn/tap/${provider.publicKey}`)).value.total_au, '7500');
  assert.equal((await storage.get(`earn/tnk/${provider.publicKey}`)).value.total_au, '8500');
  assert.equal((await storage.get('fee/fiat/cum')).value.cum_au, '1500');
  assert.equal((await storage.get('fee/tap/cum')).value.cum_au, '1500');
  assert.equal((await storage.get('fee/tnk/cum')).value.cum_au, '1500');
  assert.equal((await storage.get('burn/fiat/cum')).value.cum_au, '0');
  assert.equal((await storage.get('burn/tap/cum')).value.cum_au, '1000');
  assert.equal((await storage.get('burn/tnk/cum')).value.cum_au, '0');
});

test('MayhemContract au helpers reject numeric money inputs', async () => {
  const { contract } = await setupLedgerContract();
  assert.match(contract.safeAddAu(Number.MAX_SAFE_INTEGER, '1').message, /canonical decimal string/i);
  assert.match(contract.safeMulDivAu(Number.MAX_SAFE_INTEGER, 10_000, 1).message, /canonical decimal string/i);
  assert.equal(contract.safeAddAu('9007199254740993', '1'), '9007199254740994');
});

test('MayhemContract epochApply enforces max_apply_batch before writing', async () => {
  const { admin, provider, storage, contract } = await setupLedgerContract();
  const tooManyDebits = Array.from({ length: 2_001 }, (_, i) => ({
    rail: 'fiat',
    user: `user-${i}`,
    au: '1',
  }));
  const prepared = await prepareEpochApplyFeature(
    contract,
    storage,
    {
      op: 'epoch_apply',
      epoch: 1,
      at: 3600,
      debits: tooManyDebits,
      earnings: [{ rail: 'fiat', provider: provider.publicKey, gross_au: '2001' }],
    },
    admin.publicKey
  );
  const before = storage.snapshotBytes();
  const tooLarge = await executePreparedEpochApplyFeature(contract, storage, prepared, admin.publicKey);
  assert.match(tooLarge.message, /max_apply_batch/i);
  assert.equal(storage.snapshotBytes(), before);
});

test('MayhemContract epochApply uses the active admin max_apply_batch param', async () => {
  const { admin, provider, storage, contract } = await setupLedgerContract();
  const tuned = await execute(
    contract,
    storage,
    'setParams',
    {
      op: 'set_params',
      submitted_at: 0,
      effective_at: 86_400,
      values: { max_apply_batch: 3 },
    },
    admin.publicKey,
    4
  );
  assert.equal(tuned.ok, true, tuned.message);

  const prepared = await prepareEpochApplyFeature(
    contract,
    storage,
    {
      op: 'epoch_apply',
      epoch: 1,
      at: 86_400,
      debits: [
        { rail: 'fiat', user: 'user-a', au: '1' },
        { rail: 'fiat', user: 'user-b', au: '1' },
        { rail: 'fiat', user: 'user-c', au: '1' },
      ],
      earnings: [{ rail: 'fiat', provider: provider.publicKey, gross_au: '3' }],
    },
    admin.publicKey
  );
  const before = storage.snapshotBytes();
  const tunedTooLarge = await executePreparedEpochApplyFeature(contract, storage, prepared, admin.publicKey);
  assert.match(tunedTooLarge.message, /max_apply_batch/i);
  assert.equal(storage.snapshotBytes(), before);
});

test('MayhemContract epochApply accepts admin-raised max_apply_batch above default schema size', async () => {
  const { admin, provider, user, storage, contract } = await setupLedgerContract();
  await storage.put(`bal/${user.publicKey}/fiat`, seededBalance(user.publicKey, 10_000));
  const raised = await execute(
    contract,
    storage,
    'setParams',
    {
      op: 'set_params',
      submitted_at: 0,
      effective_at: 86_400,
      // Debits, earnings and canonical allocations all count toward this bound.
      values: { max_apply_batch: 5_502 },
    },
    admin.publicKey,
    4
  );
  assert.equal(raised.ok, true, raised.message);

  const manyDebits = Array.from({ length: 5_500 }, () => ({
    rail: 'fiat',
    user: user.publicKey,
    au: '1',
  }));
  await seedSpendHold(storage, { user: user.publicKey, epoch: 1, au: '5500' });
  const applied = await executeEpochApplyFeature(
    contract,
    storage,
    {
      op: 'epoch_apply',
      epoch: 1,
      at: 86_400,
      debits: manyDebits,
      earnings: [{ rail: 'fiat', provider: provider.publicKey, gross_au: '5500' }],
    },
    admin.publicKey
  );
  assert.equal(applied.ok, true, applied.message);
  assert.equal(applied.debited_au, '5500');
  assert.equal(applied.earned_au, '4675');
  assert.equal(applied.fee_au, '825');
});

test('MayhemContract epochApply uses the active admin max_market_usage_entries param', async () => {
  const { admin, provider, user, storage, contract } = await setupLedgerContract();
  const tuned = await execute(
    contract,
    storage,
    'setParams',
    {
      op: 'set_params',
      submitted_at: 0,
      effective_at: 86_400,
      values: { max_market_usage_entries: 1 },
    },
    admin.publicKey,
    4
  );
  assert.equal(tuned.ok, true, tuned.message);

  const prepared = await prepareEpochApplyFeature(
    contract,
    storage,
    {
      op: 'epoch_apply',
      epoch: 1,
      at: 86_400,
      debits: [{ rail: 'fiat', user: user.publicKey, au: '2' }],
      earnings: [{ rail: 'fiat', provider: provider.publicKey, gross_au: '2' }],
      market_usage: [{}, {}],
    },
    admin.publicKey
  );
  const before = storage.snapshotBytes();
  const tooManyMarketEntries = await executePreparedEpochApplyFeature(contract, storage, prepared, admin.publicKey);
  assert.match(tooManyMarketEntries.message, /max_market_usage_entries/i);
  assert.equal(storage.snapshotBytes(), before);
});

test('MayhemContract epochApply paginates settlement entries across free pages', async () => {
  const { admin, provider, user, storage, contract } = await setupLedgerContract();
  await storage.put(`bal/${user.publicKey}/fiat`, seededBalance(user.publicKey, 10_000));
  // Leave two entries for the earning and its canonical allocation (2,000 total).
  const firstDebits = Array.from({ length: 1_998 }, () => ({
    rail: 'fiat',
    user: user.publicKey,
    au: '1',
  }));
  const secondDebits = Array.from({ length: 502 }, () => ({
    rail: 'fiat',
    user: user.publicKey,
    au: '1',
  }));
  await seedSpendHold(storage, { user: user.publicKey, epoch: 1, au: '2500' });
  // Prepare the complete index/commit once. Each page consumes a disjoint part
  // of that same canonical snapshot; a page-local index would falsely claim
  // that page zero has already consumed every receipt.
  const prepared = await prepareEpochApplyFeature(contract, storage, {
    op: 'epoch_apply', epoch: 1, at: 3600,
    debits: [...firstDebits, ...secondDebits],
    earnings: [
      { rail: 'fiat', provider: provider.publicKey, gross_au: '1998' },
      { rail: 'fiat', provider: provider.publicKey, gross_au: '502' },
    ],
  }, admin.publicKey);
  assert.equal(prepared.value.receipt_index.count, 2);
  const pageInput = async (page, debits) => {
    const value = { ...prepared.value, page, last_page: page === 1,
      debits, earnings: [prepared.value.earnings[page]] };
    const allocations = [prepared.allocations[page]];
    assert.ok(debits.length + value.earnings.length + allocations.length <= 2000);
    return { key: await epochApplyFeatureKey(contract, value), value, allocations };
  };

  const firstPage = await executePreparedEpochApplyFeature(
    contract,
    storage,
    await pageInput(0, firstDebits),
    admin.publicKey
  );
  assert.equal(firstPage.ok, true, firstPage.message);
  assert.equal(firstPage.page, 0);
  assert.equal(firstPage.last_page, false);
  assert.equal(firstPage.debited_au, '1998');
  let applyState = (await storage.get('epoch/apply/state')).value;
  assert.equal(applyState.updated_epoch, 0);
  assert.equal(applyState.pending_epoch, 1);
  assert.equal(applyState.pending_next_page, 1);

  const secondPage = await executePreparedEpochApplyFeature(
    contract,
    storage,
    await pageInput(1, secondDebits),
    admin.publicKey
  );
  assert.equal(secondPage.ok, true, secondPage.message);
  assert.equal(secondPage.page, 1);
  assert.equal(secondPage.last_page, true);
  assert.equal(secondPage.debited_au, '502');
  applyState = (await storage.get('epoch/apply/state')).value;
  assert.equal(applyState.updated_epoch, 1);
  assert.equal(applyState.pending_epoch, null);
  assert.equal(applyState.pending_next_page, 0);

  assert.equal((await storage.get(`bal/${user.publicKey}/fiat`)).value.au, '7500');
  assert.equal((await storage.get(`earn/fiat/${provider.publicKey}`)).value.total_au, '2126');
  assert.equal((await storage.get('fee/fiat/cum')).value.cum_au, '374');
});

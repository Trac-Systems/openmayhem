import assert from 'node:assert/strict';
import test from 'node:test';
import MayhemContract from '../contract/contract.js';
import {
  MemoryStorage,
  execute,
  executeEpochApplyFeature,
  executeFeature,
  executeFiatSettlementFeature,
  fiatSettlementFeatureKey,
  makeIdentity,
  makeVerifier,
  seedSpendHoldsForApply,
  signConsent,
} from './helpers/contract.js';

const rulesHash = 'a'.repeat(64);
const CENT_AU = 10_000_000_000_000_000n;
const au = (value) => value.toString();
const cents = (value) => value.toString();

test('MayhemContract fiat dust feature key matches the Rust terminal fixture', async () => {
  const identity = await makeIdentity();
  const contract = new MayhemContract({ peer: { wallet: makeVerifier(identity.wallet) } }, {});
  assert.equal(
    await contract.fiatDustSweepFeatureKey({
      op: 'fiat_dust_sweep',
      provider: '11'.repeat(32),
      epoch: 7,
      at: 25_200,
    }),
    `settle/fiat-dust/${'11'.repeat(32)}/7/c18df170b5420344c89886d3aeef42f3f9064e29b92c66da4e503245f076f78d`
  );
});

async function setupFiatSettlementContract({
  providerSpecs = [
    { target: 'acct_provider_one', currency: 'eur', gross_au: au(200n * CENT_AU) },
    { target: 'acct_provider_two', currency: 'usd', gross_au: au(100n * CENT_AU) },
  ],
  payoutProviderIndexes = null,
  paramOverrides = {},
} = {}) {
  const admin = await makeIdentity();
  const providers = [];
  for (const spec of providerSpecs) {
    providers.push({ identity: await makeIdentity(), ...spec });
  }
  const user = await makeIdentity();
  const outsider = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const contract = new MayhemContract(
    { peer: { wallet: makeVerifier(providers[0].identity.wallet) } },
    {}
  );

  const bootstrap = [
    {
      type: 'setRules',
      value: { op: 'set_rules', ver: 1, hash: rulesHash },
      sender: admin.publicKey,
      txNo: 1,
    },
    {
      type: 'setParams',
      value: {
        op: 'set_params',
        submitted_at: 0,
        effective_at: 86_400,
        values: {
          holdback_epochs: 0,
          new_provider_holdback_epochs: 0,
          challenge_epochs: 0,
          ...paramOverrides,
        },
      },
      sender: admin.publicKey,
      txNo: 2,
    },
  ];
  for (const op of bootstrap) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }

  const payoutIndexes = payoutProviderIndexes ?? providers.map((_, index) => index);
  let txNo = 3;
  for (const [index, provider] of providers.entries()) {
    const ops = [
      {
        type: 'consent',
        value: {
          op: 'consent',
          ver: 1,
          hash: rulesHash,
          sig: signConsent(provider.identity.wallet, 1, rulesHash),
        },
        sender: provider.identity.publicKey,
      },
      {
        type: 'registerProvider',
        value: { op: 'register_provider' },
        sender: provider.identity.publicKey,
      },
      {
        type: 'setProviderRails',
        value: { op: 'set_provider_rails', rails: ['fiat'] },
        sender: provider.identity.publicKey,
      },
    ];
    if (payoutIndexes.includes(index)) {
      ops.push({
        type: 'setProviderPayout',
        value: {
          op: 'set_provider_payout',
          provider: provider.identity.publicKey,
          payout_addr: provider.target,
          payout_method: 'stripe',
          payout_currency: provider.currency,
        },
        sender: admin.publicKey,
      });
    }
    ops.push({
      type: 'setProviderPayout',
      value: {
        op: 'set_provider_payout',
        provider: provider.identity.publicKey,
        payout_addr: `trac1${provider.identity.publicKey.slice(0, 24)}`,
        payout_method: 'tnk',
      },
      sender: admin.publicKey,
    });
    for (const op of ops) {
      const result = await execute(contract, storage, op.type, op.value, op.sender, txNo++);
      assert.equal(result.ok, true, result.message);
    }
  }

  const debitAu = providerSpecs
    .map((spec) => BigInt(spec.gross_au))
    .reduce((sum, value) => sum + value, 0n);
  await storage.put(`bal/${user.publicKey}/fiat`, {
    user: user.publicKey,
    rail: 'fiat',
    denom: 'au_usd',
    au: au(debitAu),
    updated_epoch: 0,
    updated_at: null,
  });
  const applyValue = {
    op: 'epoch_apply',
    epoch: 1,
    at: 90_000,
    debits: [{ rail: 'fiat', user: user.publicKey, au: au(debitAu) }],
    earnings: providers.map((provider) => ({
      rail: 'fiat',
      provider: provider.identity.publicKey,
      gross_au: provider.gross_au,
    })),
  };
  await seedSpendHoldsForApply(storage, applyValue);
  const applied = await executeEpochApplyFeature(contract, storage, applyValue, admin.publicKey);
  assert.equal(applied.ok, true, applied.message);
  const applyState = (await storage.get('epoch/apply/state')).value;
  return { admin, providers, user, outsider, storage, contract, applyState };
}

async function fiatSettlementValue(ctx, {
  providerIndexes = ctx.providers.map((_, index) => index),
  stripeTransferRefs = null,
  operatorTo = 'platform_balance',
  operatorCurrency = 'eur',
  operatorAmountMinor = '45',
  operatorAu = au(45n * CENT_AU),
  overrides = {},
} = {}) {
  const providerOutputs = [];
  for (const index of providerIndexes) {
    const provider = ctx.providers[index];
    const earning = (await ctx.storage.get(`earn/fiat/${provider.identity.publicKey}`)).value;
    const payable = BigInt(earning.total_au) - BigInt(earning.paid_cum_au);
    const wholeCentAu = (payable / CENT_AU) * CENT_AU;
    if (wholeCentAu === 0n) continue;
    providerOutputs.push({
      role: 'provider',
      provider: provider.identity.publicKey,
      to: provider.target,
      currency: provider.currency,
      amount_minor: cents(wholeCentAu / CENT_AU),
      au: au(wholeCentAu),
    });
  }
  providerOutputs.sort((left, right) => left.provider.localeCompare(right.provider));
  const outputs = [
    ...providerOutputs,
    ...(operatorAmountMinor === '0' ? [] : [{
      role: 'operator_fee',
      to: operatorTo,
      currency: operatorCurrency,
      amount_minor: operatorAmountMinor,
      au: operatorAu,
    }]),
  ];
  const transferRoot = await ctx.contract.fiatSettlementTransferRoot(outputs);
  const providerAu = outputs
    .filter((output) => output.role === 'provider')
    .reduce((sum, output) => sum + BigInt(output.au), 0n);
  const operatorFeeAu = outputs
    .filter((output) => output.role === 'operator_fee')
    .reduce((sum, output) => sum + BigInt(output.au), 0n);
  const refs = stripeTransferRefs ?? outputs.map((output, index) => (
    output.role === 'provider' ? `tr_test_${index + 1}` : `platform_balance:epoch1:${index + 1}`
  ));
  const transferGroup = `mayhem_fiat_epoch_1_${ctx.applyState.last_apply_hash.slice(0, 16)}`;
  return {
    op: 'fiat_settlement',
    epoch: 1,
    at: 90_000,
    rail: 'fiat',
    processor: 'stripe',
    operator_to: operatorTo,
    epoch_apply_hash: ctx.applyState.last_apply_hash,
    stripe_transfers: outputs.map((output, index) => ({
      schema_version: 1,
      kind: output.role === 'provider' ? 'stripe_transfer' : 'platform_balance',
      ref: refs[index],
      destination: output.to,
      currency: output.currency,
      amount_minor: output.amount_minor,
      transfer_group: output.role === 'provider' ? transferGroup : null,
    })),
    transfer_root: transferRoot,
    provider_count: providerOutputs.length,
    provider_au: au(providerAu),
    operator_fee_au: au(operatorFeeAu),
    gross_au: au(providerAu + operatorFeeAu),
    outputs,
    ...overrides,
  };
}

test('MayhemContract fiatSettlement records Stripe transfer evidence and is idempotent', async () => {
  const ctx = await setupFiatSettlementContract();
  const settlement = await fiatSettlementValue(ctx);
  const preSettlementSnapshot = ctx.storage.snapshotBytes();

  ctx.contract._mayhemLastFeatureResult = undefined;
  const nonAdminResult = await executeFeature(
    ctx.contract,
    ctx.storage,
    'mayhem_feature',
    await fiatSettlementFeatureKey(ctx.contract, settlement),
    settlement,
    ctx.outsider.publicKey
  );
  const nonAdmin = nonAdminResult ?? ctx.contract._mayhemLastFeatureResult;
  assert.match(nonAdmin.message, /admin required/i);

  const settled = await executeFiatSettlementFeature(
    ctx.contract,
    ctx.storage,
    settlement,
    ctx.admin.publicKey
  );
  assert.deepEqual(settled, {
    ok: true,
    op: 'fiatSettlement',
    epoch: 1,
    rail: 'fiat',
    processor: 'stripe',
    idempotent: false,
    provider_au: au(255n * CENT_AU),
    operator_fee_au: au(45n * CENT_AU),
    gross_au: au(300n * CENT_AU),
    stripe_transfers: settlement.stripe_transfers,
    transfer_root: settlement.transfer_root,
  });

  for (const provider of ctx.providers) {
    const earning = (await ctx.storage.get(`earn/fiat/${provider.identity.publicKey}`)).value;
    const outputIndex = settlement.outputs.findIndex((output) => (
      output.role === 'provider' && output.provider === provider.identity.publicKey
    ));
    assert.notEqual(outputIndex, -1);
    assert.equal(earning.held_au, '0');
    assert.equal(earning.paid_cum_au, settlement.outputs[outputIndex].au);
    assert.equal(earning.last_settlement_stripe_ref, settlement.stripe_transfers[outputIndex].ref);
  }
  const fee = (await ctx.storage.get('fee/fiat/cum')).value;
  const operatorOutputIndex = settlement.outputs.findIndex((output) => output.role === 'operator_fee');
  assert.notEqual(operatorOutputIndex, -1);
  assert.equal(fee.cum_au, au(45n * CENT_AU));
  assert.equal(fee.swept_cum_au, au(45n * CENT_AU));
  assert.equal(fee.last_settlement_stripe_ref, settlement.stripe_transfers[operatorOutputIndex].ref);
  const record = (await ctx.storage.get('settle/fiat/1')).value;
  assert.equal(record.idempotency_key, `mayhem:fiat:settle:v1:stripe:mayhem:1:${ctx.applyState.last_apply_hash}`);
  assert.deepEqual(record.stripe_transfers, settlement.stripe_transfers);
  assert.deepEqual(record.outputs, settlement.outputs);

  const snapshot = ctx.storage.snapshotBytes();
  const replay = await executeFiatSettlementFeature(
    ctx.contract,
    ctx.storage,
    settlement,
    ctx.admin.publicKey
  );
  assert.deepEqual(replay, {
    ok: true,
    op: 'fiatSettlement',
    epoch: 1,
    rail: 'fiat',
    idempotent: true,
    stripe_transfers: settlement.stripe_transfers,
  });
  assert.equal(ctx.storage.snapshotBytes(), snapshot);

  const changedReplay = structuredClone(settlement);
  changedReplay.stripe_transfers[0].ref = 'tr_changed_replay';
  const rejected = await executeFiatSettlementFeature(
    ctx.contract,
    ctx.storage,
    changedReplay,
    ctx.admin.publicKey
  );
  assert.match(rejected.message, /already exists for epoch/i);
  assert.equal(ctx.storage.snapshotBytes(), snapshot);

  const rebuiltStorage = MemoryStorage.fromSnapshotBytes(preSettlementSnapshot);
  const rebuiltContract = new MayhemContract(
    { peer: { wallet: makeVerifier(ctx.providers[0].identity.wallet) } },
    {}
  );
  const rebuilt = await executeFiatSettlementFeature(
    rebuiltContract,
    rebuiltStorage,
    settlement,
    ctx.admin.publicKey
  );
  assert.deepEqual(rebuilt, settled);
  assert.equal(rebuiltStorage.snapshotBytes(), snapshot);
});

test('MayhemContract fiatSettlement rejects bare refs and previously consumed transfers before advancing money state', async () => {
  const ctx = await setupFiatSettlementContract();
  const settlement = await fiatSettlementValue(ctx);
  const oldShape = {
    ...settlement,
    stripe_refs: settlement.stripe_transfers
      .filter((entry) => entry.kind === 'stripe_transfer')
      .map((entry) => entry.ref),
    operator_stripe_ref: settlement.stripe_transfers
      .find((entry) => entry.kind === 'platform_balance').ref,
  };
  delete oldShape.stripe_transfers;

  await assert.rejects(
    executeFiatSettlementFeature(
      ctx.contract,
      ctx.storage,
      oldShape,
      ctx.admin.publicKey
    ),
    /does not accept fields|missing stripe_transfers/i
  );

  const consumed = settlement.stripe_transfers.find((entry) => entry.kind === 'stripe_transfer');
  await ctx.storage.put(`rail/seen/stripe/${consumed.ref}`, {
    rail: 'fiat',
    purpose: 'settlement',
  });
  const rejected = await executeFiatSettlementFeature(
    ctx.contract,
    ctx.storage,
    settlement,
    ctx.admin.publicKey
  );
  assert.match(rejected.message, /already consumed/i);
  assert.equal(await ctx.storage.get('settle/fiat/1'), null);
  for (const provider of ctx.providers) {
    assert.equal(
      (await ctx.storage.get(`earn/fiat/${provider.identity.publicKey}`)).value.paid_cum_au,
      '0'
    );
  }
  assert.equal((await ctx.storage.get('fee/fiat/cum')).value.swept_cum_au, '0');
});

test('MayhemContract fiatSettlement binds retrieved Stripe evidence to every exact output', async () => {
  const ctx = await setupFiatSettlementContract();
  const settlement = await fiatSettlementValue(ctx);
  const mutations = [
    ['destination', 'acct_wrong_provider'],
    ['currency', settlement.stripe_transfers[0].currency === 'eur' ? 'usd' : 'eur'],
    ['amount_minor', '1'],
    ['transfer_group', 'mayhem_fiat_epoch_1_wrong'],
  ];

  for (const [field, value] of mutations) {
    const changed = structuredClone(settlement);
    changed.stripe_transfers[0][field] = value;
    const rejected = await executeFiatSettlementFeature(
      ctx.contract,
      ctx.storage,
      changed,
      ctx.admin.publicKey
    );
    assert.match(rejected.message, /does not match output/i, field);
    assert.equal(await ctx.storage.get('settle/fiat/1'), null);
  }
});

test('MayhemContract fiatSettlement holds providers without admin Stripe payout targets', async () => {
  const ctx = await setupFiatSettlementContract({ payoutProviderIndexes: [0] });
  const settlement = await fiatSettlementValue(ctx, {
    providerIndexes: [0],
    stripeTransferRefs: ['tr_test_only_provider_one', 'platform_balance:epoch1:fee'],
  });

  const settled = await executeFiatSettlementFeature(
    ctx.contract,
    ctx.storage,
    settlement,
    ctx.admin.publicKey
  );
  assert.equal(settled.ok, true, settled.message);
  assert.equal(settled.provider_au, au(170n * CENT_AU));
  assert.equal(settled.operator_fee_au, au(45n * CENT_AU));

  const paidProvider = (await ctx.storage.get(`earn/fiat/${ctx.providers[0].identity.publicKey}`)).value;
  assert.equal(paidProvider.paid_cum_au, au(170n * CENT_AU));
  const heldProvider = (await ctx.storage.get(`earn/fiat/${ctx.providers[1].identity.publicKey}`)).value;
  assert.equal(heldProvider.total_au, au(85n * CENT_AU));
  assert.equal(heldProvider.paid_cum_au, '0');
  assert.equal(heldProvider.last_settlement_stripe_ref ?? null, null);
});

test('MayhemContract pays departed providers then deterministically sweeps only terminal fiat dust', async () => {
  const ctx = await setupFiatSettlementContract({
    providerSpecs: [
      {
        target: 'acct_dust_provider',
        currency: 'usd',
        gross_au: au(100n * CENT_AU + 1n),
      },
    ],
  });
  const settlement = await fiatSettlementValue(ctx, {
    operatorCurrency: 'usd',
    operatorAmountMinor: '15',
    operatorAu: au(15n * CENT_AU),
    stripeTransferRefs: ['tr_test_dust_provider', 'platform_balance:epoch1:dust-fee'],
  });

  assert.equal(settlement.outputs[0].amount_minor, '85');
  assert.equal(settlement.outputs[0].au, au(85n * CENT_AU));

  const providerKey = `prov/${ctx.providers[0].identity.publicKey}`;
  const departedProvider = (await ctx.storage.get(providerKey)).value;
  await ctx.storage.put(providerKey, { ...departedProvider, status: 'inactive', enclaves: [] });

  const settled = await executeFiatSettlementFeature(
    ctx.contract,
    ctx.storage,
    settlement,
    ctx.admin.publicKey
  );
  assert.equal(settled.ok, true, settled.message);

  const earning = (await ctx.storage.get(`earn/fiat/${ctx.providers[0].identity.publicKey}`)).value;
  assert.equal(earning.total_au, au(85n * CENT_AU + 1n));
  assert.equal(earning.paid_cum_au, au(85n * CENT_AU));
  assert.equal(BigInt(earning.total_au) - BigInt(earning.paid_cum_au), 1n);

  const sweepValue = {
    op: 'fiat_dust_sweep',
    provider: ctx.providers[0].identity.publicKey,
    epoch: 1,
    at: 90_001,
  };
  const sweepKey = await ctx.contract.fiatDustSweepFeatureKey(sweepValue);

  await ctx.storage.put(providerKey, {
    ...departedProvider,
    status: 'inactive',
    enclaves: ['canonical-enclave'],
  });
  const beforeActiveEnclaveAttempt = ctx.storage.snapshotBytes();
  const activeEnclaveResult = await executeFeature(
    ctx.contract,
    ctx.storage,
    'mayhem_feature',
    sweepKey,
    sweepValue,
    ctx.admin.publicKey
  );
  const activeEnclave = activeEnclaveResult ?? ctx.contract._mayhemLastFeatureResult;
  assert.match(activeEnclave.message, /no active enclaves/i);
  assert.equal(ctx.storage.snapshotBytes(), beforeActiveEnclaveAttempt);

  await ctx.storage.put(providerKey, { ...departedProvider, status: 'inactive', enclaves: [] });
  const earningKey = `earn/fiat/${ctx.providers[0].identity.publicKey}`;
  await ctx.storage.put(earningKey, {
    ...earning,
    held_au: '1',
    holdbacks: [{ epoch: 1, au: '1', locked_epochs: 0 }],
  });
  const providerDisputeKey = `disp/provider-open/${ctx.providers[0].identity.publicKey}`;
  await ctx.storage.put(providerDisputeKey, {
    provider: ctx.providers[0].identity.publicKey,
    count: 1,
  });
  const beforeHeldAttempt = ctx.storage.snapshotBytes();
  const heldResult = await executeFeature(
    ctx.contract,
    ctx.storage,
    'mayhem_feature',
    sweepKey,
    sweepValue,
    ctx.admin.publicKey
  );
  const held = heldResult ?? ctx.contract._mayhemLastFeatureResult;
  assert.match(held.message, /earnings are held/i);
  assert.equal(ctx.storage.snapshotBytes(), beforeHeldAttempt);
  await ctx.storage.del(providerDisputeKey);
  await ctx.storage.put(earningKey, earning);

  const beforeUnauthorized = ctx.storage.snapshotBytes();
  const unauthorizedResult = await executeFeature(
    ctx.contract,
    ctx.storage,
    'mayhem_feature',
    sweepKey,
    sweepValue,
    ctx.outsider.publicKey
  );
  const unauthorized = unauthorizedResult ?? ctx.contract._mayhemLastFeatureResult;
  assert.match(unauthorized.message, /admin required/i);
  assert.equal(ctx.storage.snapshotBytes(), beforeUnauthorized);

  const sweptResult = await executeFeature(
    ctx.contract,
    ctx.storage,
    'mayhem_feature',
    sweepKey,
    sweepValue,
    ctx.admin.publicKey
  );
  const swept = sweptResult ?? ctx.contract._mayhemLastFeatureResult;
  assert.deepEqual(swept, {
    ok: true,
    op: 'fiatDustSweep',
    provider: ctx.providers[0].identity.publicKey,
    epoch: 1,
    dust_au: '1',
    idempotent: false,
  });
  const finalEarning = (await ctx.storage.get(`earn/fiat/${ctx.providers[0].identity.publicKey}`)).value;
  assert.equal(finalEarning.total_au, au(85n * CENT_AU));
  assert.equal(finalEarning.paid_cum_au, au(85n * CENT_AU));
  assert.equal(finalEarning.fiat_dust_swept_cum_au, '1');
  const fee = (await ctx.storage.get('fee/fiat/cum')).value;
  assert.equal(fee.cum_au, au(15n * CENT_AU + 1n));
  assert.equal(fee.swept_cum_au, au(15n * CENT_AU));
  assert.equal((await ctx.storage.get(`settle/fiat-dust/${ctx.providers[0].identity.publicKey}/1`)).value.dust_au, '1');

  const replayResult = await executeFeature(
    ctx.contract,
    ctx.storage,
    'mayhem_feature',
    sweepKey,
    sweepValue,
    ctx.admin.publicKey
  );
  const replay = replayResult ?? ctx.contract._mayhemLastFeatureResult;
  assert.equal(replay.idempotent, true);
  assert.equal(replay.dust_au, '1');
});

test('MayhemContract fiatSettlement uses the active admin max_fiat_settlement_outputs param', async () => {
  const ctx = await setupFiatSettlementContract({
    paramOverrides: { max_fiat_settlement_outputs: 2 },
  });
  const settlement = await fiatSettlementValue(ctx);

  const tooManyOutputs = await executeFiatSettlementFeature(
    ctx.contract,
    ctx.storage,
    settlement,
    ctx.admin.publicKey
  );
  assert.match(tooManyOutputs.message, /max_fiat_settlement_outputs/i);
  assert.equal(await ctx.storage.get('settle/fiat/1'), null);
});

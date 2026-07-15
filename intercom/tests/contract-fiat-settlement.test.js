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
  stripeRefs = null,
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
  return {
    op: 'fiat_settlement',
    epoch: 1,
    at: 90_000,
    rail: 'fiat',
    processor: 'stripe',
    operator_to: operatorTo,
    epoch_apply_hash: ctx.applyState.last_apply_hash,
    stripe_refs: stripeRefs ?? outputs.map((output, index) => (
      output.role === 'provider' ? `tr_test_${index + 1}` : `platform_balance:epoch1:${index + 1}`
    )),
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
    stripe_refs: settlement.stripe_refs,
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
    assert.equal(earning.last_settlement_stripe_ref, settlement.stripe_refs[outputIndex]);
  }
  const fee = (await ctx.storage.get('fee/fiat/cum')).value;
  const operatorOutputIndex = settlement.outputs.findIndex((output) => output.role === 'operator_fee');
  assert.notEqual(operatorOutputIndex, -1);
  assert.equal(fee.cum_au, au(45n * CENT_AU));
  assert.equal(fee.swept_cum_au, au(45n * CENT_AU));
  assert.equal(fee.last_settlement_stripe_ref, settlement.stripe_refs[operatorOutputIndex]);
  const record = (await ctx.storage.get('settle/fiat/1')).value;
  assert.equal(record.idempotency_key, `mayhem:fiat:settle:v1:stripe:mayhem:1:${ctx.applyState.last_apply_hash}`);
  assert.deepEqual(record.stripe_refs, settlement.stripe_refs);
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
    stripe_refs: settlement.stripe_refs,
  });
  assert.equal(ctx.storage.snapshotBytes(), snapshot);

  const changedReplay = structuredClone(settlement);
  changedReplay.stripe_refs[0] = 'tr_changed_replay';
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

test('MayhemContract fiatSettlement holds providers without admin Stripe payout targets', async () => {
  const ctx = await setupFiatSettlementContract({ payoutProviderIndexes: [0] });
  const settlement = await fiatSettlementValue(ctx, {
    providerIndexes: [0],
    stripeRefs: ['tr_test_only_provider_one', 'platform_balance:epoch1:fee'],
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

test('MayhemContract fiatSettlement leaves sub-cent dust unpaid instead of marking it transferred', async () => {
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
    stripeRefs: ['tr_test_dust_provider', 'platform_balance:epoch1:dust-fee'],
  });

  assert.equal(settlement.outputs[0].amount_minor, '85');
  assert.equal(settlement.outputs[0].au, au(85n * CENT_AU));

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

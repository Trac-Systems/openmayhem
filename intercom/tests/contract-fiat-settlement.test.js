import assert from 'node:assert/strict';
import test from 'node:test';
import MayhemContract from '../contract/contract.js';
import {
  MemoryStorage,
  execute,
  executeFeature,
  makeIdentity,
  makeVerifier,
  signConsent,
} from './helpers/contract.js';

const CENT_AU = 10_000_000_000_000_000n;
const rulesHash = 'a'.repeat(64);

async function setupTargetedFiatSettlement() {
  const admin = await makeIdentity();
  const provider = await makeIdentity();
  const outsider = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const contract = new MayhemContract(
    { peer: { wallet: makeVerifier(provider.wallet) } },
    {}
  );
  for (const [type, value, sender, txNo] of [
    ['setRules', { op: 'set_rules', ver: 1, hash: rulesHash }, admin.publicKey, 1],
    [
      'setParams',
      {
        op: 'set_params',
        submitted_at: 0,
        effective_at: 86_400,
        values: {
          holdback_epochs: 0,
          new_provider_holdback_epochs: 0,
          challenge_epochs: 0,
        },
      },
      admin.publicKey,
      2,
    ],
    [
      'consent',
      {
        op: 'consent',
        ver: 1,
        hash: rulesHash,
        sig: signConsent(provider.wallet, 1, rulesHash),
      },
      provider.publicKey,
      3,
    ],
    ['registerProvider', { op: 'register_provider' }, provider.publicKey, 4],
    [
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
          chain_id: 1,
          token_address: `0x${'1'.repeat(40)}`,
          pool_address: `0x${'2'.repeat(40)}`,
        },
        tnk: {
          network: 'mainnet',
          treasury_address: `trac1${'1'.repeat(40)}`,
        },
      },
      admin.publicKey,
      5,
    ],
    [
      'setProviderRails',
      { op: 'set_provider_rails', rails: ['fiat'] },
      provider.publicKey,
      6,
    ],
  ]) {
    const result = await execute(contract, storage, type, value, sender, txNo);
    assert.equal(result.ok, true, result.message);
  }

  const revision = '1'.repeat(64);
  const target = 'acct_targeted_provider';
  const providerAu = (85n * CENT_AU).toString();
  const providerDustAu = (CENT_AU / 2n).toString();
  const providerPaidAu = (85n * CENT_AU - CENT_AU / 2n).toString();
  const operatorFeeAu = (15n * CENT_AU).toString();
  const applyHash = 'b'.repeat(64);
  const earning = {
    provider: provider.publicKey,
    rail: 'fiat',
    denom: 'au_usd',
    total_au: providerAu,
    held_au: '0',
    paid_cum_au: '0',
    holdbacks: [],
    updated_epoch: 1,
    updated_at: 'epoch/targeted/1/apply',
    last_holdback_release_epoch: 1,
  };
  await storage.put(`earn/fiat/${provider.publicKey}`, earning);
  await storage.put(`payout/binding/fiat/${provider.publicKey}/${revision}`, {
    type: 'provider_payout_binding',
    verified: true,
    provider: provider.publicKey,
    rail: 'fiat',
    revision,
    target,
    currency: 'gbp',
    chain_id: null,
    activation_epoch: 1,
  });
  await storage.put(`payout/liability/fiat/${provider.publicKey}/${revision}`, {
    ...earning,
    type: 'provider_payout_liability',
    revision,
    target,
    currency: 'gbp',
    chain_id: null,
  });
  await storage.put('fee/fiat/cum', {
    rail: 'fiat',
    denom: 'au_usd',
    cum_au: operatorFeeAu,
    swept_cum_au: '0',
    settled_cum_au: operatorFeeAu,
    updated_epoch: 1,
    updated_at: 'epoch/targeted/1/apply',
    last_apply_hash: applyHash,
    last_fee_bps: 1_500,
  });
  await storage.put('epoch/apply/state', {
    updated_epoch: 1,
    pending_epoch: null,
    last_apply_hash: applyHash,
    updated_at: 'epoch/targeted/1/apply',
  });

  const outputs = [
    {
      role: 'provider',
      provider: provider.publicKey,
      payout_revision: revision,
      to: target,
      liability_au: providerAu,
      paid_au: providerPaidAu,
      rounding_au: providerDustAu,
      dust_au: providerDustAu,
      source_currency: 'eur',
      source_amount_minor: '79',
      destination_currency: 'gbp',
      destination_amount_minor: '67',
      fx_quote_id: 'fxq_targeted_provider',
      fx_quote_hash: 'c'.repeat(64),
    },
    {
      role: 'operator_fee',
      to: 'platform_balance',
      liability_au: operatorFeeAu,
      paid_au: operatorFeeAu,
      rounding_au: '0',
      dust_au: '0',
      source_currency: 'eur',
      source_amount_minor: '14',
    },
  ];
  const transferGroup = `mayhem_fiat_epoch_1_${applyHash.slice(0, 16)}`;
  const value = {
    op: 'settle_targeted_fiat',
    epoch: 1,
    at: 90_000,
    rail: 'fiat',
    processor: 'stripe',
    source_currency: 'eur',
    operator_to: 'platform_balance',
    epoch_apply_hash: applyHash,
    stripe_transfers: [
      {
        schema_version: 2,
        kind: 'stripe_transfer',
        ref: 'tr_targeted_provider',
        destination: target,
        source_currency: 'eur',
        source_amount_minor: '79',
        destination_currency: 'gbp',
        destination_amount_minor: '67',
        fx_quote_id: 'fxq_targeted_provider',
        fx_quote_hash: 'c'.repeat(64),
        destination_payment: 'py_targeted_provider',
        transfer_group: transferGroup,
      },
      {
        schema_version: 2,
        kind: 'platform_balance',
        ref: 'platform_balance:targeted:1',
        destination: 'platform_balance',
        source_currency: 'eur',
        source_amount_minor: '14',
        transfer_group: null,
      },
    ],
    transfer_root: await contract.targetedFiatSettlementTransferRoot(outputs),
    provider_count: 1,
    provider_liability_au: providerAu,
    provider_paid_au: providerPaidAu,
    operator_fee_liability_au: operatorFeeAu,
    operator_fee_retained_au: operatorFeeAu,
    gross_liability_au: (100n * CENT_AU).toString(),
    gross_paid_au: (100n * CENT_AU - CENT_AU / 2n).toString(),
    rounding_au: providerDustAu,
    dust_au: providerDustAu,
    source_amount_minor: '93',
    destination_totals: [{ currency: 'gbp', amount_minor: '67' }],
    outputs,
  };
  return { admin, provider, outsider, storage, contract, revision, value };
}

async function applyTargetedFiat(ctx, value = ctx.value, sender = ctx.admin.publicKey) {
  ctx.contract._mayhemLastFeatureResult = undefined;
  const key = await ctx.contract.targetedFiatSettlementFeatureKey(value);
  const result = await executeFeature(
    ctx.contract,
    ctx.storage,
    'mayhem_feature',
    key,
    value,
    sender
  );
  return result ?? ctx.contract._mayhemLastFeatureResult;
}

async function sameCurrencySettlement(
  ctx,
  { currency = 'eur', quoteFields = 'valid' } = {}
) {
  const value = structuredClone(ctx.value);
  const output = value.outputs[0];
  const transfer = value.stripe_transfers[0];
  value.source_currency = currency;
  for (const entry of value.outputs) entry.source_currency = currency;
  for (const entry of value.stripe_transfers) entry.source_currency = currency;
  output.destination_currency = currency;
  output.destination_amount_minor = output.source_amount_minor;
  transfer.destination_currency = currency;
  transfer.destination_amount_minor = transfer.source_amount_minor;
  if (quoteFields === 'null') {
    output.fx_quote_id = null;
    output.fx_quote_hash = null;
    transfer.fx_quote_id = null;
    transfer.fx_quote_hash = null;
  } else if (quoteFields === 'absent') {
    delete output.fx_quote_id;
    delete output.fx_quote_hash;
    delete transfer.fx_quote_id;
    delete transfer.fx_quote_hash;
  }
  value.destination_totals = [{
    currency: output.destination_currency,
    amount_minor: output.destination_amount_minor,
  }];
  value.transfer_root = await ctx.contract.targetedFiatSettlementTransferRoot(value.outputs);

  const bindingKey = `payout/binding/fiat/${ctx.provider.publicKey}/${ctx.revision}`;
  const binding = (await ctx.storage.get(bindingKey)).value;
  await ctx.storage.put(bindingKey, { ...binding, currency: output.destination_currency });
  const liabilityKey = `payout/liability/fiat/${ctx.provider.publicKey}/${ctx.revision}`;
  const liability = (await ctx.storage.get(liabilityKey)).value;
  await ctx.storage.put(liabilityKey, { ...liability, currency: output.destination_currency });
  return value;
}

test('targeted fiat settlement consumes only its immutable payout revision', async () => {
  const ctx = await setupTargetedFiatSettlement();
  const before = ctx.storage.snapshotBytes();
  const nonAdmin = await applyTargetedFiat(ctx, ctx.value, ctx.outsider.publicKey);
  assert.match(nonAdmin.message, /admin required/i);
  assert.equal(ctx.storage.snapshotBytes(), before);

  const settled = await applyTargetedFiat(ctx);
  assert.equal(settled.ok, true, settled.message);
  assert.equal(settled.op, 'targetedFiatSettlement');
  const liability = (
    await ctx.storage.get(
      `payout/liability/fiat/${ctx.provider.publicKey}/${ctx.revision}`
    )
  ).value;
  assert.equal(liability.paid_cum_au, ctx.value.provider_paid_au);
  assert.equal(liability.last_settlement_transfer, 'tr_targeted_provider');
  assert.equal(
    (await ctx.storage.get('rail/seen/stripe-fx-quote/fxq_targeted_provider')).value.epoch,
    1
  );
  assert.equal(
    (await ctx.storage.get('rail/seen/stripe-destination-payment/py_targeted_provider')).value.epoch,
    1
  );
  assert.equal(
    (await ctx.storage.get('rail/seen/stripe-platform/platform_balance:targeted:1')).value.epoch,
    1
  );

  const replay = await applyTargetedFiat(ctx);
  assert.equal(replay.ok, true, replay.message);
  assert.equal(replay.idempotent, true);
});

test('targeted fiat settlement rejects payout revision substitution', async () => {
  const ctx = await setupTargetedFiatSettlement();
  const outputs = ctx.value.outputs.map((output) => (
    output.role === 'provider'
      ? { ...output, payout_revision: '2'.repeat(64) }
      : output
  ));
  const substituted = {
    ...ctx.value,
    outputs,
    transfer_root: await ctx.contract.targetedFiatSettlementTransferRoot(outputs),
  };
  const before = ctx.storage.snapshotBytes();
  const result = await applyTargetedFiat(ctx, substituted);
  assert.match(result.message, /immutable payout binding/i);
  assert.equal(ctx.storage.snapshotBytes(), before);
});

test('targeted fiat settlement rejects old one-currency evidence and transfer substitutions', async () => {
  const ctx = await setupTargetedFiatSettlement();
  const legacy = structuredClone(ctx.value);
  legacy.outputs[0] = {
    role: 'provider',
    provider: ctx.provider.publicKey,
    payout_revision: ctx.revision,
    to: 'acct_targeted_provider',
    currency: 'usd',
    amount_minor: '85',
    au: ctx.value.provider_paid_au,
  };
  assert.match(
    ctx.contract.normalizeTargetedFiatSettlementValue(legacy).message,
    /does not accept fields|missing fields/i
  );

  const substituted = structuredClone(ctx.value);
  substituted.stripe_transfers[0].destination_amount_minor = '68';
  const result = await applyTargetedFiat(ctx, substituted);
  assert.match(result.message, /transfer does not match output/i);

  for (const mutate of [
    (value) => { value.outputs[0].source_amount_minor = '0'; },
    (value) => { value.outputs[0].destination_amount_minor = '0'; },
    (value) => { value.stripe_transfers[0].source_amount_minor = '0'; },
    (value) => { value.stripe_transfers[0].destination_amount_minor = '0'; },
  ]) {
    const zeroAmount = structuredClone(ctx.value);
    mutate(zeroAmount);
    assert.match(
      ctx.contract.normalizeTargetedFiatSettlementValue(zeroAmount).message,
      /amount must be positive/i
    );
  }
});

test('targeted fiat settlement rejects globally consumed quote and destination-payment evidence', async () => {
  for (const seenKey of [
    'rail/seen/stripe/tr_targeted_provider',
    'rail/seen/stripe-platform/platform_balance:targeted:1',
    'rail/seen/stripe-fx-quote/fxq_targeted_provider',
    'rail/seen/stripe-destination-payment/py_targeted_provider',
  ]) {
    const ctx = await setupTargetedFiatSettlement();
    await ctx.storage.put(seenKey, { epoch: 0, purpose: 'injected-replay' });
    const before = ctx.storage.snapshotBytes();
    const result = await applyTargetedFiat(ctx);
    assert.match(result.message, /evidence was already consumed/i);
    assert.equal(ctx.storage.snapshotBytes(), before);
  }
});

test('targeted fiat settlement retains non-USD same-currency valuation quote uniqueness', async () => {
  const ctx = await setupTargetedFiatSettlement();
  const value = await sameCurrencySettlement(ctx);
  const settled = await applyTargetedFiat(ctx, value);
  assert.equal(settled.ok, true, settled.message);
  assert.equal(settled.stripe_transfers[0].destination_payment, 'py_targeted_provider');
  assert.equal(
    (await ctx.storage.get('rail/seen/stripe-fx-quote/fxq_targeted_provider')).value.epoch,
    1
  );
  assert.equal(
    (await ctx.storage.get('rail/seen/stripe/tr_targeted_provider')).value.epoch,
    1
  );
  assert.equal(
    (await ctx.storage.get(
      'rail/seen/stripe-destination-payment/py_targeted_provider'
    )).value.epoch,
    1
  );

  const replayCtx = await setupTargetedFiatSettlement();
  const replayedQuote = await sameCurrencySettlement(replayCtx);
  await replayCtx.storage.put(
    'rail/seen/stripe-fx-quote/fxq_targeted_provider',
    { epoch: 0, purpose: 'injected-replay' }
  );
  const before = replayCtx.storage.snapshotBytes();
  const replayResult = await applyTargetedFiat(replayCtx, replayedQuote);
  assert.match(replayResult.message, /evidence was already consumed/i);
  assert.equal(replayCtx.storage.snapshotBytes(), before);
});

test('targeted fiat settlement accepts a direct USD payout with a null quote pair', async () => {
  const ctx = await setupTargetedFiatSettlement();
  const value = await sameCurrencySettlement(ctx, {
    currency: 'usd',
    quoteFields: 'null',
  });
  const settled = await applyTargetedFiat(ctx, value);
  assert.equal(settled.ok, true, settled.message);
  assert.equal(settled.stripe_transfers[0].fx_quote_id, null);
  assert.equal(settled.stripe_transfers[0].fx_quote_hash, null);
  assert.equal(
    (await ctx.storage.get('rail/seen/stripe/tr_targeted_provider')).value.epoch,
    1
  );
  assert.equal(
    (await ctx.storage.get(
      'rail/seen/stripe-destination-payment/py_targeted_provider'
    )).value.epoch,
    1
  );
  assert.deepEqual(
    [...ctx.storage.values.keys()].filter((key) => (
      key.startsWith('rail/seen/stripe-fx-quote/')
    )),
    []
  );
});

test('targeted fiat settlement enforces quote identity by USD valuation path', async () => {
  const nonUsdCtx = await setupTargetedFiatSettlement();
  for (const quoteFields of ['absent', 'null']) {
    const value = await sameCurrencySettlement(nonUsdCtx, { quoteFields });
    assert.match(
      nonUsdCtx.contract.normalizeTargetedFiatSettlementOutput(value.outputs[0]).message,
      /FX quote identity is required/i
    );
    assert.match(
      nonUsdCtx.contract.normalizeStripeTransferEvidence(value.stripe_transfers[0]).message,
      /FX quote identity is required/i
    );
  }

  const quotedUsdCtx = await setupTargetedFiatSettlement();
  const quotedUsd = await sameCurrencySettlement(quotedUsdCtx, { currency: 'usd' });
  assert.match(
    quotedUsdCtx.contract.normalizeTargetedFiatSettlementOutput(quotedUsd.outputs[0]).message,
    /direct USD.*FX quote/i
  );
  assert.match(
    quotedUsdCtx.contract.normalizeStripeTransferEvidence(quotedUsd.stripe_transfers[0]).message,
    /direct USD.*FX quote/i
  );

  const inconsistentCtx = await setupTargetedFiatSettlement();
  const inconsistent = await sameCurrencySettlement(inconsistentCtx, {
    currency: 'usd',
    quoteFields: 'null',
  });
  delete inconsistent.stripe_transfers[0].fx_quote_id;
  delete inconsistent.stripe_transfers[0].fx_quote_hash;
  const inconsistentResult = await applyTargetedFiat(inconsistentCtx, inconsistent);
  assert.match(inconsistentResult.message, /transfer does not match output/i);
});

test('targeted fiat settlement keeps same-currency amount and destination-payment checks', async () => {
  const amountCtx = await setupTargetedFiatSettlement();
  const wrongAmount = await sameCurrencySettlement(amountCtx);
  wrongAmount.stripe_transfers[0].destination_amount_minor = '80';
  const amountResult = await applyTargetedFiat(amountCtx, wrongAmount);
  assert.match(amountResult.message, /transfer does not match output/i);

  const paymentCtx = await setupTargetedFiatSettlement();
  const invalidPayment = await sameCurrencySettlement(paymentCtx);
  invalidPayment.stripe_transfers[0].destination_payment = 'not_a_payment';
  assert.match(
    paymentCtx.contract.normalizeTargetedFiatSettlementValue(invalidPayment).message,
    /destination payment readback/i
  );

  const replayCtx = await setupTargetedFiatSettlement();
  const replayedPayment = await sameCurrencySettlement(replayCtx);
  await replayCtx.storage.put(
    'rail/seen/stripe-destination-payment/py_targeted_provider',
    { epoch: 0, purpose: 'injected-replay' }
  );
  const before = replayCtx.storage.snapshotBytes();
  const replayResult = await applyTargetedFiat(replayCtx, replayedPayment);
  assert.match(replayResult.message, /evidence was already consumed/i);
  assert.equal(replayCtx.storage.snapshotBytes(), before);
});

test('targeted fiat settlement still requires strict FX quotes across currencies', async () => {
  const ctx = await setupTargetedFiatSettlement();
  for (const quoteFields of ['absent', 'null']) {
    const output = structuredClone(ctx.value.outputs[0]);
    const transfer = structuredClone(ctx.value.stripe_transfers[0]);
    if (quoteFields === 'absent') {
      delete output.fx_quote_id;
      delete output.fx_quote_hash;
      delete transfer.fx_quote_id;
      delete transfer.fx_quote_hash;
    } else {
      output.fx_quote_id = null;
      output.fx_quote_hash = null;
      transfer.fx_quote_id = null;
      transfer.fx_quote_hash = null;
    }
    assert.match(
      ctx.contract.normalizeTargetedFiatSettlementOutput(output).message,
      /FX quote identity is required/i
    );
    assert.match(
      ctx.contract.normalizeStripeTransferEvidence(transfer).message,
      /FX quote identity is required/i
    );
  }
});

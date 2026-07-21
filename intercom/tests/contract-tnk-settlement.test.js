import assert from 'node:assert/strict';
import test from 'node:test';
import MayhemContract from '../contract/contract.js';
import {
  MemoryStorage,
  execute,
  executeFeature,
  executeRateFeature,
  makeIdentity,
  makeVerifier,
  signConsent,
} from './helpers/contract.js';

const rulesHash = 'a'.repeat(64);
const treasury = `testtrac1${'1'.repeat(40)}`;
const rate = {
  op: 'rate_oracle',
  tnk_usd_au: '50000000000000000',
  source: 'gate-spot',
  ts: 90_000,
};

async function setupTargetedTnkSettlement() {
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
        tnk: { network: 'testnet1', treasury_address: treasury },
      },
      admin.publicKey,
      3,
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
      4,
    ],
    ['registerProvider', { op: 'register_provider' }, provider.publicKey, 5],
    [
      'setProviderRails',
      { op: 'set_provider_rails', rails: ['tnk'] },
      provider.publicKey,
      6,
    ],
  ]) {
    const result = await execute(contract, storage, type, value, sender, txNo);
    assert.equal(result.ok, true, result.message);
  }
  const rateResult = await executeRateFeature(
    contract,
    storage,
    rate,
    admin.publicKey
  );
  assert.equal(rateResult.ok, true, rateResult.message);

  const revision = '3'.repeat(64);
  const target = 'testtrac1targetedprovider';
  const providerAu = '850000';
  const operatorFeeAu = '150000';
  const applyHash = 'c'.repeat(64);
  const earning = {
    provider: provider.publicKey,
    rail: 'tnk',
    denom: 'au_usd',
    total_au: providerAu,
    held_au: '0',
    paid_cum_au: '0',
    holdbacks: [],
    updated_epoch: 1,
    updated_at: 'epoch/targeted/1/apply',
    last_holdback_release_epoch: 1,
  };
  await storage.put(`earn/tnk/${provider.publicKey}`, earning);
  await storage.put(`payout/binding/tnk/${provider.publicKey}/${revision}`, {
    type: 'provider_payout_binding',
    verified: true,
    provider: provider.publicKey,
    rail: 'tnk',
    revision,
    target,
    currency: null,
    chain_id: null,
    activation_epoch: 1,
  });
  await storage.put(`payout/liability/tnk/${provider.publicKey}/${revision}`, {
    ...earning,
    type: 'provider_payout_liability',
    revision,
    target,
    currency: null,
    chain_id: null,
  });
  await storage.put('fee/tnk/cum', {
    rail: 'tnk',
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
      au: providerAu,
      tnk_e18: '17000000',
    },
    {
      role: 'operator_fee',
      to: 'testtrac1operator',
      au: operatorFeeAu,
      tnk_e18: '3000000',
    },
  ];
  const value = {
    op: 'settle_targeted_tnk',
    epoch: 1,
    at: 90_000,
    rail: 'tnk',
    network: 'testnet1',
    treasury_from: treasury,
    operator_to: 'testtrac1operator',
    epoch_apply_hash: applyHash,
    rate_tnk_usd_au: rate.tnk_usd_au,
    rate_source: rate.source,
    rate_ts: rate.ts,
    msb_transfers: outputs.map((output, index) => ({
      schema_version: 1,
      network: 'testnet1',
      tx_hash: (index + 1).toString(16).repeat(64),
      confirmed_length: 100 + index,
      observed_signed_length: 120,
      from: treasury,
      to: output.to,
      amount_e18: output.tnk_e18,
    })),
    transfer_root: await contract.targetedTnkSettlementTransferRoot(outputs),
    provider_count: 1,
    provider_au: providerAu,
    operator_fee_au: operatorFeeAu,
    gross_au: '1000000',
    tnk_e18: '20000000',
    outputs,
  };
  return { admin, provider, outsider, storage, contract, revision, value };
}

async function applyTargetedTnk(ctx, value = ctx.value, sender = ctx.admin.publicKey) {
  ctx.contract._mayhemLastFeatureResult = undefined;
  const key = await ctx.contract.targetedTnkSettlementFeatureKey(value);
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

test('targeted TNK settlement consumes only its immutable payout revision', async () => {
  const ctx = await setupTargetedTnkSettlement();
  const before = ctx.storage.snapshotBytes();
  const nonAdmin = await applyTargetedTnk(ctx, ctx.value, ctx.outsider.publicKey);
  assert.match(nonAdmin.message, /admin required/i);
  assert.equal(ctx.storage.snapshotBytes(), before);

  const settled = await applyTargetedTnk(ctx);
  assert.equal(settled.ok, true, settled.message);
  assert.equal(settled.op, 'targetedTnkSettlement');
  const liability = (
    await ctx.storage.get(
      `payout/liability/tnk/${ctx.provider.publicKey}/${ctx.revision}`
    )
  ).value;
  assert.equal(liability.paid_cum_au, ctx.value.provider_au);
  assert.equal(liability.last_settlement_transfer, ctx.value.msb_transfers[0].tx_hash);

  const replay = await applyTargetedTnk(ctx);
  assert.equal(replay.ok, true, replay.message);
  assert.equal(replay.idempotent, true);
});

test('targeted TNK settlement rejects payout revision substitution', async () => {
  const ctx = await setupTargetedTnkSettlement();
  const outputs = ctx.value.outputs.map((output) => (
    output.role === 'provider'
      ? { ...output, payout_revision: '4'.repeat(64) }
      : output
  ));
  const substituted = {
    ...ctx.value,
    outputs,
    transfer_root: await ctx.contract.targetedTnkSettlementTransferRoot(outputs),
  };
  const before = ctx.storage.snapshotBytes();
  const result = await applyTargetedTnk(ctx, substituted);
  assert.match(result.message, /immutable payout binding/i);
  assert.equal(ctx.storage.snapshotBytes(), before);
});

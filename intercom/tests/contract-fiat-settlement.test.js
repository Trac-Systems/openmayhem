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
      'setProviderRails',
      { op: 'set_provider_rails', rails: ['fiat'] },
      provider.publicKey,
      5,
    ],
  ]) {
    const result = await execute(contract, storage, type, value, sender, txNo);
    assert.equal(result.ok, true, result.message);
  }

  const revision = '1'.repeat(64);
  const target = 'acct_targeted_provider';
  const providerAu = (85n * CENT_AU).toString();
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
    currency: 'usd',
    chain_id: null,
    activation_epoch: 1,
  });
  await storage.put(`payout/liability/fiat/${provider.publicKey}/${revision}`, {
    ...earning,
    type: 'provider_payout_liability',
    revision,
    target,
    currency: 'usd',
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
      currency: 'usd',
      amount_minor: '85',
      au: providerAu,
    },
    {
      role: 'operator_fee',
      to: 'platform_balance',
      currency: 'usd',
      amount_minor: '15',
      au: operatorFeeAu,
    },
  ];
  const transferGroup = `mayhem_fiat_epoch_1_${applyHash.slice(0, 16)}`;
  const value = {
    op: 'settle_targeted_fiat',
    epoch: 1,
    at: 90_000,
    rail: 'fiat',
    processor: 'stripe',
    operator_to: 'platform_balance',
    epoch_apply_hash: applyHash,
    stripe_transfers: [
      {
        schema_version: 1,
        kind: 'stripe_transfer',
        ref: 'tr_targeted_provider',
        destination: target,
        currency: 'usd',
        amount_minor: '85',
        transfer_group: transferGroup,
      },
      {
        schema_version: 1,
        kind: 'platform_balance',
        ref: 'platform_balance:targeted:1',
        destination: 'platform_balance',
        currency: 'usd',
        amount_minor: '15',
        transfer_group: null,
      },
    ],
    transfer_root: await contract.targetedFiatSettlementTransferRoot(outputs),
    provider_count: 1,
    provider_au: providerAu,
    operator_fee_au: operatorFeeAu,
    gross_au: (100n * CENT_AU).toString(),
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
  assert.equal(liability.paid_cum_au, ctx.value.provider_au);
  assert.equal(liability.last_settlement_transfer, 'tr_targeted_provider');

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

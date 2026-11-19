import assert from 'node:assert/strict';
import test from 'node:test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import Autobase from 'autobase';
import Corestore from 'corestore';
import Hyperbee from 'hyperbee';
import Wallet from 'trac-peer/src/wallet.js';
import { FeatureOperation, FeatureCheck } from 'trac-peer/src/operations/feature/index.js';
import MayhemContract from '../contract/contract.js';
import ContractV30 from '../contract/v30.js';
import {
  MemoryStorage,
  execute,
  executeFeature,
  executeRateFeature,
  makeIdentity,
  makeVerifier,
  prepareTargetedPayout,
  signConsent,
} from './helpers/contract.js';

const rulesHash = 'a'.repeat(64);
const chainId = 1;
const tokenAddress = `0x${'1'.repeat(40)}`;
const poolAddress = `0x${'2'.repeat(40)}`;
const providerTarget = `0x${'3'.repeat(40)}`;
const rate = {
  op: 'tap_rate_oracle',
  tap_usd_au: '1000000000000000000',
  source: 'uniswap-v3-twap',
  ts: 90_000,
};
const grossSpendForProviderClaims = (providerClaimWei, providerShareBps = 7_500) => (
  (
    BigInt(providerClaimWei) * 10_000n +
    BigInt(providerShareBps) -
    1n
  ) / BigInt(providerShareBps)
);

async function setupTargetedTapSettlement({
  totalAu = '850000',
  heldAu = '0',
  paidCumAu = '0',
  feeBps = 1_500,
  rateAt = rate.ts,
  snapshotAt = 90_000,
  proposalAt = 90_000,
  Implementation = MayhemContract,
} = {}) {
  const providerShareBps = 10_000 - feeBps - 1_000;
  const admin = await makeIdentity();
  const provider = await makeIdentity();
  const outsider = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const contract = new Implementation(
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
          fee_bps: feeBps,
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
          chain_id: chainId,
          token_address: tokenAddress,
          pool_address: poolAddress,
        },
        tnk: {
          network: 'testnet1',
          treasury_address: `testtrac1${'1'.repeat(40)}`,
        },
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
    ['setProviderRails', { op: 'set_provider_rails', rails: ['tap'] }, provider.publicKey, 6],
  ]) {
    const result = await execute(contract, storage, type, value, sender, txNo);
    assert.equal(result.ok, true, result.message);
  }
  const rateResult = await executeRateFeature(
    contract,
    storage,
    { ...rate, ts: rateAt },
    admin.publicKey
  );
  assert.equal(rateResult.ok, true, rateResult.message);
  const rateRecord = (await storage.get('tap/rate/latest')).value;

  const revision = '4'.repeat(64);
  const applyHash = '5'.repeat(64);
  const earning = {
    provider: provider.publicKey,
    rail: 'tap',
    denom: 'au_usd',
    total_au: totalAu,
    held_au: heldAu,
    paid_cum_au: paidCumAu,
    holdbacks: heldAu === '0'
      ? []
      : [{ epoch: 1, au: heldAu, locked_epochs: 2 }],
    updated_epoch: 1,
    updated_at: 'epoch/targeted/1/apply',
    last_holdback_release_epoch: 1,
    chain_id: chainId,
    pool_address: poolAddress,
  };
  await storage.put(`earn/tap/${provider.publicKey}`, earning);
  await storage.put(`payout/binding/tap/${provider.publicKey}/${revision}`, {
    type: 'provider_payout_binding',
    verified: true,
    provider: provider.publicKey,
    rail: 'tap',
    revision,
    target: providerTarget,
    currency: null,
    chain_id: chainId,
    activation_epoch: 1,
  });
  await storage.put(`payout/liability/tap/${provider.publicKey}/${revision}`, {
    ...earning,
    type: 'provider_payout_liability',
    revision,
    target: providerTarget,
    currency: null,
    chain_id: chainId,
  });
  await storage.put('epoch/apply/state', {
    updated_epoch: 1,
    pending_epoch: null,
    last_apply_hash: applyHash,
    last_settlement_unix: snapshotAt,
    updated_at: 'epoch/targeted/1/apply',
  });

  const payableAu = (
    BigInt(totalAu) - BigInt(heldAu) - BigInt(paidCumAu)
  ).toString();
  const tapWei = payableAu;
  const cumulativeClaimWei = (BigInt(paidCumAu) + BigInt(tapWei)).toString();
  const priorGrossSpendWei = grossSpendForProviderClaims(
    paidCumAu,
    providerShareBps
  );
  const cumulativeGrossSpendWei = grossSpendForProviderClaims(cumulativeClaimWei, providerShareBps);
  const providerEntries = [{
    account: providerTarget,
    cumulative_wei: cumulativeClaimWei,
  }];
  const refunds = [];
  const entries = structuredClone(providerEntries);
  const outputs = [{
    provider: provider.publicKey,
    payout_revision: revision,
    to: providerTarget,
    paid_cum_au_before: paidCumAu,
    aggregate_paid_cum_au_before: paidCumAu,
    paid_au: payableAu,
    tap_wei: tapWei,
    prior_cumulative_claim_wei: paidCumAu,
    cumulative_claim_wei: cumulativeClaimWei,
  }];
  await storage.put(
    `settle/targeted/tap/claim/${chainId}/${poolAddress}/${providerTarget}`,
    {
      type: 'targeted_tap_cumulative_claim',
      chain_id: chainId,
      token_address: tokenAddress,
      pool_address: poolAddress,
      account: providerTarget,
      cumulative_claim_wei: paidCumAu,
      updated_epoch: 0,
      updated_at: null,
    }
  );
  await storage.put(`settle/targeted/tap/state/${chainId}/${poolAddress}`, {
    type: 'targeted_tap_settlement_state',
    chain_id: chainId,
    token_address: tokenAddress,
    pool_address: poolAddress,
    payment_config_ver: 1,
    last_epoch: 0,
    cumulative_spent_wei: priorGrossSpendWei.toString(),
    cumulative_provider_claimed_wei: paidCumAu,
    cumulative_buyer_refund_wei: '0',
    last_root: null,
    last_execution_tx: null,
    updated_at: null,
  });
  const value = {
    op: 'settle_targeted_tap',
    epoch: 1,
    at: snapshotAt,
    rail: 'tap',
    chain_id: chainId,
    token_address: tokenAddress,
    pool_address: poolAddress,
    payment_config_ver: 1,
    epoch_apply_hash: applyHash,
    tap_rate_lock: {
      type: 'tap_settlement_rate_lock',
      epoch: 1,
      bundle_sha256: '6'.repeat(64),
      denom: 'tap_usd_au',
      tap_usd_au: rate.tap_usd_au,
      source: rate.source,
      rate_ts: rateAt,
      rate_record_key: rateRecord.updated_at,
      posted_by: admin.publicKey,
      posted_by_role: 'admin',
      chain_id: chainId,
      token_address: tokenAddress,
      pool_address: poolAddress,
      payment_config_ver: 1,
    },
    root: contract.targetedTapSettlementRoot(entries),
    root_confirmed: true,
    proposal_tx: `0x${'7'.repeat(64)}`,
    proposal_block_number: 88,
    proposal_block_timestamp: proposalAt,
    proposal_block_hash: `0x${'8'.repeat(64)}`,
    execution_tx: `0x${'9'.repeat(64)}`,
    execution_status: 1,
    execution_block_number: 100,
    execution_block_hash: `0x${'a'.repeat(64)}`,
    finalized_block_number: 112,
    confirmation_depth: 12,
    confirmation_policy: 'finalized-tag',
    cumulative_spent_wei: cumulativeGrossSpendWei.toString(),
    provider_cumulative_claimed_wei: cumulativeClaimWei,
    buyer_refund_wei: '0',
    fee_bps: feeBps,
    tap_burn_bps: 1_000,
    provider_share_bps: providerShareBps,
    provider_count: 1,
    provider_paid_au: payableAu,
    provider_tap_wei: tapWei,
    provider_entries: providerEntries,
    refunds,
    entries,
    outputs,
  };
  return {
    admin,
    provider,
    outsider,
    storage,
    contract,
    revision,
    applyHash,
    providerShareBps,
    value,
  };
}

async function applyTargetedTap(ctx, value = ctx.value, sender = ctx.admin.publicKey) {
  const settlementExists = await ctx.storage.get(`settle/targeted/tap/${value.epoch}`);
  const prepared = await prepareTargetedPayout(
    ctx.contract,
    ctx.storage,
    value,
    ctx.admin,
    { submit: sender === ctx.admin.publicKey && settlementExists === null }
  );
  if (prepared instanceof Error || prepared?.ok === false) return prepared;
  ctx.contract._mayhemLastFeatureResult = undefined;
  const key = await ctx.contract.targetedTapSettlementFeatureKey(value);
  if (key instanceof Error) return key;
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

test('targeted TAP settlement binds execution and advances exact payout liability once', async () => {
  const ctx = await setupTargetedTapSettlement();
  const before = ctx.storage.snapshotBytes();
  const nonAdmin = await applyTargetedTap(ctx, ctx.value, ctx.outsider.publicKey);
  assert.match(nonAdmin.message, /admin required/i);
  assert.equal(ctx.storage.snapshotBytes(), before);

  const settled = await applyTargetedTap(ctx);
  assert.equal(settled.ok, true, settled.message);
  assert.equal(settled.idempotent, false);
  const liability = (
    await ctx.storage.get(
      `payout/liability/tap/${ctx.provider.publicKey}/${ctx.revision}`
    )
  ).value;
  assert.equal(liability.paid_cum_au, ctx.value.provider_paid_au);
  assert.equal(liability.last_settlement_transfer, ctx.value.execution_tx);

  const replay = await applyTargetedTap(ctx);
  assert.equal(replay.ok, true, replay.message);
  assert.equal(replay.idempotent, true);
});

test('targeted TAP settlement uses total minus held minus paid as exact payable', async () => {
  const ctx = await setupTargetedTapSettlement({
    totalAu: '1000000',
    heldAu: '100000',
    paidCumAu: '50000',
  });
  assert.equal(ctx.value.provider_paid_au, '850000');
  const settled = await applyTargetedTap(ctx);
  assert.equal(settled.ok, true, settled.message);
  const liability = (
    await ctx.storage.get(
      `payout/liability/tap/${ctx.provider.publicKey}/${ctx.revision}`
    )
  ).value;
  assert.equal(liability.paid_cum_au, '900000');
});

test('targeted TAP settlement accepts exact immutable evidence after cadence advances', async () => {
  const ctx = await setupTargetedTapSettlement();
  await ctx.storage.put('epoch/apply-anchor/1', {
    type: 'epoch_apply_anchor',
    epoch: 1,
    apply_hash: ctx.applyHash,
    settlement_unix: 90_000,
    applied_at: 'epoch/targeted/1/apply',
  });
  await ctx.storage.put('epoch/apply/state', {
    updated_epoch: 2,
    pending_epoch: null,
    last_apply_hash: 'f'.repeat(64),
    last_settlement_unix: 93_600,
    updated_at: 'epoch/targeted/2/apply',
  });
  const liabilityKey =
    `payout/liability/tap/${ctx.provider.publicKey}/${ctx.revision}`;
  const liabilityBefore = (await ctx.storage.get(liabilityKey)).value;
  await ctx.storage.put(liabilityKey, {
    ...liabilityBefore,
    total_au: (BigInt(liabilityBefore.total_au) + 500000n).toString(),
    updated_epoch: 2,
    updated_at: 'epoch/targeted/2/apply',
  });
  const earningKey = `earn/tap/${ctx.provider.publicKey}`;
  const earningBefore = (await ctx.storage.get(earningKey)).value;
  await ctx.storage.put(earningKey, {
    ...earningBefore,
    total_au: (BigInt(earningBefore.total_au) + 500000n).toString(),
    updated_epoch: 2,
    updated_at: 'epoch/targeted/2/apply',
  });

  const settled = await applyTargetedTap(ctx);
  assert.equal(settled.ok, true, settled.message);
  assert.equal(settled.epoch, 1);
  assert.equal(settled.idempotent, false);
  const liabilityAfter = (await ctx.storage.get(liabilityKey)).value;
  assert.equal(liabilityAfter.paid_cum_au, ctx.value.provider_paid_au);
  assert.equal(
    BigInt(liabilityAfter.total_au) - BigInt(liabilityAfter.paid_cum_au),
    500000n
  );
  const replay = await applyTargetedTap(ctx);
  assert.equal(replay.ok, true, replay.message);
  assert.equal(replay.idempotent, true);
});

test('targeted TAP settlement rejects payout target, revision, rate, and amount substitution', async () => {
  for (const mutate of [
    (value) => { value.outputs[0].payout_revision = 'b'.repeat(64); },
    (value) => { value.outputs[0].to = `0x${'c'.repeat(40)}`; },
    (value) => { value.tap_rate_lock.tap_usd_au = '2000000000000000000'; },
    (value) => { value.outputs[0].tap_wei = '849999'; },
  ]) {
    const ctx = await setupTargetedTapSettlement();
    const value = structuredClone(ctx.value);
    mutate(value);
    const before = ctx.storage.snapshotBytes();
    const result = await applyTargetedTap(ctx, value);
    assert.ok(result instanceof Error || result?.message, 'mutation must reject');
    assert.equal(ctx.storage.snapshotBytes(), before);
  }
});

test('targeted TAP settlement rejects apply, root, confirmation, and transaction replay', async () => {
  for (const mutate of [
    (value) => { value.epoch_apply_hash = 'd'.repeat(64); },
    (value) => { value.root = `0x${'e'.repeat(64)}`; },
    (value) => {
      value.finalized_block_number = 111;
      value.confirmation_depth = 11;
    },
  ]) {
    const ctx = await setupTargetedTapSettlement();
    const value = structuredClone(ctx.value);
    mutate(value);
    const before = ctx.storage.snapshotBytes();
    const result = await applyTargetedTap(ctx, value);
    assert.ok(result instanceof Error || result?.message, 'mutation must reject');
    assert.equal(ctx.storage.snapshotBytes(), before);
  }

  const ctx = await setupTargetedTapSettlement();
  const prepared = await prepareTargetedPayout(
    ctx.contract,
    ctx.storage,
    ctx.value,
    ctx.admin
  );
  assert.equal(prepared, ctx.value);
  await ctx.storage.put(
    `rail/seen/tap-settlement-root/${chainId}/${poolAddress}/${ctx.value.root}`,
    { epoch: 0, purpose: 'injected-replay' }
  );
  const rejected = await applyTargetedTap(ctx);
  assert.match(rejected.message, /root was already consumed/i);
});

test('targeted TAP settlement rejects duplicate outputs and overpayment', async () => {
  const duplicateCtx = await setupTargetedTapSettlement();
  const duplicate = structuredClone(duplicateCtx.value);
  duplicate.outputs.push(structuredClone(duplicate.outputs[0]));
  duplicate.provider_count = 2;
  duplicate.provider_paid_au = '1700000';
  duplicate.provider_tap_wei = '1700000';
  const duplicateResult = await applyTargetedTap(duplicateCtx, duplicate);
  assert.match(duplicateResult.message, /duplicate/i);

  const overpayCtx = await setupTargetedTapSettlement();
  const overpay = structuredClone(overpayCtx.value);
  overpay.outputs[0].paid_au = '850001';
  overpay.outputs[0].tap_wei = '850001';
  overpay.outputs[0].cumulative_claim_wei = '850001';
  overpay.provider_entries[0].cumulative_wei = '850001';
  overpay.entries[0].cumulative_wei = '850001';
  overpay.provider_paid_au = '850001';
  overpay.provider_tap_wei = '850001';
  overpay.provider_cumulative_claimed_wei = '850001';
  overpay.cumulative_spent_wei =
    grossSpendForProviderClaims('850001', overpayCtx.providerShareBps).toString();
  overpay.root = overpayCtx.contract.targetedTapSettlementRoot(overpay.entries);
  const overpayResult = await applyTargetedTap(overpayCtx, overpay);
  assert.match(overpayResult.message, /exceeds revision liability/i);
});

test('targeted TAP settlement chains two provider liabilities sharing one target', async () => {
  const ctx = await setupTargetedTapSettlement();
  const secondProvider = await makeIdentity();
  for (const [type, value, sender, txNo] of [
    [
      'consent',
      {
        op: 'consent',
        ver: 1,
        hash: rulesHash,
        sig: signConsent(secondProvider.wallet, 1, rulesHash),
      },
      secondProvider.publicKey,
      7,
    ],
    ['registerProvider', { op: 'register_provider' }, secondProvider.publicKey, 8],
    ['setProviderRails', { op: 'set_provider_rails', rails: ['tap'] }, secondProvider.publicKey, 9],
  ]) {
    const result = await execute(ctx.contract, ctx.storage, type, value, sender, txNo);
    assert.equal(result.ok, true, result.message);
  }

  const secondRevision = 'd'.repeat(64);
  const secondPaidAu = '400000';
  const secondEarning = {
    provider: secondProvider.publicKey,
    rail: 'tap',
    denom: 'au_usd',
    total_au: secondPaidAu,
    held_au: '0',
    paid_cum_au: '0',
    holdbacks: [],
    updated_epoch: 1,
    updated_at: 'epoch/targeted/1/apply',
    last_holdback_release_epoch: 1,
    chain_id: chainId,
    pool_address: poolAddress,
  };
  await ctx.storage.put(`earn/tap/${secondProvider.publicKey}`, secondEarning);
  await ctx.storage.put(
    `payout/binding/tap/${secondProvider.publicKey}/${secondRevision}`,
    {
      type: 'provider_payout_binding',
      verified: true,
      provider: secondProvider.publicKey,
      rail: 'tap',
      revision: secondRevision,
      target: providerTarget,
      currency: null,
      chain_id: chainId,
      activation_epoch: 1,
    }
  );
  await ctx.storage.put(
    `payout/liability/tap/${secondProvider.publicKey}/${secondRevision}`,
    {
      ...secondEarning,
      type: 'provider_payout_liability',
      revision: secondRevision,
      target: providerTarget,
      currency: null,
      chain_id: chainId,
    }
  );

  const outputs = [
    {
      provider: ctx.provider.publicKey,
      payout_revision: ctx.revision,
      to: providerTarget,
      paid_cum_au_before: '0',
      aggregate_paid_cum_au_before: '0',
      paid_au: ctx.value.provider_paid_au,
      tap_wei: ctx.value.provider_tap_wei,
    },
    {
      provider: secondProvider.publicKey,
      payout_revision: secondRevision,
      to: providerTarget,
      paid_cum_au_before: '0',
      aggregate_paid_cum_au_before: '0',
      paid_au: secondPaidAu,
      tap_wei: secondPaidAu,
    },
  ].sort((left, right) => (
    left.provider.localeCompare(right.provider) ||
    left.payout_revision.localeCompare(right.payout_revision)
  ));
  let cursor = 0n;
  for (const output of outputs) {
    output.prior_cumulative_claim_wei = cursor.toString();
    cursor += BigInt(output.tap_wei);
    output.cumulative_claim_wei = cursor.toString();
  }
  const providerEntries = [{
    account: providerTarget,
    cumulative_wei: cursor.toString(),
  }];
  const entries = structuredClone(providerEntries);
  const value = {
    ...ctx.value,
    root: ctx.contract.targetedTapSettlementRoot(entries),
    cumulative_spent_wei:
      grossSpendForProviderClaims(cursor, ctx.providerShareBps).toString(),
    provider_cumulative_claimed_wei: cursor.toString(),
    buyer_refund_wei: '0',
    provider_count: 2,
    provider_paid_au: cursor.toString(),
    provider_tap_wei: cursor.toString(),
    provider_entries: providerEntries,
    refunds: [],
    entries,
    outputs,
  };

  const settled = await applyTargetedTap(ctx, value);
  assert.equal(settled.ok, true, settled.message);
  assert.equal(
    (await ctx.storage.get(
      `payout/liability/tap/${ctx.provider.publicKey}/${ctx.revision}`
    )).value.paid_cum_au,
    ctx.value.provider_paid_au
  );
  assert.equal(
    (await ctx.storage.get(
      `payout/liability/tap/${secondProvider.publicKey}/${secondRevision}`
    )).value.paid_cum_au,
    secondPaidAu
  );
  assert.equal(
    (await ctx.storage.get(
      `settle/targeted/tap/claim/${chainId}/${poolAddress}/${providerTarget}`
    )).value.cumulative_claim_wei,
    cursor.toString()
  );

  const replay = await applyTargetedTap(ctx, value);
  assert.equal(replay.ok, true, replay.message);
  assert.equal(replay.idempotent, true);

  const conflict = structuredClone(value);
  conflict.execution_tx = `0x${'b'.repeat(64)}`;
  conflict.execution_block_hash = `0x${'c'.repeat(64)}`;
  assert.match(
    (await applyTargetedTap(ctx, conflict)).message,
    /already exists for epoch/i
  );
});

test('targeted TAP settlement keeps gross, provider claims, and same-account refunds separate', async () => {
  const ctx = await setupTargetedTapSettlement();
  const refundWei = 12_345n;
  const providerClaimWei = BigInt(ctx.value.provider_cumulative_claimed_wei);
  const refunds = [{
    account: providerTarget,
    cumulative_wei: refundWei.toString(),
  }];
  const entries = [{
    account: providerTarget,
    cumulative_wei: (providerClaimWei + refundWei).toString(),
  }];
  const value = {
    ...ctx.value,
    buyer_refund_wei: refundWei.toString(),
    refunds,
    entries,
    root: ctx.contract.targetedTapSettlementRoot(entries),
  };

  const settled = await applyTargetedTap(ctx, value);
  assert.equal(settled.ok, true, settled.message);
  const state = (
    await ctx.storage.get(`settle/targeted/tap/state/${chainId}/${poolAddress}`)
  ).value;
  assert.equal(
    state.cumulative_spent_wei,
    grossSpendForProviderClaims(providerClaimWei, ctx.providerShareBps).toString()
  );
  assert.equal(state.cumulative_provider_claimed_wei, providerClaimWei.toString());
  assert.equal(state.cumulative_buyer_refund_wei, refundWei.toString());
  assert.equal(
    (
      await ctx.storage.get(
        `settle/targeted/tap/claim/${chainId}/${poolAddress}/${providerTarget}`
      )
    ).value.cumulative_claim_wei,
    providerClaimWei.toString()
  );
  assert.equal(
    (
      await ctx.storage.get(
        `settle/targeted/tap/refund/${chainId}/${poolAddress}/${providerTarget}`
      )
    ).value.cumulative_refund_wei,
    refundWei.toString()
  );

  const replay = await applyTargetedTap(ctx, value);
  assert.equal(replay.ok, true, replay.message);
  assert.equal(replay.idempotent, true);
});

test('targeted TAP settlement fails closed when the ledger fee diverges from the on-chain split', async () => {
  const ctx = await setupTargetedTapSettlement({
    totalAu: '800000',
    feeBps: 1_000,
  });
  assert.equal(ctx.providerShareBps, 8_000);
  assert.equal(ctx.value.provider_tap_wei, '800000');
  assert.equal(ctx.value.cumulative_spent_wei, '1000000');

  const before = ctx.storage.snapshotBytes();
  const rejected = await applyTargetedTap(ctx);
  assert.match(
    rejected.message,
    /fixed on-chain operator split/i
  );
  assert.equal(ctx.storage.snapshotBytes(), before);
});

test('v31 settles the difference of exact cumulative caps with the later proposal clock', async () => {
  // Cumulative gross goes from 2 to 3. Both provider caps are exact (1 and 2).
  // The 1-wei payout is larger than floor(1 * 7500 / 10000).
  const day = Date.UTC(2030, 0, 2) / 1000;
  const snapshotAt = day + 7 * 3600 + 59 * 60;
  const rateAt = day + 8 * 3600 + 55 * 60;
  const proposalAt = day + 8 * 3600 + 57 * 60;
  const ctx = await setupTargetedTapSettlement({
    totalAu: '2', paidCumAu: '1', snapshotAt, rateAt, proposalAt,
  });
  assert.equal(ctx.value.cumulative_spent_wei, '3');
  const result = await applyTargetedTap(ctx);
  assert.equal(result.ok, true, result.message);
  const stored = (await ctx.storage.get('settle/targeted/tap/1')).value;
  assert.equal(stored.at, snapshotAt);
  assert.equal(stored.proposal_block_timestamp, proposalAt);
  assert.equal(stored.provider_tap_wei, '1');
  assert.equal((await applyTargetedTap(ctx)).idempotent, true);
  const legacy = structuredClone(ctx.value);
  delete legacy.proposal_block_timestamp;
  const old = new ContractV30({}, {});
  assert.ok(!(old.normalizeTargetedTapSettlementValue(legacy) instanceof Error));
  assert.ok(old.normalizeTargetedTapSettlementValue(ctx.value) instanceof Error);
  assert.ok(ctx.contract.normalizeTargetedTapSettlementValue(legacy) instanceof Error);
});

test('TAP refuses either inexact cumulative cap, including a matching delta between inexact caps', async () => {
  for (const [priorGross, nextGross] of [['1','3'], ['2','2'], ['4','6']]) {
    const ctx = await setupTargetedTapSettlement({ totalAu: '2', paidCumAu: '1' });
    const stateKey = `settle/targeted/tap/state/${chainId}/${poolAddress}`;
    const state = (await ctx.storage.get(stateKey)).value;
    await ctx.storage.put(stateKey, { ...state, cumulative_spent_wei: priorGross });
    ctx.value.cumulative_spent_wei = nextGross;
    const result = await applyTargetedTap(ctx);
    assert.match(result.message, /provider entitlement/);
    assert.equal(await ctx.storage.get('settle/targeted/tap/1'), null);
    assert.equal((await ctx.storage.get(stateKey)).value.last_epoch, 0);
  }
});

test('TAP applies proposal-time fee and freshness policy, including rotated history', async () => {
  const schedule = (key, value, next, change) => ({ key,
    current: { ver: 1, effective_at: 0, value },
    pending: { ver: 2, effective_at: change, value: next } });
  for (const [seconds, allowed] of [[120,true],[121,false]]) {
    const ctx = await setupTargetedTapSettlement({ snapshotAt: rate.ts - 10, proposalAt: rate.ts + seconds });
    await ctx.storage.put('params/rate_staleness_seconds',
      schedule('rate_staleness_seconds', 2700, 120, rate.ts + 1));
    const result = await applyTargetedTap(ctx);
    if (allowed) assert.equal(result.ok, true, result.message);
    else assert.match(result.message, /stale/);
  }
  const future = await setupTargetedTapSettlement({ snapshotAt: rate.ts - 100, proposalAt: rate.ts - 1 });
  assert.match((await applyTargetedTap(future)).message, /in the future/);
  const fee = await setupTargetedTapSettlement({ proposalAt: rate.ts + 120 });
  await fee.storage.put('params/fee_bps', schedule('fee_bps', 1500, 1000, rate.ts + 100));
  assert.match((await applyTargetedTap(fee)).message, /fixed on-chain operator split/);
  const old = await setupTargetedTapSettlement({ proposalAt: rate.ts + 200 });
  await old.storage.put('params/rate_staleness_seconds', { key: 'rate_staleness_seconds',
    current: { ver: 3, effective_at: rate.ts + 1000, value: 2700 }, pending: null });
  const verify = async () => (await applyTargetedTap(old)).message;
  assert.match(await verify(), /Missing historical TAP parameter/);
  for (const [ver, value, effective_at] of [[2,120,rate.ts],[3,2700,rate.ts+1000]]) {
    await old.storage.put('params/update/'+ver, {
      ver, effective_at, values: { rate_staleness_seconds: value }, set_by_role: 'admin',
    });
  }
  assert.match(await verify(), /stale/);
});

test('TAP binds the liability clock to the original signed preparations', async () => {
  const ctx = await setupTargetedTapSettlement({ proposalAt: rate.ts + 5 });
  await prepareTargetedPayout(ctx.contract, ctx.storage, ctx.value, ctx.admin);
  const before = ctx.storage.snapshotBytes();
  ctx.value.at++;
  assert.match((await applyTargetedTap(ctx)).message, /snapshot does not match/);
  assert.equal(ctx.storage.snapshotBytes(), before);
});

test('native signed v30 TAP history replays to the original tree including its historical rounding', async t => {
  const ctx = await setupTargetedTapSettlement({
    totalAu: '2', paidCumAu: '1', Implementation: ContractV30,
  });
  // V30 accepted floor(delta gross * 3/4): gross 2 -> 4, claim 1 -> 2.
  // This is a historical operation that v31 must never reinterpret.
  delete ctx.value.proposal_block_timestamp;
  ctx.value.cumulative_spent_wei = '4';
  assert.equal(await prepareTargetedPayout(ctx.contract, ctx.storage, ctx.value, ctx.admin,
    { contractVersion: 30 }), ctx.value);
  const key = 'mayhem_' + await ctx.contract.targetedTapSettlementFeatureKey(ctx.value);
  const nonce = 'synthetic-tap-history';
  const signature = Buffer.from(ctx.admin.wallet.sign(Buffer.from(JSON.stringify(ctx.value) + nonce))).toString('hex');
  const op = { type: 'feature', key, value: { dispatch: { type: 'mayhem_feature', contract_version: 30,
    key, address: ctx.admin.publicKey, value: ctx.value, nonce, hash: signature } } };
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'tap-v30-history-'));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const seed = async batch => { for (const [key, value] of ctx.storage.values) await batch.put(key, value); };
  const wallet = new Wallet();
  await wallet.ready;
  const handler = (contract, canonicalView) => new FeatureOperation(new FeatureCheck(), {
    wallet, protocolInstance: { featMaxBytes: () => 1e6 }, contractInstance: contract, canonicalView,
  });
  const viewOptions = { extension: false, keyEncoding: 'utf-8', valueEncoding: 'json' };
  const store = new Corestore(path.join(dir, 'writer'));
  let historyLength;
  const base = new Autobase(store, null, { valueEncoding: 'json', ackInterval: 0,
    open: storage => new Hyperbee(storage.get('view'), viewOptions),
    async apply(nodes, view) {
      const batch = view.batch();
      try {
        for (const node of nodes) {
          if (node.value === 'seed') await seed(batch);
          else if (node.value?.type === 'feature') {
            historyLength = node.length;
            await handler(ctx.contract, view).handle(node.value, batch, base, node);
          }
        }
        await batch.flush();
      } finally { await batch.close(); }
    } });
  const replayStore = new Corestore(path.join(dir, 'replay'));
  const replay = new Hyperbee(replayStore.get({ name: 'view' }), viewOptions);
  try {
    await base.ready();
    await base.append('seed');
    await base.append(op);
    await base.update();
    assert.equal((await base.view.get('fr/' + signature)).value.ok, true);
    const node = { value: op, from: base.local, length: historyLength, indexed: true, optimistic: false };
    await replay.ready();
    const prefix = replay.batch();
    try {
      await seed(prefix);
      await prefix.flush();
    } finally { await prefix.close(); }
    const batch = replay.batch();
    const current = new MayhemContract(ctx.contract.protocol, {});
    try {
      await handler(current, base.view).handle(op, batch, base, node);
      await batch.flush();
    } finally { await batch.close(); }
    assert.equal(current._mayhemReplayStatus.completed, 1);
    assert.deepEqual(await replay.core.treeHash(), await base.view.core.treeHash());
    const restored = (await replay.get('settle/targeted/tap/1')).value;
    assert.equal(restored.provider_cumulative_claimed_wei, '2');
    assert.equal(Object.hasOwn(restored, 'proposal_block_timestamp'), false);
    const fresh = { ...ctx.value, proposal_block_timestamp: ctx.value.at };
    assert.match(current.normalizeTargetedTapSettlementValue(fresh).message, /exact cap/);
  } finally {
    await replay.close();
    await replayStore.close();
    await base.close();
    await store.close();
  }
});

test('v31 consumes the already signed v30 preparations with the recovered proposal evidence exactly once', async () => {
  const ctx = await setupTargetedTapSettlement({ totalAu: '2', paidCumAu: '1',
    snapshotAt: rate.ts - 3360, proposalAt: rate.ts + 120, Implementation: ContractV30 });
  const archive = structuredClone(ctx.value);
  delete archive.proposal_block_timestamp;
  assert.equal(await prepareTargetedPayout(ctx.contract, ctx.storage, archive, ctx.admin, { contractVersion: 30 }), archive);
  const archivedBytes = JSON.stringify(archive);
  const oldResult = await executeFeature(ctx.contract, ctx.storage, 'mayhem_feature',
    await ctx.contract.targetedTapSettlementFeatureKey(archive), archive, ctx.admin.publicKey);
  assert.match(oldResult.message, /oracle timestamp is in the future/);
  const current = new MayhemContract(ctx.contract.protocol, {});
  const submission = { ...structuredClone(archive), proposal_block_timestamp: ctx.value.proposal_block_timestamp };
  const key = await current.targetedTapSettlementFeatureKey(submission);
  const submit = () => executeFeature(current, ctx.storage, 'mayhem_feature', key, submission, ctx.admin.publicKey);
  assert.equal((await submit()).idempotent, false);
  const state = ctx.storage.snapshotBytes();
  assert.equal((await submit()).idempotent, true);
  assert.equal(ctx.storage.snapshotBytes(), state);
  const preparation = (await ctx.storage.get(current.payoutPreparationRecordKey('tap', archive.root_preparation_id))).value;
  assert.equal(preparation.contract_version, 30);
  assert.equal(preparation.consumed_by, 'settle/targeted/tap/1');
  assert.equal(JSON.stringify(archive), archivedBytes);
});

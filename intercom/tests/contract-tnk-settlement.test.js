import assert from 'node:assert/strict';
import test from 'node:test';
import Contract30 from '../contract/v30.js';
import { inventory, advanceHead } from './helpers/payout-history.js';
import b4a from 'b4a';

import MayhemContract, {
  CONTRACT_VERSION,
  payoutPreparationMessage,
  targetedPayoutControlMessage,
} from '../contract/contract.js';
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
const applyHash = 'b'.repeat(64);
const rate = {
  op: 'rate_oracle',
  tnk_usd_au: '50000000000000000',
  source: 'gate-spot',
  ts: 90_000,
};

const signHex = (wallet, message) =>
  b4a.toString(wallet.sign(b4a.from(message)), 'hex');

async function submitControl(ctx, value, featureKeyMethod) {
  value.admin_sig = signHex(ctx.admin.wallet, targetedPayoutControlMessage(value));
  const key = await ctx.contract[featureKeyMethod](value);
  assert.equal(key instanceof Error, false, key.message);
  return await executeFeature(
    ctx.contract,
    ctx.storage,
    'mayhem_feature',
    key,
    value,
    ctx.admin.publicKey
  );
}

async function buildPlan(ctx, outputs, carry = [], { expectOk = true, epoch = 1, hash = applyHash } = {}) {
  const outcome = outputs.length > 0 ? 'payouts' : carry.length > 0 ? 'carry' : 'no_work';
  const outputsRoot = await ctx.contract.opaqueHash(
    'mayhem-targeted-payout-epoch-outputs-v1',
    { rail: 'tnk', epoch, outputs }
  );
  const carryRoot = await ctx.contract.opaqueHash(
    'mayhem-targeted-payout-epoch-carry-v1',
    { rail: 'tnk', epoch, carry }
  );
  const planRoot = await ctx.contract.opaqueHash(
    'mayhem-targeted-payout-epoch-plan-v1',
    {
      rail: 'tnk',
      epoch,
      at: 90_000,
      epoch_apply_hash: hash,
      snapshot_signed_length: 120,
      outcome,
      outputs_root: outputsRoot,
      carry_root: carryRoot,
    }
  );
  const value = {
    op: 'prepare_targeted_payout_epoch',
    contract_version: ctx.version ?? CONTRACT_VERSION,
    rail: 'tnk',
    epoch,
    at: 90_000,
    epoch_apply_hash: hash,
    snapshot_signed_length: 120,
    outcome,
    outputs,
    carry,
    outputs_root: outputsRoot,
    carry_root: carryRoot,
    plan_root: planRoot,
    admin: ctx.admin.publicKey,
    admin_sig: '0'.repeat(128),
  };
  const result = await submitControl(ctx, value, 'targetedPayoutEpochFeatureKey');
  if (!expectOk) return { value, result };
  assert.equal(result.ok, true, result.message);
  return value;
}

function signedMsbPayload(output, txHash) {
  return {
    type: 9,
    address: treasury,
    tro: {
      tx: txHash,
      txv: 'c'.repeat(64),
      to: output.to,
      am: BigInt(output.tnk_e18).toString(16).padStart(32, '0'),
      in: 'd'.repeat(64),
      is: 'e'.repeat(128),
    },
  };
}

async function prepareOutput(ctx, plan, output, txHash) {
  const payload = {
    settlement_op: 'settle_targeted_tnk_output',
    rail: 'tnk',
    epoch: plan.epoch,
    epoch_apply_hash: plan.epoch_apply_hash,
    plan_root: plan.plan_root,
    economic_op_id: output.economic_op_id,
    output_index: output.output_index,
    output,
    network: 'testnet1',
    treasury_from: treasury,
    rate_tnk_usd_au: rate.tnk_usd_au,
    rate_source: rate.source,
    rate_ts: rate.ts,
    msb_tx_hash: txHash,
    msb_payload: signedMsbPayload(output, txHash),
  };
  const liability = output.role === 'provider'
    ? {
        provider: output.provider,
        payout_revision: output.payout_revision,
        target: output.to,
        currency: null,
        chain_id: null,
        paid_cum_au_before: output.paid_cum_au_before,
        aggregate_paid_cum_au_before: output.aggregate_paid_cum_au_before,
        liability_au: output.au,
        paid_au: output.au,
      }
    : null;
  const value = {
    op: 'prepare_targeted_payout',
    contract_version: ctx.version ?? CONTRACT_VERSION,
    economic_op_id: output.economic_op_id,
    rail: 'tnk',
    epoch: plan.epoch,
    epoch_apply_hash: plan.epoch_apply_hash,
    prepared_at: 90_000,
    kind: output.role === 'provider' ? 'liability' : 'fee',
    output_index: output.output_index,
    payload_hash: await ctx.contract.opaqueHash(
      'mayhem-targeted-payout-preparation-payload-v1',
      {
        economic_op_id: output.economic_op_id,
        rail: 'tnk',
        epoch: plan.epoch,
        epoch_apply_hash: plan.epoch_apply_hash,
        kind: output.role === 'provider' ? 'liability' : 'fee',
        output_index: output.output_index,
        payload,
      }
    ),
    payload,
    liability,
    external_effect_ids: [txHash],
    admin: ctx.admin.publicKey,
    admin_sig: '0'.repeat(128),
  };
  value.admin_sig = signHex(ctx.admin.wallet, payoutPreparationMessage(value));
  const key = await ctx.contract.targetedPayoutPreparationFeatureKey(value);
  if (key instanceof Error) return key;
  return await executeFeature(
    ctx.contract,
    ctx.storage,
    'mayhem_feature',
    key,
    value,
    ctx.admin.publicKey
  );
}

async function settleOutput(ctx, plan, output, txHash) {
  const value = {
    op: 'settle_targeted_tnk_output',
    contract_version: ctx.version ?? CONTRACT_VERSION,
    rail: 'tnk',
    epoch: plan.epoch,
    at: 90_000,
    epoch_apply_hash: plan.epoch_apply_hash,
    plan_root: plan.plan_root,
    economic_op_id: output.economic_op_id,
    output_index: output.output_index,
    preparation_id: output.economic_op_id,
    external_effect_id: txHash,
    msb_transfer: {
      schema_version: 1,
      network: 'testnet1',
      tx_hash: txHash,
      confirmed_length: 100 + output.output_index,
      observed_signed_length: 120,
      from: treasury,
      to: output.to,
      amount_e18: output.tnk_e18,
    },
    admin: ctx.admin.publicKey,
    admin_sig: '0'.repeat(128),
  };
  return {
    value,
    result: await submitControl(ctx, value, 'targetedTnkOutputFeatureKey'),
  };
}

async function closeEpoch(ctx, plan) {
  const value = {
    op: 'close_targeted_payout_epoch',
    contract_version: ctx.version ?? CONTRACT_VERSION,
    rail: 'tnk',
    epoch: plan.epoch,
    at: 90_000,
    epoch_apply_hash: plan.epoch_apply_hash,
    plan_root: plan.plan_root,
    admin: ctx.admin.publicKey,
    admin_sig: '0'.repeat(128),
  };
  return await submitControl(ctx, value, 'closeTargetedPayoutEpochFeatureKey');
}

async function setup({
  payoutMinAu = '1',
  providerAu = '850000',
  operatorAu = '150000',
} = {}) {
  const admin = await makeIdentity();
  const provider = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const contract = new MayhemContract(
    { peer: { wallet: makeVerifier(admin.wallet) } },
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
          payout_min_au: payoutMinAu,
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
  const rateResult = await executeRateFeature(contract, storage, rate, admin.publicKey);
  assert.equal(rateResult.ok, true, rateResult.message);

  const revision = '3'.repeat(64);
  const target = 'testtrac1targetedprovider';
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
  await storage.put('payout/liability-index/tnk', {
    type: 'provider_payout_liability_index',
    rail: 'tnk',
    entries: [{
      provider: provider.publicKey,
      payout_revision: revision,
    }],
    updated_epoch: 1,
    updated_at: 'epoch/targeted/1/apply',
  });
  await storage.put('fee/tnk/cum', {
    rail: 'tnk',
    denom: 'au_usd',
    cum_au: operatorAu,
    swept_cum_au: '0',
    settled_cum_au: operatorAu,
    updated_epoch: 1,
    updated_at: 'epoch/targeted/1/apply',
    last_apply_hash: applyHash,
    last_fee_bps: 1_500,
  });
  await storage.put('epoch/apply-anchor/1', {
    type: 'epoch_apply_anchor',
    epoch: 1,
    apply_hash: applyHash,
    settlement_unix: 90_000,
    applied_at: 'epoch/targeted/1/apply',
  });
  await storage.put('epoch/apply/state', {
    updated_epoch: 1,
    updated_at: 'epoch/targeted/1/apply',
    last_apply_hash: applyHash,
    last_settlement_unix: 90_000,
  });
  const outputs = [{
      economic_op_id: '1'.repeat(64),
      output_index: 0,
      role: 'provider',
      provider: provider.publicKey,
      payout_revision: revision,
      to: target,
      paid_cum_au_before: '0',
      aggregate_paid_cum_au_before: '0',
      au: providerAu,
      tnk_e18: (
        (BigInt(providerAu) * 1_000_000_000_000_000_000n + 49_999_999_999_999_999n) /
        50_000_000_000_000_000n
      ).toString(),
    }];
  if (operatorAu !== '0') {
    outputs.push({
      economic_op_id: '2'.repeat(64),
      output_index: 1,
      role: 'operator_fee',
      to: 'testtrac1operator',
      au: operatorAu,
      tnk_e18: (
        (BigInt(operatorAu) * 1_000_000_000_000_000_000n + 49_999_999_999_999_999n) /
        50_000_000_000_000_000n
      ).toString(),
    });
  }
  return { admin, provider, storage, contract, revision, outputs };
}

test('TNK output progress survives a later failure and closes only when complete', async () => {
  const ctx = await setup();
  const plan = await buildPlan(ctx, ctx.outputs);
  const providerTx = '4'.repeat(64);
  const feeTx = '5'.repeat(64);

  const prepared = await prepareOutput(ctx, plan, ctx.outputs[0], providerTx);
  assert.equal(prepared.ok, true, prepared.message);
  const first = await settleOutput(ctx, plan, ctx.outputs[0], providerTx);
  assert.equal(first.result.ok, true, first.result.message);
  assert.match((await closeEpoch(ctx, plan)).message, /unsettled planned outputs/i);

  const liabilityKey =
    `payout/liability/tnk/${ctx.provider.publicKey}/${ctx.revision}`;
  assert.equal((await ctx.storage.get(liabilityKey)).value.paid_cum_au, '850000');
  const retry = await submitControl(
    ctx,
    first.value,
    'targetedTnkOutputFeatureKey'
  );
  assert.equal(retry.ok, true, retry.message);
  assert.equal(retry.idempotent, true);

  assert.equal((await prepareOutput(ctx, plan, ctx.outputs[1], feeTx)).ok, true);
  assert.equal((await settleOutput(ctx, plan, ctx.outputs[1], feeTx)).result.ok, true);
  const closed = await closeEpoch(ctx, plan);
  assert.equal(closed.ok, true, closed.message);
  assert.equal(closed.output_count, 2);
});

test('TNK preparation binds the exact signed MSB payload and rejects effect reuse', async () => {
  const ctx = await setup();
  const plan = await buildPlan(ctx, ctx.outputs);
  const txHash = '6'.repeat(64);
  const prepared = await prepareOutput(ctx, plan, ctx.outputs[0], txHash);
  assert.equal(prepared.ok, true, prepared.message);

  const conflicting = await prepareOutput(ctx, plan, ctx.outputs[1], txHash);
  assert.match(conflicting.message, /external effect id already has a preparation/i);

  const changedOutput = structuredClone(ctx.outputs[0]);
  changedOutput.to = 'testtrac1different';
  const invalidPlan = structuredClone(plan);
  invalidPlan.outputs[0] = changedOutput;
  const invalid = await prepareOutput(ctx, invalidPlan, changedOutput, '7'.repeat(64));
  assert.match(invalid.message, /canonical epoch plan|plan output/i);
});

test('TNK preparation refreshes a matured revision holdback before validating its liability', async () => {
  const ctx = await setup();
  const liabilityKey =
    `payout/liability/tnk/${ctx.provider.publicKey}/${ctx.revision}`;
  const liability = (await ctx.storage.get(liabilityKey)).value;
  await ctx.storage.put(liabilityKey, {
    ...liability,
    held_au: liability.total_au,
    holdbacks: [{ epoch: 0, au: liability.total_au, locked_epochs: 1 }],
    updated_epoch: 0,
    last_holdback_release_epoch: 0,
  });

  const plan = await buildPlan(ctx, ctx.outputs);
  const prepared = await prepareOutput(
    ctx,
    plan,
    ctx.outputs[0],
    '8'.repeat(64)
  );
  assert.equal(prepared.ok, true, prepared.message);
});

test('TNK carry-only epoch closes without fabricating an external effect', async () => {
  const ctx = await setup({
    payoutMinAu: '1000000000000000000',
    operatorAu: '0',
  });
  const carry = [{
    provider: ctx.provider.publicKey,
    payout_revision: ctx.revision,
    liability_au: '850000',
    held_au: '0',
    payable_au: '850000',
    payout_min_au: '1000000000000000000',
    reason: 'below_payout_minimum',
  }];
  const plan = await buildPlan(ctx, [], carry);
  const closed = await closeEpoch(ctx, plan);
  assert.equal(closed.ok, true, closed.message);
  assert.equal(closed.outcome, 'carry');
  assert.equal(closed.output_count, 0);
  assert.equal(closed.carry_count, 1);
});

test('TNK epoch plans fail closed when payable or carried liabilities are omitted', async () => {
  const payable = await setup();
  const omittedPayable = await buildPlan(
    payable,
    [],
    [],
    { expectOk: false }
  );
  assert.match(
    omittedPayable.result.message,
    /include every payable canonical liability exactly/i
  );
  const omittedOperator = await buildPlan(
    payable,
    [payable.outputs[0]],
    [],
    { expectOk: false }
  );
  assert.match(
    omittedOperator.result.message,
    /complete canonical operator fee/i
  );

  const carried = await setup({
    payoutMinAu: '1000000000000000000',
    operatorAu: '0',
  });
  const omittedCarry = await buildPlan(
    carried,
    [],
    [],
    { expectOk: false }
  );
  assert.match(
    omittedCarry.result.message,
    /explicitly carry every held or below-minimum liability/i
  );
});

for (const legacy of [false, true]) {
  test(`TNK settles disjoint later tranches before original ${legacy ? 'v30' : 'v31'} transfers`, async () => {
    const ctx = await setup();
    const current = ctx.contract;
    if (legacy) {
      ctx.contract = new Contract30(current.protocol, {});
      ctx.version = 30;
    }
    const [original, fee] = ctx.outputs;
    const oldPlan = await buildPlan(ctx, ctx.outputs);
    const oldHash = '4'.repeat(64);
    assert.equal((await prepareOutput(ctx, oldPlan, original, oldHash)).ok, true);
    const prepKey = `payout/preparation/tnk/${original.economic_op_id}`;
    const read = async key => (await ctx.storage.get(key))?.value ?? null;
    const originalPrep = structuredClone(await read(prepKey));
    ctx.contract = current;
    delete ctx.version;
    await inventory(ctx, 'tnk');
    const revision = `payout/liability/tnk/${original.provider}/${original.payout_revision}`;
    const aggregate = `earn/tnk/${original.provider}`;
    for (const key of [revision, aggregate]) {
      const row = await read(key);
      await ctx.storage.put(key, { ...row, total_au: String(BigInt(row.total_au) + 100n), updated_epoch: 2 });
    }
    const fees = await read('fee/tnk/cum');
    await ctx.storage.put('fee/tnk/cum', { ...fees, updated_epoch: 2, cum_au: String(BigInt(fees.cum_au)+50n),
      settled_cum_au: String(BigInt(fees.settled_cum_au)+50n) });
    const hash = 'c'.repeat(64);
    await advanceHead(ctx.storage, 2, hash);
    const later = { ...original, economic_op_id: '5'.repeat(64), au: '100', tnk_e18: '2000',
      paid_cum_au_before: original.au, aggregate_paid_cum_au_before: original.au };
    const laterFee = { ...fee, economic_op_id: '6'.repeat(64), au: '50', tnk_e18: '1000' };
    const options = { epoch: 2, hash };
    const overlap = await buildPlan(ctx, [{ ...later, paid_cum_au_before: '0' }, laterFee], [],
      { ...options, expectOk: false });
    assert.match(overlap.result.message, /every payable canonical liability/);
    const plan = await buildPlan(ctx, [later, laterFee], [], options);
    const feeHash = '7'.repeat(64);
    assert.equal((await prepareOutput(ctx, plan, laterFee, feeHash)).ok, true);
    assert.equal((await settleOutput(ctx, plan, laterFee, feeHash)).result.ok, true);
    const laterHash = '8'.repeat(64);
    assert.equal((await prepareOutput(ctx, plan, later, laterHash)).ok, true);
    assert.match((await settleOutput(ctx, plan, later, oldHash)).result.message, /preparation|effect/i);
    assert.equal((await settleOutput(ctx, plan, later, laterHash)).result.ok, true);
    assert.equal((await closeEpoch(ctx, plan)).ok, true);
    assert.deepEqual(await read(prepKey), originalPrep);
    assert.equal((await read(revision)).paid_cum_au, '100');
    assert.match((await closeEpoch(ctx, oldPlan)).message, /unsettled/);
    const first = await settleOutput(ctx, oldPlan, original, oldHash);
    assert.equal(first.result.ok, true, first.result.message);
    assert.equal((await settleOutput(ctx, oldPlan, original, oldHash)).result.idempotent, true);
    const oldFeeHash = '9'.repeat(64);
    assert.equal((await prepareOutput(ctx, oldPlan, fee, oldFeeHash)).ok, true);
    assert.equal((await settleOutput(ctx, oldPlan, fee, oldFeeHash)).result.ok, true);
    assert.equal((await closeEpoch(ctx, oldPlan)).ok, true);
    assert.equal((await read(revision)).paid_cum_au, String(BigInt(original.au)+100n));
    assert.equal((await read(revision)).last_settlement_epoch, 2);
    assert.equal((await read(revision)).last_settlement_transfer, laterHash);
    assert.equal((await read(aggregate)).last_holdback_release_epoch, 2);
    assert.equal((await read('fee/tnk/cum')).last_targeted_settlement_transfer, feeHash);
    assert.equal((await read(`payout/booked/tnk/revision/${original.provider}/${ctx.revision}`)).pending_au, '0');
  });
}

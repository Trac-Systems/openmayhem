import assert from 'node:assert/strict';
import test from 'node:test';
import Contract27 from '../contract/history/v27.js';
import Contract28 from '../contract/history/v28.js';
import { fiatFenceKey, fiatRecoveryFeatureKey, fiatResolutionKey } from '../contract/fiat-recovery.js';
import { buildRecoveryFeature, buildVerifiedSettlement, inspectOverlap, snapshotContract } from '../scripts/fiat-preparation-recovery.mjs';
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
  makeIdentity,
  makeVerifier,
  signConsent,
} from './helpers/contract.js';

const CENT_AU = 10_000_000_000_000_000n;
const rulesHash = 'a'.repeat(64);
const applyHash = 'b'.repeat(64);

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

async function planValue(ctx, epoch = 1, anchor = applyHash) {
  const outputsRoot = await ctx.contract.opaqueHash(
    'mayhem-targeted-payout-epoch-outputs-v1',
    { rail: 'fiat', epoch, outputs: ctx.outputs }
  );
  const carryRoot = await ctx.contract.opaqueHash(
    'mayhem-targeted-payout-epoch-carry-v1',
    { rail: 'fiat', epoch, carry: [] }
  );
  const planRoot = await ctx.contract.opaqueHash(
    'mayhem-targeted-payout-epoch-plan-v1',
    {
      rail: 'fiat',
      epoch,
      at: 90_000,
      epoch_apply_hash: anchor,
      snapshot_signed_length: 220,
      outcome: 'payouts',
      outputs_root: outputsRoot,
      carry_root: carryRoot,
    }
  );
  return {
    op: 'prepare_targeted_payout_epoch',
    contract_version: ctx.version ?? CONTRACT_VERSION,
    rail: 'fiat',
    epoch,
    at: 90_000,
    epoch_apply_hash: anchor,
    snapshot_signed_length: 220,
    outcome: 'payouts',
    outputs: ctx.outputs,
    carry: [],
    outputs_root: outputsRoot,
    carry_root: carryRoot,
    plan_root: planRoot,
    admin: ctx.admin.publicKey,
    admin_sig: '0'.repeat(128),
  };
}

async function buildPlan(ctx) {
  const value = await planValue(ctx);
  const result = await submitControl(ctx, value, 'targetedPayoutEpochFeatureKey');
  assert.equal(result.ok, true, result.message);
  return value;
}

async function prepareEconomicOutput(ctx, plan, output) {
  const payload = {
    settlement_op: 'settle_targeted_fiat_output',
    plan_root: plan.plan_root,
    economic_op_id: output.economic_op_id,
    rail: 'fiat',
    epoch: plan.epoch,
    epoch_apply_hash: plan.epoch_apply_hash,
    output_index: output.output_index,
    output,
    processor: 'stripe',
    source_currency: output.source_currency,
  };
  const liability = output.role === 'provider'
    ? {
        provider: output.provider,
        payout_revision: output.payout_revision,
        target: output.to,
        currency: output.destination_currency,
        chain_id: null,
        paid_cum_au_before: output.paid_cum_au_before,
        aggregate_paid_cum_au_before: output.aggregate_paid_cum_au_before,
        liability_au: output.liability_au,
        paid_au: output.paid_au,
      }
    : null;
  const kind = output.role === 'provider' ? 'liability' : 'fee';
  const value = {
    op: 'prepare_targeted_payout',
    contract_version: ctx.version ?? CONTRACT_VERSION,
    economic_op_id: output.economic_op_id,
    rail: 'fiat',
    epoch: plan.epoch,
    epoch_apply_hash: plan.epoch_apply_hash,
    prepared_at: 90_000,
    kind,
    output_index: output.output_index,
    payload_hash: await ctx.contract.opaqueHash(
      'mayhem-targeted-payout-preparation-payload-v1',
      {
        economic_op_id: output.economic_op_id,
        rail: 'fiat',
        epoch: plan.epoch,
        epoch_apply_hash: plan.epoch_apply_hash,
        kind,
        output_index: output.output_index,
        payload,
      }
    ),
    payload,
    liability,
    external_effect_ids: [],
    admin: ctx.admin.publicKey,
    admin_sig: '0'.repeat(128),
  };
  value.admin_sig = signHex(ctx.admin.wallet, payoutPreparationMessage(value));
  const key = await ctx.contract.targetedPayoutPreparationFeatureKey(value);
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

function attemptRequest(output, attemptNo, plan) {
  const identityDigit = (
    output.output_index * 4 + attemptNo + 5
  ).toString(16);
  if (output.role === 'operator_fee') {
    return {
      processor: 'stripe',
      kind: 'platform_balance',
      destination: output.to,
      source_currency: output.source_currency,
      source_amount_minor: output.source_amount_minor,
      destination_currency: null,
      destination_amount_min_minor: null,
      destination_amount_max_minor: null,
      fx_quote_id: null,
      fx_quote_hash: null,
      transfer_group: null,
      metadata_hash: identityDigit.repeat(64),
    };
  }
  return {
    processor: 'stripe',
    kind: 'stripe_transfer',
    destination: output.to,
    source_currency: output.source_currency,
    source_amount_minor: output.source_amount_minor,
    destination_currency: output.destination_currency,
    destination_amount_min_minor: output.destination_amount_min_minor,
    destination_amount_max_minor: output.destination_amount_max_minor,
    fx_quote_id: `fxq_attempt_${plan.epoch}_${output.output_index}_${attemptNo}`,
    fx_quote_hash: (attemptNo + 6).toString(16).repeat(64),
    transfer_group: `mayhem_fiat_epoch_${plan.epoch}_${plan.epoch_apply_hash.slice(0, 16)}`,
    metadata_hash: identityDigit.repeat(64),
  };
}

async function attemptValue(ctx, plan, output, attemptNo) {
  const request = attemptRequest(output, attemptNo, plan);
  const attemptId = await ctx.contract.targetedFiatAttemptId(
    output.economic_op_id,
    attemptNo,
    request
  );
  return {
    op: 'prepare_targeted_fiat_attempt',
    contract_version: ctx.version ?? CONTRACT_VERSION,
    rail: 'fiat',
    epoch: plan.epoch,
    epoch_apply_hash: plan.epoch_apply_hash,
    plan_root: plan.plan_root,
    economic_op_id: output.economic_op_id,
    output_index: output.output_index,
    attempt_id: attemptId,
    attempt_no: attemptNo,
    prepared_at: 90_000 + attemptNo,
    quote_expires_at: output.role === 'provider' ? 90_100 + attemptNo : null,
    idempotency_key_hash:
      await ctx.contract.targetedFiatAttemptIdempotencyKeyHash(attemptId),
    request_hash: await ctx.contract.opaqueHash(
      'mayhem-targeted-fiat-attempt-request-v1',
      {
        economic_op_id: output.economic_op_id,
        attempt_id: attemptId,
        attempt_no: attemptNo,
        request,
      }
    ),
    request,
    admin: ctx.admin.publicKey,
    admin_sig: '0'.repeat(128),
  };
}

async function prepareAttempt(ctx, plan, output, attemptNo) {
  const value = await attemptValue(ctx, plan, output, attemptNo);
  return {
    value,
    result: await submitControl(ctx, value, 'targetedFiatAttemptFeatureKey'),
  };
}

function transferEvidence(output, attempt, ref) {
  if (output.role === 'operator_fee') {
    return {
      schema_version: 2,
      kind: 'platform_balance',
      attempt_id: attempt.attempt_id,
      ref,
      destination: output.to,
      source_currency: output.source_currency,
      source_amount_minor: output.source_amount_minor,
      transfer_group: null,
    };
  }
  return {
    schema_version: 2,
    kind: 'stripe_transfer',
    attempt_id: attempt.attempt_id,
    ref,
    destination: output.to,
    source_currency: output.source_currency,
    source_amount_minor: output.source_amount_minor,
    destination_currency: output.destination_currency,
    destination_amount_minor: output.destination_amount_min_minor,
    fx_quote_id: attempt.request.fx_quote_id,
    fx_quote_hash: attempt.request.fx_quote_hash,
    destination_payment: `py_${attempt.attempt_id.slice(0, 16)}`,
    transfer_group: attempt.request.transfer_group,
  };
}

function finalizationValue(ctx, attempt, status, evidence) {
  return {
    op: 'finalize_targeted_fiat_attempt',
    contract_version: ctx.version ?? CONTRACT_VERSION,
    rail: 'fiat',
    epoch: attempt.epoch,
    economic_op_id: attempt.economic_op_id,
    attempt_id: attempt.attempt_id,
    status,
    at: status === 'expired_pre_effect' ? attempt.quote_expires_at : 90_050,
    evidence,
    admin: ctx.admin.publicKey,
    admin_sig: '0'.repeat(128),
  };
}

async function finalizeAttempt(ctx, attempt, status, evidence) {
  const value = finalizationValue(ctx, attempt, status, evidence);
  return {
    value,
    result: await submitControl(
      ctx,
      value,
      'finalizeTargetedFiatAttemptFeatureKey'
    ),
  };
}

function outputSettlementValue(ctx, plan, output, attempt, transfer) {
  return {
    op: 'settle_targeted_fiat_output',
    contract_version: ctx.version ?? CONTRACT_VERSION,
    rail: 'fiat',
    epoch: plan.epoch,
    at: 90_050,
    epoch_apply_hash: plan.epoch_apply_hash,
    plan_root: plan.plan_root,
    economic_op_id: output.economic_op_id,
    output_index: output.output_index,
    preparation_id: output.economic_op_id,
    attempt_id: attempt.attempt_id,
    stripe_transfer: transfer,
    admin: ctx.admin.publicKey,
    admin_sig: '0'.repeat(128),
  };
}

async function settleOutput(ctx, plan, output, attempt, transfer) {
  const value = outputSettlementValue(ctx, plan, output, attempt, transfer);
  return {
    value,
    result: await submitControl(ctx, value, 'targetedFiatOutputFeatureKey'),
  };
}

async function closeEpoch(ctx, plan) {
  const value = {
    op: 'close_targeted_payout_epoch',
    contract_version: ctx.version ?? CONTRACT_VERSION,
    rail: 'fiat',
    epoch: plan.epoch,
    at: 90_050,
    epoch_apply_hash: plan.epoch_apply_hash,
    plan_root: plan.plan_root,
    admin: ctx.admin.publicKey,
    admin_sig: '0'.repeat(128),
  };
  return await submitControl(ctx, value, 'closeTargetedPayoutEpochFeatureKey');
}

async function setup(provider = null) {
  const admin = await makeIdentity();
  provider ??= await makeIdentity();
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
          payout_min_au: '1',
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
  const providerAu = (85n * CENT_AU).toString();
  const dustAu = (CENT_AU / 2n).toString();
  const paidAu = (85n * CENT_AU - CENT_AU / 2n).toString();
  const operatorAu = (15n * CENT_AU).toString();
  const target = 'acct_targeted_provider';
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
  await storage.put('payout/liability-index/fiat', {
    type: 'provider_payout_liability_index',
    rail: 'fiat',
    entries: [{
      provider: provider.publicKey,
      payout_revision: revision,
    }],
    updated_epoch: 1,
    updated_at: 'epoch/targeted/1/apply',
  });
  await storage.put('fee/fiat/cum', {
    rail: 'fiat',
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
  const outputs = [
    {
      economic_op_id: '1'.repeat(64),
      output_index: 0,
      role: 'provider',
      provider: provider.publicKey,
      payout_revision: revision,
      to: target,
      paid_cum_au_before: '0',
      aggregate_paid_cum_au_before: '0',
      liability_au: providerAu,
      paid_au: paidAu,
      rounding_au: dustAu,
      dust_au: dustAu,
      source_currency: 'eur',
      source_amount_minor: '79',
      destination_currency: 'gbp',
      destination_amount_min_minor: '65',
      destination_amount_max_minor: '69',
    },
    {
      economic_op_id: '2'.repeat(64),
      output_index: 1,
      role: 'operator_fee',
      to: 'platform_balance',
      liability_au: operatorAu,
      paid_au: operatorAu,
      rounding_au: '0',
      dust_au: '0',
      source_currency: 'eur',
      source_amount_minor: '14',
    },
  ];
  return { admin, provider, storage, contract, revision, outputs };
}

for (const version of new Set([27, 28, CONTRACT_VERSION])) {
test(`fiat v${version} retained plan settles independently and preserves an earlier recipient`, async () => {
  const ctx = await setup();
  const plan = await buildPlan(ctx);
  // Model a plan admitted before an upgrade. Subsequent commands still use the
  // current contract; neither its signature nor its economic identity is rewritten.
  plan.contract_version = version;
  plan.admin_sig = signHex(ctx.admin.wallet, targetedPayoutControlMessage(plan));
  const saved = (await ctx.storage.get('payout/epoch-plan/fiat/1')).value;
  await ctx.storage.put('payout/epoch-plan/fiat/1', { ...saved, value: plan });
  const providerOutput = ctx.outputs[0];
  assert.equal((await prepareEconomicOutput(ctx, plan, providerOutput)).ok, true);
  const attempt = await prepareAttempt(ctx, plan, providerOutput, 1);
  assert.equal(attempt.result.ok, true, attempt.result.message);
  const transfer = transferEvidence(providerOutput, attempt.value, 'tr_provider_1');
  assert.equal(
    (await finalizeAttempt(ctx, attempt.value, 'succeeded', transfer)).result.ok,
    true
  );
  const first = await settleOutput(ctx, plan, providerOutput, attempt.value, transfer);
  assert.equal(first.result.ok, true, first.result.message);
  assert.match((await closeEpoch(ctx, plan)).message, /unsettled planned outputs/i);

  const liabilityKey =
    `payout/liability/fiat/${ctx.provider.publicKey}/${ctx.revision}`;
  assert.equal(
    (await ctx.storage.get(liabilityKey)).value.paid_cum_au,
    providerOutput.paid_au
  );
  const retry = await submitControl(ctx, first.value, 'targetedFiatOutputFeatureKey');
  assert.equal(retry.ok, true, retry.message);
  assert.equal(retry.idempotent, true);

  const feeBefore = (await ctx.storage.get('fee/fiat/cum')).value;
  await ctx.storage.put('fee/fiat/cum', {
    ...feeBefore,
    cum_au: (BigInt(feeBefore.cum_au) + 3n * CENT_AU).toString(),
    settled_cum_au: (BigInt(feeBefore.settled_cum_au) + 3n * CENT_AU).toString(),
    updated_epoch: 2,
  });

  const feeOutput = ctx.outputs[1];
  assert.equal((await prepareEconomicOutput(ctx, plan, feeOutput)).ok, true);
  const feeAttempt = await prepareAttempt(ctx, plan, feeOutput, 1);
  assert.equal(feeAttempt.result.ok, true, feeAttempt.result.message);
  const feeTransfer = transferEvidence(
    feeOutput,
    feeAttempt.value,
    'platform_balance:targeted:1'
  );
  assert.equal(
    (await finalizeAttempt(ctx, feeAttempt.value, 'succeeded', feeTransfer)).result.ok,
    true
  );
  assert.equal(
    (await settleOutput(ctx, plan, feeOutput, feeAttempt.value, feeTransfer)).result.ok,
    true
  );
  const feeAfter = (await ctx.storage.get('fee/fiat/cum')).value;
  assert.equal(feeAfter.swept_cum_au, feeOutput.paid_au);
  assert.equal(
    BigInt(feeAfter.cum_au) - BigInt(feeAfter.swept_cum_au),
    3n * CENT_AU
  );
  assert.equal((await closeEpoch(ctx, plan)).ok, true);
  assert.equal((await ctx.storage.get('payout/epoch-plan/fiat/1')).value.value.contract_version, version);
});
}

test('fiat attempt renewal requires definitive expiry and never follows ambiguity or success', async () => {
  const ctx = await setup();
  const plan = await buildPlan(ctx);
  const output = ctx.outputs[0];
  assert.equal((await prepareEconomicOutput(ctx, plan, output)).ok, true);

  const first = await prepareAttempt(ctx, plan, output, 1);
  assert.equal(first.result.ok, true, first.result.message);
  const ambiguousReplacement = await prepareAttempt(ctx, plan, output, 2);
  assert.match(
    ambiguousReplacement.result.message,
    /only after definitive pre-effect expiry/i
  );

  const expiryEvidence = {
    fx_quote_id: first.value.request.fx_quote_id,
    fx_quote_hash: first.value.request.fx_quote_hash,
    quote_expires_at: first.value.quote_expires_at,
    error_code: 'fx_quote_expired',
    external_effect_absent: true,
  };
  assert.equal(
    (
      await finalizeAttempt(
        ctx,
        first.value,
        'expired_pre_effect',
        expiryEvidence
      )
    ).result.ok,
    true
  );
  const second = await prepareAttempt(ctx, plan, output, 2);
  assert.equal(second.result.ok, true, second.result.message);
  const transfer = transferEvidence(output, second.value, 'tr_provider_renewed');
  assert.equal(
    (await finalizeAttempt(ctx, second.value, 'succeeded', transfer)).result.ok,
    true
  );
  const forbiddenThird = await prepareAttempt(ctx, plan, output, 3);
  assert.match(forbiddenThird.result.message, /only after definitive pre-effect expiry/i);
});

test('fiat attempt identity and idempotency hash are derived from the canonical request', async () => {
  const ctx = await setup();
  const plan = await buildPlan(ctx);
  const output = ctx.outputs[0];
  assert.equal((await prepareEconomicOutput(ctx, plan, output)).ok, true);

  const canonical = await attemptValue(ctx, plan, output, 1);
  const wrongAttemptId = 'f'.repeat(64);
  const tamperedAttempt = {
    ...canonical,
    attempt_id: wrongAttemptId,
    idempotency_key_hash:
      await ctx.contract.targetedFiatAttemptIdempotencyKeyHash(wrongAttemptId),
    request_hash: await ctx.contract.opaqueHash(
      'mayhem-targeted-fiat-attempt-request-v1',
      {
        economic_op_id: canonical.economic_op_id,
        attempt_id: wrongAttemptId,
        attempt_no: canonical.attempt_no,
        request: canonical.request,
      }
    ),
  };
  const attemptError =
    await ctx.contract.normalizePrepareTargetedFiatAttemptValue(tamperedAttempt);
  assert.match(attemptError.message, /attempt id does not match/i);

  const tamperedIdempotency = {
    ...canonical,
    idempotency_key_hash: 'e'.repeat(64),
  };
  const idempotencyError =
    await ctx.contract.normalizePrepareTargetedFiatAttemptValue(
      tamperedIdempotency
    );
  assert.match(idempotencyError.message, /idempotency key hash does not match/i);

  const accepted =
    await ctx.contract.normalizePrepareTargetedFiatAttemptValue(canonical);
  assert.equal(accepted instanceof Error, false, accepted.message);
});

test('fiat success evidence is bound to its exact canonical attempt', async () => {
  const ctx = await setup();
  const plan = await buildPlan(ctx);
  const providerOutput = ctx.outputs[0];
  const providerAttempt = await attemptValue(ctx, plan, providerOutput, 1);
  const providerEvidence = transferEvidence(
    providerOutput,
    providerAttempt,
    'tr_attempt_bound'
  );

  const missingAttempt = { ...providerEvidence };
  delete missingAttempt.attempt_id;
  const missingError = ctx.contract.normalizeFinalizeTargetedFiatAttemptValue(
    finalizationValue(ctx, providerAttempt, 'succeeded', missingAttempt)
  );
  assert.match(missingError.message, /attempt id does not match/i);

  const mismatchedEvidence = {
    ...providerEvidence,
    attempt_id: 'f'.repeat(64),
  };
  const mismatchError = ctx.contract.normalizeFinalizeTargetedFiatAttemptValue(
    finalizationValue(ctx, providerAttempt, 'succeeded', mismatchedEvidence)
  );
  assert.match(mismatchError.message, /attempt id does not match/i);

  const settlementError =
    ctx.contract.normalizeTargetedFiatOutputSettlementValue(
      outputSettlementValue(
        ctx,
        plan,
        providerOutput,
        providerAttempt,
        mismatchedEvidence
      )
    );
  assert.match(settlementError.message, /attempt id does not match/i);

  const operatorOutput = ctx.outputs[1];
  const operatorAttempt = await attemptValue(ctx, plan, operatorOutput, 1);
  const operatorEvidence = {
    ...transferEvidence(
      operatorOutput,
      operatorAttempt,
      'platform_balance:attempt_bound'
    ),
    attempt_id: 'e'.repeat(64),
  };
  const operatorError = ctx.contract.normalizeFinalizeTargetedFiatAttemptValue(
    finalizationValue(ctx, operatorAttempt, 'succeeded', operatorEvidence)
  );
  assert.match(operatorError.message, /attempt id does not match/i);

  const accepted = ctx.contract.normalizeFinalizeTargetedFiatAttemptValue(
    finalizationValue(ctx, providerAttempt, 'succeeded', providerEvidence)
  );
  assert.equal(accepted instanceof Error, false, accepted.message);
});

test('fiat epoch plan rejects rounded operator fees before settlement', async () => {
  const ctx = await setup();
  const operator = ctx.outputs[1];
  ctx.outputs[1] = {
    ...operator,
    paid_au: (BigInt(operator.liability_au) - 1n).toString(),
    rounding_au: '1',
    dust_au: '1',
  };
  const roundedPlan = await planValue(ctx);
  const error =
    await ctx.contract.normalizeTargetedPayoutEpochValue(roundedPlan);
  assert.match(error.message, /operator fee must retain its exact AU liability/i);
});

test('fiat attempt finalization rejects a globally duplicated external effect', async () => {
  const ctx = await setup();
  const plan = await buildPlan(ctx);
  const output = ctx.outputs[0];
  assert.equal((await prepareEconomicOutput(ctx, plan, output)).ok, true);
  const attempt = await prepareAttempt(ctx, plan, output, 1);
  assert.equal(attempt.result.ok, true, attempt.result.message);
  const transfer = transferEvidence(output, attempt.value, 'tr_duplicate_effect');
  await ctx.storage.put(
    ctx.contract.targetedFiatAttemptEffectKey(transfer.ref),
    { economic_op_id: 'f'.repeat(64), consumed: false }
  );
  const finalized = await finalizeAttempt(ctx, attempt.value, 'succeeded', transfer);
  assert.match(finalized.result.message, /external effect was already finalized/i);
});

const overlapStart = '8028518251457218441';
const paid508 = '1021701396860965955';
const paid542 = '1877278632791707922';

async function freezeFiatEpoch(ctx, epoch, anchor, outputs) {
  const update = `epoch/targeted/${epoch}/apply`;
  await ctx.storage.put(`epoch/apply-anchor/${epoch}`, {
    type: 'epoch_apply_anchor', epoch, apply_hash: anchor, settlement_unix: 90_000, applied_at: update
  });
  await ctx.storage.put('epoch/apply/state', {
    updated_epoch: epoch, updated_at: update, last_apply_hash: anchor, last_settlement_unix: 90_000
  });
  ctx.outputs = outputs;
  const value = await planValue(ctx, epoch, anchor);
  const r = await submitControl(ctx, value, 'targetedPayoutEpochFeatureKey');
  assert.equal(r.ok, true, r.message);
  return value;
}

async function overlapFixture(withAttempt = true) {
  const people = (await Promise.all([makeIdentity(), makeIdentity()]))
    .sort((a, b) => a.publicKey.localeCompare(b.publicKey));
  const ctx = await setup(people[1]);
  const { storage, provider, revision } = ctx;
  const proto = { peer: { wallet: makeVerifier(ctx.admin.wallet) } };
  ctx.contract = new Contract27(proto, {});
  ctx.version = 27;
  const get = async k => (await storage.get(k))?.value ?? null;
  const earningKey = `earn/fiat/${provider.publicKey}`;
  const liabilityKey = `payout/liability/fiat/${provider.publicKey}/${revision}`;
  const fee = await get('fee/fiat/cum');
  const oldFee = {
    ...ctx.outputs[1], economic_op_id: '4'.repeat(64),
    paid_au: '9473747554087500', liability_au: '9473747554087500', source_amount_minor: '1'
  };
  await storage.put('fee/fiat/cum', { ...fee, cum_au: oldFee.paid_au, settled_cum_au: oldFee.paid_au });
  const old = {
    ...ctx.outputs[0], paid_cum_au_before: overlapStart, aggregate_paid_cum_au_before: overlapStart,
    destination_currency: 'eur', source_currency: 'eur', source_amount_minor: '89',
    destination_amount_min_minor: '89', destination_amount_max_minor: '89',
    paid_au: paid508, liability_au: '1028501115410039184', dust_au: '6799718549073229', rounding_au: '6799718549073229'
  };
  const bindingKey = `payout/binding/fiat/${provider.publicKey}/${revision}`;
  await storage.put(bindingKey, { ...await get(bindingKey), currency: 'eur' });
  async function updateBalances(epoch, out) {
    const total = String(BigInt(overlapStart) + BigInt(out.liability_au));
    for (const k of [earningKey, liabilityKey]) {
      await storage.put(k, {
        ...await get(k), ...(k === liabilityKey ? { currency: 'eur' } : {}),
        paid_cum_au: overlapStart, total_au: total, updated_epoch: epoch
      });
    }
  }
  await updateBalances(508, old);
  const prior = await freezeFiatEpoch(ctx, 508, 'a'.repeat(64), [old, oldFee]);
  const pre = await prepareEconomicOutput(ctx, prior, old);
  assert.equal(pre.ok, true, pre.message);
  const attempt = withAttempt ? await prepareAttempt(ctx, prior, old, 1) : null;
  if (attempt) assert.equal(attempt.result.ok, true, attempt.result.message);
  ctx.contract = new Contract28(proto, {});
  ctx.version = 28;
  const first = {
    ...old, economic_op_id: '2'.repeat(64), provider: people[0].publicKey, to: 'acct_fixture_other',
    source_amount_minor: '678', destination_amount_min_minor: '678', destination_amount_max_minor: '678',
    paid_cum_au_before: '0', aggregate_paid_cum_au_before: '0', paid_au: '7760944591663280315',
    liability_au: '7767070390800905355', dust_au: '6125799137625040', rounding_au: '6125799137625040'
  };
  const next = {
    ...old, economic_op_id: '3'.repeat(64), output_index: 1, source_amount_minor: '164',
    destination_amount_min_minor: '164', destination_amount_max_minor: '164', paid_au: paid542,
    liability_au: '1882503406878845184', dust_au: '5224774087137262', rounding_au: '5224774087137262'
  };
  await updateBalances(542, next);
  for (const key of [`prov/${provider.publicKey}`, earningKey, liabilityKey, bindingKey]) {
    const row = { ...await get(key) };
    if ('provider' in row) row.provider = first.provider;
    if ('target' in row) row.target = first.to;
    if ('total_au' in row) Object.assign(row, { total_au: first.liability_au, paid_cum_au: '0' });
    await storage.put(key.replace(provider.publicKey, first.provider), row);
  }
  const index = await get('payout/liability-index/fiat');
  await storage.put('payout/liability-index/fiat', {
    ...index, entries: [
      { provider: first.provider, payout_revision: revision }, ...index.entries]
  });
  const retainedFee = {
    ...oldFee, output_index: 2, economic_op_id: '5'.repeat(64), source_amount_minor: '58',
    paid_au: '654219431548440044', liability_au: '654219431548440044'
  };
  await storage.put('fee/fiat/cum', { ...fee, cum_au: retainedFee.paid_au, settled_cum_au: retainedFee.paid_au });
  const frozen = await freezeFiatEpoch(ctx, 542, 'b'.repeat(64), [first, next, retainedFee]);
  assert.equal((await prepareEconomicOutput(ctx, frozen, first)).ok, true);
  const paidAttempt = await prepareAttempt(ctx, frozen, first, 1);
  assert.equal(paidAttempt.result.ok, true, paidAttempt.result.message);
  const paidTransfer = transferEvidence(first, paidAttempt.value, 'tr_other_output');
  assert.equal((await finalizeAttempt(ctx, paidAttempt.value, 'succeeded', paidTransfer)).result.ok, true);
  assert.equal((await settleOutput(ctx, frozen, first, paidAttempt.value, paidTransfer)).result.ok, true);
  assert.equal((await prepareEconomicOutput(ctx, frozen, retainedFee)).ok, true);
  const feeAttempt = await prepareAttempt(ctx, frozen, retainedFee, 1);
  assert.equal(feeAttempt.result.ok, true, feeAttempt.result.message);
  const feeEvidence = transferEvidence(retainedFee, feeAttempt.value, 'platform_balance:fixture:542');
  assert.equal((await finalizeAttempt(ctx, feeAttempt.value, 'succeeded', feeEvidence)).result.ok, true);
  assert.equal((await settleOutput(ctx, frozen, retainedFee, feeAttempt.value, feeEvidence)).result.ok, true);
  const blocked = await prepareEconomicOutput(ctx, frozen, next);
  assert.match(blocked.message, /watermark/);
  ctx.contract = new MayhemContract(proto, {});
  delete ctx.version;
  return { ctx, old, next, first, prior, frozen, attempt, earningKey, liabilityKey, proto };
}

function stateReader(ctx) {
  const c = new MayhemContract(ctx.contract.protocol, {});
  c.get = async k => (await ctx.storage.get(k))?.value ?? null;
  return c;
}

async function reviewedFeature(ctx, stage, transfer = null) {
  const length = stage === 'fence' ? 1000 : 1001;
  const c = stateReader(ctx);
  const state = await inspectOverlap(c, length);
  const digest = '5'.repeat(64);
  const review = {
    old_operation: state.old_operation, new_operation: state.new_operation, admin: state.admin,
    state_hash: state.state_hash, signed_length: length, stage, at: stage === 'fence' ? 100000 : 200000,
    attestation: stage === 'fence' ? null : {
      fence_key: state.fence_key, sends_drained_at: 110000,
      history_from: 0, history_through: 199999, destination_history_complete: true, in_flight_requests: 0,
      verified_sibling_ref: state.verified_sibling_ref, retained_fee_ref: state.retained_fee_ref, old_effect: transfer?.ref ?? 'absent', new_effect: 'absent', evidence_sha256: digest
    },
    ...(transfer ? { stripe_transfer: transfer } : {}),
  };
  // Capture explicit nulls and populated rows exactly as the pinned state RPC returns them.
  // First collect all reads on an independent in-memory copy for the offline preflight.
  const rows = new Map();
  const mem = MemoryStorage.fromSnapshotBytes(ctx.storage.snapshotBytes());
  c.get = async k => {
    const value = (await mem.get(k))?.value ?? null;
    if (!rows.has(k)) rows.set(k, { key: k, confirmed: true, signed_length: length, value });
    return value;
  };
  c.put = (k, v) => mem.put(k, v);
  const sign = async message => signHex(ctx.admin.wallet, message);
  const evidence = stage === 'fence' ? null : digest;
  await buildRecoveryFeature(c, length, review, evidence, sign);
  const snapshot = { signed_length: length, rows: [...rows.values()] };
  const recreated = await buildRecoveryFeature(snapshotContract(snapshot), length, review, evidence, sign);
  assert.throws(() => snapshotContract({ ...snapshot, rows: [...snapshot.rows, snapshot.rows[0]] }), /Duplicate/);
  await assert.rejects(buildRecoveryFeature(snapshotContract(snapshot), length, { ...review, state_hash: '0'.repeat(64) }, evidence, sign), /Review/);
  return recreated;
}

async function submitRecovery(ctx, feature, sender = ctx.admin.publicKey) {
  return executeFeature(ctx.contract, ctx.storage, 'mayhem_feature', feature.key, feature.value, sender);
}
async function resignRecovery(ctx, feature, change) {
  const value = structuredClone(feature.value);
  change(value);
  value.admin_sig = signHex(ctx.admin.wallet, targetedPayoutControlMessage(value));
  return { value, key: await fiatRecoveryFeatureKey(ctx.contract, value) };
}

test('508/542 recovers its reservation and permits subsequent provider and fee payouts', async () => {
  const { ctx, old, next, first, prior, frozen, attempt, earningKey, liabilityKey, proto } = await overlapFixture();
  const read = async k => (await ctx.storage.get(k))?.value ?? null;
  const originalRows = new Map(ctx.storage.values);
  const firstPaidKey = ctx.contract.targetedPayoutOutputRecordKey('fiat', 542, first.economic_op_id);
  const oldPaidKey = ctx.contract.targetedPayoutOutputRecordKey('fiat', 508, old.economic_op_id);
  const fence = await reviewedFeature(ctx, 'fence');
  assert.match((await submitRecovery(ctx, fence, first.provider)).message, /admin|authority/i);
  assert.equal((await submitRecovery(ctx, fence)).ok, true);
  assert.equal((await submitRecovery(ctx, fence)).idempotent, true);
  assert.match((await prepareAttempt(ctx, prior, old, 2)).result.message, /fenced/);
  const uncertain = transferEvidence(old, attempt.value, 'tr_uncertain');
  assert.match((await finalizeAttempt(ctx, attempt.value, 'succeeded', uncertain)).result.message, /fenced/);
  const repair = await reviewedFeature(ctx, 'supersede_absent');
  for (const change of [
    v => { v.attestation.in_flight_requests = 1; },
    v => { v.attestation.destination_history_complete = false; },
    v => { v.attestation.new_effect = 'unknown'; },
    v => { v.attestation.old_effect = 'unknown'; },
    v => { v.attestation.history_from = 100000; },
    v => { v.instruction.payload.output.paid_au = paid508; },
    v => { v.state_hash = '0'.repeat(64); },
  ]) {
    const invalid = await resignRecovery(ctx, repair, change);
    assert.ok((await submitRecovery(ctx, invalid)) instanceof Error);
    assert.equal(await read(fiatResolutionKey(old.economic_op_id)), null);
  }
  assert.equal((await submitRecovery(ctx, repair)).ok, true);
  ctx.storage = MemoryStorage.fromSnapshotBytes(ctx.storage.snapshotBytes());
  ctx.contract = new MayhemContract(proto, {});
  assert.equal((await submitRecovery(ctx, repair)).idempotent, true);
  for (const [k, v] of originalRows) {
    if (k.startsWith('earn/') || k.startsWith('payout/liability/') || k.startsWith('fee/') ||
      k.startsWith('settle/') || k.startsWith('payout/epoch-plan/') ||
      k.includes(first.economic_op_id) || k.includes('tr_other_output')) assert.deepEqual(await read(k), v, k);
  }
  assert.equal(await read(oldPaidKey), null);
  assert.match((await closeEpoch(ctx, prior)).message, /unsettled/);
  assert.match((await closeEpoch(ctx, frozen)).message, /unsettled/);
  const replacement = await prepareAttempt(ctx, frozen, next, 1);
  assert.equal(replacement.result.ok, true, replacement.result.message);
  const t = transferEvidence(next, replacement.value, 'tr_fixture_new');
  assert.equal((await finalizeAttempt(ctx, replacement.value, 'succeeded', t)).result.ok, true);
  assert.equal((await settleOutput(ctx, frozen, next, replacement.value, t)).result.ok, true);
  assert.equal((await closeEpoch(ctx, frozen)).ok, true);
  assert.match((await closeEpoch(ctx, prior)).message, /unsettled/);
  assert.equal(await read('settle/targeted/fiat/508'), null);
  assert.equal(await read(oldPaidKey), null);
  for (const k of [earningKey, liabilityKey]) assert.equal((await read(k)).paid_cum_au, String(BigInt(overlapStart) + BigInt(paid542)));
  assert.deepEqual(await read(firstPaidKey), originalRows.get(firstPaidKey));
  assert.equal((await submitRecovery(ctx, repair)).idempotent, true);

  // Both providers earn again on the same revisions while the old 508 fee remains fenced.
  const newEarnings = 100n * CENT_AU;
  const expectedBalances = new Map();
  const providerOutputs = [];
  for (const original of [first, next]) {
    const providerKey = `earn/fiat/${original.provider}`;
    const revisionKey = ctx.contract.providerPayoutLiabilityKey(original.provider, 'fiat', original.payout_revision);
    const revision = await read(revisionKey);
    const aggregate = await read(providerKey);
    for (const [key, balance] of [[revisionKey, revision], [providerKey, aggregate]]) {
      expectedBalances.set(key, String(BigInt(balance.paid_cum_au) + newEarnings));
      await ctx.storage.put(key, {
        ...balance, updated_epoch: 543, total_au: String(BigInt(balance.total_au) + newEarnings),
      });
    }
    providerOutputs.push({
      ...original, economic_op_id: (6 + original.output_index).toString().repeat(64),
      paid_cum_au_before: revision.paid_cum_au, aggregate_paid_cum_au_before: aggregate.paid_cum_au,
      liability_au: String(newEarnings + BigInt(original.dust_au)), paid_au: String(newEarnings),
      source_amount_minor: '88', destination_amount_min_minor: '88', destination_amount_max_minor: '88',
    });
  }
  const newFee = 10n * CENT_AU;
  const fee = await read('fee/fiat/cum');
  await ctx.storage.put('fee/fiat/cum', {
    ...fee, updated_epoch: 543,
    cum_au: String(BigInt(fee.cum_au) + newFee),
    settled_cum_au: String(BigInt(fee.settled_cum_au) + newFee),
  });
  const feeOutput = {
    ...frozen.outputs[2], economic_op_id: '8'.repeat(64),
    liability_au: String(newFee), paid_au: String(newFee), source_amount_minor: '9',
  };
  const later = await freezeFiatEpoch(ctx, 543, 'c'.repeat(64), [...providerOutputs, feeOutput]);
  for (const output of later.outputs) {
    const ref = output.role === 'operator_fee' ? 'platform_balance:fixture:543' : 'tr_fixture_later_' + output.output_index;
    const prepared = await prepareEconomicOutput(ctx, later, output);
    assert.equal(prepared.ok, true, prepared.message);
    const attempt = await prepareAttempt(ctx, later, output, 1);
    assert.equal(attempt.result.ok, true, attempt.result.message);
    const evidence = transferEvidence(output, attempt.value, ref);
    const finalized = await finalizeAttempt(ctx, attempt.value, 'succeeded', evidence);
    assert.equal(finalized.result.ok, true, finalized.result.message);
    const accounted = await settleOutput(ctx, later, output, attempt.value, evidence);
    assert.equal(accounted.result.ok, true, accounted.result.message);
  }
  const closed = await closeEpoch(ctx, later);
  assert.equal(closed.ok, true, closed.message);
  assert.equal((await submitRecovery(ctx, repair)).idempotent, true);
  for (const [key, amount] of expectedBalances) assert.equal((await read(key)).paid_cum_au, amount);
  assert.equal((await read('fee/fiat/cum')).swept_cum_au, String(BigInt(fee.swept_cum_au) + newFee));
  assert.equal(await read('settle/targeted/fiat/508'), null);
  assert.deepEqual(await read(firstPaidKey), originalRows.get(firstPaidKey));
});

test('508 fence holds the original fee even if a runner already prepared or finalized its retention', async () => {
  const { ctx, prior } = await overlapFixture();
  const fee = prior.outputs[1];
  assert.equal((await prepareEconomicOutput(ctx, prior, fee)).ok, true);
  const attempt = await prepareAttempt(ctx, prior, fee, 1);
  assert.equal(attempt.result.ok, true);
  const evidence = transferEvidence(fee, attempt.value, 'platform_balance:fixture:508');
  const saved = ctx.storage.snapshotBytes();
  for (const finalized of [false, true]) {
    ctx.storage = MemoryStorage.fromSnapshotBytes(saved);
    if (finalized) assert.equal((await finalizeAttempt(ctx, attempt.value, 'succeeded', evidence)).result.ok, true);
    const feeKey = ctx.contract.feeCumKey('fiat');
    const before = await ctx.storage.get(feeKey);
    assert.equal((await submitRecovery(ctx, await reviewedFeature(ctx, 'fence'))).ok, true);
    assert.match((await prepareEconomicOutput(ctx, prior, fee)).message, /holds the 508 fee/);
    assert.match((await prepareAttempt(ctx, prior, fee, 1)).result.message, /holds the 508 fee/);
    assert.match((await finalizeAttempt(ctx, attempt.value, 'succeeded', evidence)).result.message, /holds the 508 fee/);
    assert.match((await settleOutput(ctx, prior, fee, attempt.value, evidence)).result.message, /holds the 508 fee/);
    assert.equal((await submitRecovery(ctx, await reviewedFeature(ctx, 'supersede_absent'))).ok, true);
    assert.match((await settleOutput(ctx, prior, fee, attempt.value, evidence)).result.message, /holds the 508 fee/);
    assert.deepEqual(await ctx.storage.get(feeKey), before);
  }
});

test('missing last 508 attempt cannot authorize a fence or absence resolution', async () => {
  const { ctx } = await overlapFixture(false);
  await assert.rejects(reviewedFeature(ctx, 'fence'), /missing last 508 attempt/);
});

test('verified 508 effect follows its original attempt and leaves frozen 542 blocked', async () => {
  const { ctx, old, next, prior, frozen, attempt, earningKey } = await overlapFixture();
  assert.equal((await submitRecovery(ctx, await reviewedFeature(ctx, 'fence'))).ok, true);
  const transfer = transferEvidence(old, attempt.value, 'tr_fixture_observed');
  const feature = await reviewedFeature(ctx, 'reconcile_verified', transfer);
  assert.equal((await submitRecovery(ctx, feature)).ok, true);
  assert.equal((await submitRecovery(ctx, feature)).idempotent, true);
  assert.equal((await ctx.storage.get(earningKey)).value.paid_cum_au, overlapStart);
  const reader = stateReader(ctx);
  const localWrites = new Map();
  const read = reader.get;
  reader.get = k => localWrites.has(k) ? localWrites.get(k) : read(k);
  reader.put = (k, v) => localWrites.set(k, v);
  const paid = await buildVerifiedSettlement(reader, 1002, feature, async m => signHex(ctx.admin.wallet, m));
  const submitPaid = () => executeFeature(ctx.contract, ctx.storage, 'mayhem_feature', paid.key, paid.value, ctx.admin.publicKey);
  assert.equal((await submitPaid()).ok, true);
  assert.equal((await submitPaid()).idempotent, true);
  assert.equal((await ctx.storage.get(earningKey)).value.paid_cum_au, String(BigInt(overlapStart) + BigInt(paid508)));
  assert.match((await prepareEconomicOutput(ctx, frozen, next)).message, /watermark/);
  assert.match((await closeEpoch(ctx, frozen)).message, /unsettled/);
});

test('recovery rejects contradictory legacy state and a changed last observation', async () => {
  const { ctx, old, next, prior, attempt, first } = await overlapFixture();
  const saved = ctx.storage.snapshotBytes();
  const feature = await reviewedFeature(ctx, 'fence');
  const id = old.economic_op_id;
  const attemptKey = ctx.contract.targetedFiatAttemptRecordKey(id, attempt.value.attempt_id);
  const tailKey = ctx.contract.targetedFiatAttemptTailKey(id);
  const lockKey = ctx.contract.payoutPreparationLiabilityLockKey('fiat', old);
  const reservationKey = ctx.contract.payoutPreparationAggregateTailKey('fiat', old.provider);
  const siblingKey = ctx.contract.targetedPayoutOutputRecordKey('fiat', 542, first.economic_op_id);
  for (const [key, patch] of [
    [tailKey, { attempt_id: '4'.repeat(64) }],
    [attemptKey, { consumed: true }],
    [attemptKey, { status: 'prepared', result: { status: 'succeeded' } }],
    [attemptKey, { preparation: { ...attempt.value, request_hash: 'e'.repeat(64) } }],
    [lockKey, { economic_op_id: next.economic_op_id }],
    [reservationKey, { paid_cum_au_after: overlapStart }],
    [siblingKey, { economic_op_id: next.economic_op_id }],
    [ctx.contract.targetedFiatAttemptTailKey(next.economic_op_id), { status: 'prepared' }],
    [ctx.contract.payoutPreparationRecordKey('fiat', id), { external_effect_ids: ['9'.repeat(64)] }],
    [ctx.contract.targetedFiatAttemptEffectKey('tr_other_output'), { consumed: false }],
  ]) {
    ctx.storage = MemoryStorage.fromSnapshotBytes(saved);
    const before = await ctx.storage.get(key);
    await ctx.storage.put(key, { ...before?.value, ...patch });
    assert.ok((await submitRecovery(ctx, feature)) instanceof Error, key);
    assert.equal(await ctx.storage.get(fiatFenceKey(id)), null);
  }
  // A definitive expiry plus renewal changes the signed observation. Neither
  // the old fence nor an absence instruction for the expired attempt can win.
  ctx.storage = MemoryStorage.fromSnapshotBytes(saved);
  const p = attempt.value;
  const expiry = {
    fx_quote_id: p.request.fx_quote_id, fx_quote_hash: p.request.fx_quote_hash,
    quote_expires_at: p.quote_expires_at, error_code: 'fx_quote_expired', external_effect_absent: true
  };
  assert.equal((await finalizeAttempt(ctx, p, 'expired_pre_effect', expiry)).result.ok, true);
  const expired = await reviewedFeature(ctx, 'fence');
  const replacement = await prepareAttempt(ctx, prior, old, 2);
  assert.equal(replacement.result.ok, true);
  await assert.rejects(reviewedFeature(ctx, 'fence'), /additional 508 attempts/);
  assert.match((await submitRecovery(ctx, expired)).message, /observation/);
  assert.match((await submitRecovery(ctx, feature)).message, /observation/);
  const verified = transferEvidence(old, replacement.value, 'tr_fixture_already_found');
  assert.equal((await finalizeAttempt(ctx, replacement.value, 'succeeded', verified)).result.ok, true);
  assert.ok((await submitRecovery(ctx, feature)) instanceof Error);
  assert.equal(await ctx.storage.get(fiatFenceKey(id)), null);
});

test('conflicting evidence and a bad nested signature cannot release either reservation', async () => {
  const { ctx, old, next, attempt } = await overlapFixture();
  assert.equal((await submitRecovery(ctx, await reviewedFeature(ctx, 'fence'))).ok, true);
  const absent = await reviewedFeature(ctx, 'supersede_absent');
  const signedIncorrectly = await resignRecovery(ctx, absent, v => { v.instruction.admin_sig = '0'.repeat(128); });
  assert.match((await submitRecovery(ctx, signedIncorrectly)).message, /signature/);
  const changedSibling = await resignRecovery(ctx, absent, v => { v.attestation.verified_sibling_ref = 'tr_wrong_output'; });
  assert.match((await submitRecovery(ctx, changedSibling)).message, /evidence/);
  const transfer = transferEvidence(old, attempt.value, 'tr_verified_fixture');
  const positive = await reviewedFeature(ctx, 'reconcile_verified', transfer);
  await ctx.storage.put(ctx.contract.stripeDestinationPaymentSeenKey(transfer), { economic_op_id: 'e'.repeat(64) });
  assert.match((await submitRecovery(ctx, positive)).message, /already consumed/);
  assert.equal(await ctx.storage.get(ctx.contract.targetedFiatAttemptEffectKey(transfer.ref)), null);
  assert.equal(await ctx.storage.get(fiatResolutionKey(old.economic_op_id)), null);
  assert.equal(await ctx.storage.get(ctx.contract.payoutPreparationRecordKey('fiat', next.economic_op_id)), null);
});

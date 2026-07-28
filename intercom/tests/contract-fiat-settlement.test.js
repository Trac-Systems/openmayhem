import assert from 'node:assert/strict';
import test from 'node:test';
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

async function planValue(ctx) {
  const outputsRoot = await ctx.contract.opaqueHash(
    'mayhem-targeted-payout-epoch-outputs-v1',
    { rail: 'fiat', epoch: 1, outputs: ctx.outputs }
  );
  const carryRoot = await ctx.contract.opaqueHash(
    'mayhem-targeted-payout-epoch-carry-v1',
    { rail: 'fiat', epoch: 1, carry: [] }
  );
  const planRoot = await ctx.contract.opaqueHash(
    'mayhem-targeted-payout-epoch-plan-v1',
    {
      rail: 'fiat',
      epoch: 1,
      at: 90_000,
      epoch_apply_hash: applyHash,
      snapshot_signed_length: 220,
      outcome: 'payouts',
      outputs_root: outputsRoot,
      carry_root: carryRoot,
    }
  );
  return {
    op: 'prepare_targeted_payout_epoch',
    contract_version: CONTRACT_VERSION,
    rail: 'fiat',
    epoch: 1,
    at: 90_000,
    epoch_apply_hash: applyHash,
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
    epoch: 1,
    epoch_apply_hash: applyHash,
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
    contract_version: CONTRACT_VERSION,
    economic_op_id: output.economic_op_id,
    rail: 'fiat',
    epoch: 1,
    epoch_apply_hash: applyHash,
    prepared_at: 90_000,
    kind,
    output_index: output.output_index,
    payload_hash: await ctx.contract.opaqueHash(
      'mayhem-targeted-payout-preparation-payload-v1',
      {
        economic_op_id: output.economic_op_id,
        rail: 'fiat',
        epoch: 1,
        epoch_apply_hash: applyHash,
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

function attemptRequest(output, attemptNo) {
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
    fx_quote_id: `fxq_attempt_${attemptNo}`,
    fx_quote_hash: (attemptNo + 6).toString(16).repeat(64),
    transfer_group: `mayhem_fiat_epoch_1_${applyHash.slice(0, 16)}`,
    metadata_hash: identityDigit.repeat(64),
  };
}

async function attemptValue(ctx, plan, output, attemptNo) {
  const request = attemptRequest(output, attemptNo);
  const attemptId = await ctx.contract.targetedFiatAttemptId(
    output.economic_op_id,
    attemptNo,
    request
  );
  return {
    op: 'prepare_targeted_fiat_attempt',
    contract_version: CONTRACT_VERSION,
    rail: 'fiat',
    epoch: 1,
    epoch_apply_hash: applyHash,
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
    contract_version: CONTRACT_VERSION,
    rail: 'fiat',
    epoch: 1,
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
    contract_version: CONTRACT_VERSION,
    rail: 'fiat',
    epoch: 1,
    at: 90_050,
    epoch_apply_hash: applyHash,
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
    contract_version: CONTRACT_VERSION,
    rail: 'fiat',
    epoch: 1,
    at: 90_050,
    epoch_apply_hash: applyHash,
    plan_root: plan.plan_root,
    admin: ctx.admin.publicKey,
    admin_sig: '0'.repeat(128),
  };
  return await submitControl(ctx, value, 'closeTargetedPayoutEpochFeatureKey');
}

async function setup() {
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

test('fiat outputs settle independently and an earlier recipient survives later failure', async () => {
  const ctx = await setup();
  const plan = await buildPlan(ctx);
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
  assert.equal((await closeEpoch(ctx, plan)).ok, true);
});

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

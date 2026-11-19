import { reviewedPayoutCountersMatch } from './payout-reservations.js';

// This transition addresses the overlapping frozen provider outputs in 508/542.
// All reads and signatures are deterministic. External observations are the
// canonical admin's attestations, never inferred from the absence of a ledger row.
export const fiatFenceKey = id => `payout/recovery/fiat/${id}/fence`;
export const fiatResolutionKey = id => `payout/recovery/fiat/${id}/resolution`;
const hash = value => typeof value === 'string' && /^[0-9a-f]{64}$/.test(value);
const time = value => Number.isSafeInteger(value) && value > 0;
const equal = (a, b) => {
  if (a === b) return true;
  if (!a || !b || typeof a !== 'object' || typeof b !== 'object' ||
    Array.isArray(a) !== Array.isArray(b)) return false;
  return Object.keys(a).length === Object.keys(b).length &&
    Object.keys(a).every(k => Object.hasOwn(b, k) && equal(a[k], b[k]));
};
const knownVersion = value => Number.isSafeInteger(value) && value >= 17 && value <= 31;
const fail = message => new Error(`FIAT 508/542 recovery: ${message}`);

// The fee has no frozen starting watermark. Once its sibling is fenced,
// a retained platform-balance attempt must never advance the 508 fee sweep.
export async function fiat508FeeHeld(contract, economicOpId, plan) {
  const record = plan ?? await contract.get(contract.targetedPayoutEpochPlanRecordKey('fiat', 508));
  const outputs = record?.value?.outputs;
  return outputs?.[1]?.role === 'operator_fee' && outputs[1].economic_op_id === economicOpId &&
    (!hash(outputs[0]?.economic_op_id) || await contract.get(fiatFenceKey(outputs[0].economic_op_id)) !== null);
}

// The other transfer in the 542 group belongs to output zero. Bind its paid
// receipt and global effect lock so it cannot serve as evidence for output one.
async function paidSibling(contract, plan, version) {
  const output = plan.outputs[0];
  if (output?.role !== 'provider' || output.output_index !== 0 || !hash(output.economic_op_id))
    return fail('missing first provider output');
  const id = output.economic_op_id;
  const key = contract.targetedPayoutOutputRecordKey('fiat', 542, id);
  const paid = await contract.get(key);
  const value = paid?.value;
  const transfer = value?.stripe_transfer;
  if (paid?.type !== 'targeted_fiat_output_settlement' || paid.economic_op_id !== id ||
    paid.epoch !== 542 || paid.rail !== 'fiat' || value?.plan_root !== plan.plan_root ||
    !knownVersion(value.contract_version) ||
    contract.normalizeTargetedFiatOutputSettlementValue({ ...value, contract_version: version }) instanceof Error ||
    value.epoch_apply_hash !== plan.epoch_apply_hash || value.economic_op_id !== id ||
    value.output_index !== 0 || value.preparation_id !== id || !hash(value.attempt_id) ||
    transfer?.kind !== 'stripe_transfer' || transfer.attempt_id !== value.attempt_id ||
    transfer.destination !== output.to || transfer.source_currency !== output.source_currency ||
    transfer.source_amount_minor !== output.source_amount_minor ||
    contract.normalizeStripeTransferEvidence(transfer, 'FIAT paid sibling', { expectedAttemptId: value.attempt_id }) instanceof Error)
    return fail('missing or ambiguous 542 output-zero effect');
  const effect = await contract.get(contract.targetedFiatAttemptEffectKey(transfer.ref));
  const attempt = await contract.get(contract.targetedFiatAttemptRecordKey(id, value.attempt_id));
  const seen = await contract.get(contract.stripeTransferSeenKey(transfer));
  if (attempt?.status !== 'succeeded' || attempt.consumed !== true || attempt.consumed_by !== key ||
    !equal(attempt.result?.evidence, transfer) || effect?.economic_op_id !== id ||
    effect.attempt_id !== value.attempt_id || effect.effect_id !== transfer.ref ||
    effect.consumed !== true || effect.consumed_by !== key ||
    seen?.economic_op_id !== id || seen.rail !== 'fiat' || seen.epoch !== 542)
    return fail('542 output-zero payment is not reconciled');
  return paid;
}

async function lastFiatAttempt(contract, prior, old, latest, fence, version) {
  const id = old.economic_op_id;
  if (!latest) return fail('missing last 508 attempt');
  if (latest.economic_op_id !== id || !hash(latest.attempt_id) ||
    !Number.isSafeInteger(latest.attempt_no) || latest.attempt_no < 1)
    return fail('invalid last attempt pointer');
  if (latest.attempt_no !== 1) return fail('last observation has additional 508 attempts to reconcile');
  const attempt = await contract.get(contract.targetedFiatAttemptRecordKey(id, latest.attempt_id));
  const preparation = attempt?.preparation;
  if (attempt?.type !== 'targeted_fiat_attempt' || attempt.economic_op_id !== id ||
    attempt.attempt_id !== latest.attempt_id || attempt.attempt_no !== latest.attempt_no ||
    attempt.status !== latest.status || attempt.consumed !== false || attempt.consumed_by !== null ||
    !knownVersion(preparation?.contract_version) || preparation.op !== 'prepare_targeted_fiat_attempt' ||
    preparation.rail !== 'fiat' || preparation.epoch !== 508 || preparation.epoch_apply_hash !== prior.epoch_apply_hash ||
    preparation.plan_root !== prior.plan_root || preparation.output_index !== old.output_index || preparation.economic_op_id !== id ||
    preparation.attempt_no !== latest.attempt_no || preparation.attempt_id !== latest.attempt_id ||
    !time(preparation.prepared_at) || preparation.request?.kind !== 'stripe_transfer' ||
    preparation.request.destination !== old.to || preparation.request.source_currency !== old.source_currency ||
    preparation.request.source_amount_minor !== old.source_amount_minor ||
    preparation.request.destination_currency !== old.destination_currency ||
    preparation.request.destination_amount_min_minor !== old.destination_amount_min_minor ||
    preparation.request.destination_amount_max_minor !== old.destination_amount_max_minor ||
    preparation.request.transfer_group !== `mayhem_fiat_epoch_508_${prior.epoch_apply_hash.slice(0, 16)}`)
    return fail('ambiguous last attempt');
  const idempotency = await contract.get(`payout/attempt-idempotency/fiat/${preparation.idempotency_key_hash}`);
  if (idempotency?.economic_op_id !== id || idempotency.attempt_id !== preparation.attempt_id)
    return fail('last attempt request or idempotency lock changed');
  if (preparation.request.fx_quote_id !== null) {
    const quote = await contract.get(`payout/attempt-quote/fiat/${preparation.request.fx_quote_id}`);
    if (quote?.economic_op_id !== id || quote.attempt_id !== preparation.attempt_id ||
      quote.quote_hash !== preparation.request.fx_quote_hash ||
      await contract.get(contract.stripeFxQuoteSeenKey(preparation.request)) !== null)
      return fail('last attempt has conflicting quote evidence');
  }
  if (await contract.normalizePrepareTargetedFiatAttemptValue({ ...preparation, contract_version: version }) instanceof Error) {
    return fail('invalid retained attempt request');
  }
  const originalStatus = fence ? fence.last_status : latest.status;
  if (fence && latest.status !== 'recovery_fenced') return fail('attempt changed after fence');
  if (originalStatus === 'prepared' && attempt.result === null) return attempt;
  const result = attempt.result;
  if (originalStatus !== 'expired_pre_effect' || result?.status !== originalStatus ||
    !knownVersion(result.contract_version) || result.epoch !== 508 || result.economic_op_id !== id ||
    result.attempt_id !== preparation.attempt_id || result.at < preparation.quote_expires_at ||
    result.evidence?.quote_expires_at !== preparation.quote_expires_at ||
    result.evidence.fx_quote_id !== preparation.request.fx_quote_id || result.evidence.fx_quote_hash !== preparation.request.fx_quote_hash ||
    contract.normalizeFinalizeTargetedFiatAttemptValue({ ...result, contract_version: version }) instanceof Error) {
    return fail('attempt is terminal or has ambiguous evidence');
  }
  return attempt;
}

// Also used by the offline operator tool. Its reader must supply an explicit
// confirmed value (including null) for each requested key at one signed length.
export async function observeFiatOverlap(contract, version = 31) {
  const plans = [];
  for (const epoch of [508, 542]) {
    const plan = await contract.get(contract.targetedPayoutEpochPlanRecordKey('fiat', epoch));
    const value = plan?.value;
    if (plan?.type !== 'targeted_payout_epoch_plan' || plan.rail !== 'fiat' ||
      plan.epoch !== epoch || !hash(plan.plan_root) || value?.plan_root !== plan.plan_root ||
      value.op !== 'prepare_targeted_payout_epoch' || !knownVersion(value.contract_version) ||
      value.rail !== 'fiat' || value.epoch !== epoch || !Array.isArray(value.outputs) ||
      !hash(value.epoch_apply_hash) || !time(value.at)) return fail('missing or invalid frozen plan');
    // The roots commit to the frozen economics independently of the admission
    // version. Validate a projection in memory and retain the original signed bytes.
    if (await contract.normalizeTargetedPayoutEpochValue({ ...value, contract_version: version }) instanceof Error) {
      return fail('invalid retained plan commitments');
    }
    const anchor = await contract.requireEpochApplyAnchor(epoch, value.epoch_apply_hash, 'FIAT recovery');
    if (anchor instanceof Error) return anchor;
    if (await contract.get(contract.targetedPayoutEpochCloseRecordKey('fiat', epoch)) !== null)
      return fail('epoch already closed');
    plans.push(value);
  }
  const [prior, next] = plans;
  const sibling = await paidSibling(contract, next, version);
  if (sibling instanceof Error) return sibling;
  const oldFee = prior.outputs[1];
  const retained = next.outputs[2];
  if (prior.outputs.length !== 2 || next.outputs.length !== 3 ||
    oldFee?.role !== 'operator_fee' || retained?.role !== 'operator_fee' ||
    await contract.get(contract.targetedPayoutOutputRecordKey('fiat', 508, oldFee.economic_op_id)) !== null) {
    return fail('unexpected fee disposition in the frozen plans');
  }
  const fee = await contract.get(contract.targetedPayoutOutputRecordKey('fiat', 542, retained.economic_op_id));
  const feeValue = fee?.value;
  const retention = feeValue?.stripe_transfer;
  if (fee?.type !== 'targeted_fiat_output_settlement' || fee.epoch !== 542 || fee.rail !== 'fiat' ||
    fee.economic_op_id !== retained.economic_op_id || feeValue?.plan_root !== next.plan_root ||
    !knownVersion(feeValue.contract_version) ||
    contract.normalizeTargetedFiatOutputSettlementValue({ ...feeValue, contract_version: version }) instanceof Error ||
    feeValue.epoch_apply_hash !== next.epoch_apply_hash || feeValue.output_index !== 2 ||
    feeValue.economic_op_id !== retained.economic_op_id || retention?.kind !== 'platform_balance' ||
    retention.destination !== retained.to || retention.source_currency !== retained.source_currency ||
    retention.source_amount_minor !== retained.source_amount_minor ||
    contract.normalizeStripeTransferEvidence(retention, 'FIAT retained fee', { expectedAttemptId: feeValue.attempt_id }) instanceof Error) {
    return fail('missing or conflicting retained fee at 542 output two');
  }
  const replacement = next.outputs[1];
  const matches = prior.outputs.filter(o => o?.role === 'provider' &&
    o.provider === replacement?.provider && o.payout_revision === replacement?.payout_revision);
  if (matches.length !== 1 || replacement?.role !== 'provider') return fail('no unique overlap');
  const old = matches[0];
  if (old.output_index !== 0) return fail('wrong predecessor index');
  for (const [plan, out] of [[prior, old], [next, replacement]]) {
    if (plan.outputs[out.output_index] !== out ||
      contract.normalizeTargetedPayoutPlanOutput('fiat', out, out.output_index) instanceof Error ||
      await contract.get(contract.targetedPayoutOutputRecordKey('fiat', plan.epoch, out.economic_op_id)) !== null)
      return fail('invalid or already settled output');
  }
  if (replacement.output_index !== 1 || old.economic_op_id === replacement.economic_op_id ||
    !['to', 'source_currency', 'destination_currency', 'paid_cum_au_before',
      'aggregate_paid_cum_au_before'].every(k => old[k] === replacement[k]) ||
    contract.compareAu(replacement.paid_au, old.paid_au) < 0 ||
    contract.compareAu(replacement.liability_au, old.liability_au) < 0)
    return fail('replacement does not cover the same frozen liability interval');
  const id = old.economic_op_id;
  const nextId = replacement.economic_op_id;
  const payload = (plan, output) => ({
    settlement_op: 'settle_targeted_fiat_output', rail: 'fiat', epoch: plan.epoch,
    epoch_apply_hash: plan.epoch_apply_hash, plan_root: plan.plan_root,
    economic_op_id: output.economic_op_id, output_index: output.output_index,
    output, processor: 'stripe', source_currency: output.source_currency,
  });
  const oldLiability = contract.payoutPreparationLiabilityForOutput(prior, old, 'fiat');
  const prep = await contract.validatePayoutPreparationRecord({
    economicOpId: id, rail: 'fiat', epoch: 508, epochApplyHash: prior.epoch_apply_hash,
    kind: 'liability', outputIndex: old.output_index, payload: payload(prior, old),
    liability: oldLiability, externalEffectIds: [],
  });
  if (prep instanceof Error) return prep;
  if (!knownVersion(prep.contract_version) || prep.consumed_by !== null ||
    prep.superseded_by !== undefined || !time(prep.prepared_at) ||
    await contract.get(contract.payoutPreparationRecordKey('fiat', nextId)) !== null ||
    await contract.get(contract.targetedFiatAttemptTailKey(nextId)) !== null)
    return fail('unexpected preparation or replacement attempt');
  const lock = await contract.get(contract.payoutPreparationLiabilityLockKey('fiat', oldLiability));
  const tail = await contract.get(contract.payoutPreparationAggregateTailKey('fiat', old.provider));
  const earning = await contract.get(contract.earningKey(old.provider, 'fiat'));
  const liability = await contract.get(contract.providerPayoutLiabilityKey(old.provider, 'fiat', old.payout_revision));
  if (lock?.rail !== 'fiat' || lock.provider !== old.provider ||
    lock.payout_revision !== old.payout_revision || lock.paid_cum_au_before !== old.paid_cum_au_before ||
    tail?.economic_op_id !== id || tail.consumed !== false ||
    tail.paid_cum_au_before !== old.aggregate_paid_cum_au_before ||
    tail.paid_cum_au_after !== contract.safeAddAu(old.aggregate_paid_cum_au_before, old.paid_au) ||
    !await reviewedPayoutCountersMatch(contract, old, replacement, liability, earning))
    return fail('reservation or paid watermark changed');

  const fence = await contract.get(fiatFenceKey(id));
  const latest = await contract.get(contract.targetedFiatAttemptTailKey(id));
  const attempt = await lastFiatAttempt(contract, prior, old, latest, fence, version);
  if (attempt instanceof Error) return attempt;
  const terms = {
    rail: 'fiat', epoch: 542, epoch_apply_hash: next.epoch_apply_hash,
    economic_op_id: nextId, kind: 'liability', output_index: 1, payload: payload(next, replacement),
  };
  const nextPreparation = {
    op: 'prepare_targeted_payout', ...terms, prepared_at: next.at,
    payload_hash: await contract.opaqueHash('mayhem-targeted-payout-preparation-payload-v1', terms),
    liability: contract.payoutPreparationLiabilityForOutput(next, replacement, 'fiat'), external_effect_ids: [],
  };
  const observation = {
    old_plan_root: prior.plan_root, new_plan_root: next.plan_root,
    old_operation: id, new_operation: nextId, old_preparation: prep, lock, tail,
    paid_sibling: sibling, retained_fee: fee, revision_paid: old.paid_cum_au_before, aggregate_paid: old.aggregate_paid_cum_au_before, latest, attempt, fence,
  };
  // Undo only the status tag written by the fence and require the full
  // original observation to match. A fresh approval cannot bless a missing
  // attempt, a changed effect, or a different reservation after fencing.
  if (fence !== null) {
    const original = {
      ...observation, fence: null,
      latest: { ...latest, status: fence.last_status },
      attempt: { ...attempt, status: fence.last_status },
    };
    if (fence.type !== 'fiat_recovery_fence' || fence.value?.stage !== 'fence' ||
      fence.value.old_operation !== id || fence.value.new_operation !== nextId ||
      fence.feature_key !== await fiatRecoveryFeatureKey(contract, fence.value) ||
      fence.value.state_hash !== await contract.opaqueHash('mayhem-fiat-508-542-observation-v1', original)) {
      return fail('state changed since the signed fence');
    }
  }
  return {
    id, nextId, prep, nextPreparation, latest, attempt, fence,
    priorAt: prior.at,
    lastObservedAt: Math.max(prior.at, next.at, prep.prepared_at, sibling.value.at,
      feeValue.at, attempt.preparation.prepared_at, attempt.result?.at ?? 0),
    siblingRef: sibling.value.stripe_transfer.ref, retainedFeeRef: retention.ref,
    state_hash: await contract.opaqueHash('mayhem-fiat-508-542-observation-v1', observation),
  };
}

export const fiatRecoveryFeatureKey = async (contract, value) =>
  `payout/recovery-submit/fiat/508/542/${await contract.opaqueHash('mayhem-fiat-508-542-v1', value)}`;

export async function applyFiatRecovery(contract, key, value, version) {
  const shape = contract.validateExactObjectKeys(value, ['op', 'contract_version', 'rail', 'stage', 'old_operation',
    'new_operation', 'state_hash', 'signed_length', 'at', 'attestation', 'instruction', 'admin', 'admin_sig'],
    'FIAT preparation recovery');
  if (shape) return shape;
  if (value.op !== 'recover_fiat_preparation' || value.contract_version !== version || value.rail !== 'fiat' ||
    !['fence', 'supersede_absent', 'reconcile_verified'].includes(value.stage) ||
    ![value.old_operation, value.new_operation, value.state_hash, value.admin].every(hash) ||
    !time(value.signed_length) || !time(value.at) || !contract.isHexBytes(value.admin_sig, 64) ||
    key !== await fiatRecoveryFeatureKey(contract, value)) return fail('invalid signed operation');
  const authority = await contract.requireTargetedPayoutAdminSignature(value, 'FIAT preparation recovery');
  if (authority) return authority;
  const fencing = value.stage === 'fence';
  const recordKey = fencing ? fiatFenceKey(value.old_operation) : fiatResolutionKey(value.old_operation);
  const existing = await contract.get(recordKey);
  if (existing !== null) {
    if (existing.feature_key === key && equal(existing.value, value)) return { ok: true, idempotent: true };
    return fail('conflicting recovery instruction');
  }
  const state = await observeFiatOverlap(contract, version);
  if (state instanceof Error) return state;
  if (state.id !== value.old_operation || state.nextId !== value.new_operation || state.state_hash !== value.state_hash)
    return fail('reviewed observation no longer matches');
  const { id, latest, attempt, prep, fence } = state;
  if (fencing) {
    if (value.attestation !== null || value.instruction !== null || fence || value.at < state.lastObservedAt) {
      return fail('invalid fence');
    }
    await contract.put(recordKey, {
      type: 'fiat_recovery_fence', feature_key: key,
      last_status: latest.status, value: value
    });
    await contract.put(contract.targetedFiatAttemptRecordKey(id, latest.attempt_id), { ...attempt, status: 'recovery_fenced' });
    await contract.put(contract.targetedFiatAttemptTailKey(id), { ...latest, status: 'recovery_fenced' });
    return { ok: true, stage: value.stage };
  }
  const attestation = value.attestation;
  const attShape = contract.validateExactObjectKeys(attestation, ['fence_key', 'sends_drained_at', 'history_from',
    'history_through', 'destination_history_complete', 'in_flight_requests', 'old_effect', 'new_effect',
    'verified_sibling_ref', 'retained_fee_ref', 'evidence_sha256'], 'FIAT recovery attestation');
  if (attShape) return attShape;
  if (!fence || fence.feature_key !== attestation.fence_key || value.signed_length <= fence.value.signed_length ||
    !time(attestation.sends_drained_at) || attestation.sends_drained_at < fence.value.at ||
    !Number.isSafeInteger(attestation.history_from) || attestation.history_from < 0 ||
    attestation.history_from > Math.min(prep.prepared_at, state.priorAt) ||
    !time(attestation.history_through) || attestation.history_through < attestation.sends_drained_at ||
    attestation.history_through < state.lastObservedAt ||
    attestation.history_through > value.at || attestation.destination_history_complete !== true ||
    attestation.in_flight_requests !== 0 || attestation.new_effect !== 'absent' || attestation.verified_sibling_ref !== state.siblingRef ||
    attestation.retained_fee_ref !== state.retainedFeeRef ||
    !hash(attestation.evidence_sha256) || /^0+$/.test(attestation.evidence_sha256)) return fail('incomplete post-fence evidence');
  if (value.stage === 'supersede_absent') {
    const expected = { ...state.nextPreparation, contract_version: version, admin: value.admin, admin_sig: value.instruction?.admin_sig };
    if (attestation.old_effect !== 'absent' || !equal(value.instruction, expected)) return fail('wrong replacement preparation');
    const prepKey = await contract.targetedPayoutPreparationFeatureKey(expected);
    if (prepKey instanceof Error) return prepKey;
    const result = await contract.applyTargetedPayoutPreparationFeature(prepKey, expected, id);
    if (result instanceof Error) return result;
    await contract.put(contract.payoutPreparationRecordKey('fiat', id), {
      ...prep, superseded_by: state.nextId,
      recovery_key: recordKey
    });
  } else {
    const result = value.instruction;
    if (fence.last_status !== 'prepared' || !result || result.status !== 'succeeded' ||
      result.economic_op_id !== id || result.attempt_id !== attempt.attempt_id ||
      result.admin !== value.admin || result.at !== value.at || result.evidence?.kind !== 'stripe_transfer' ||
      result.evidence.ref !== attestation.old_effect) return fail('wrong verified transfer');
    const finalKey = await contract.finalizeTargetedFiatAttemptFeatureKey(result);
    if (finalKey instanceof Error) return finalKey;
    for (const seenKey of [contract.stripeTransferSeenKey(result.evidence), contract.stripeFxQuoteSeenKey(result.evidence),
    contract.stripeDestinationPaymentSeenKey(result.evidence)]) {
      if (seenKey && await contract.get(seenKey) !== null) return fail('verified transfer evidence already consumed');
    }
    const finalized = await contract.applyFinalizeTargetedFiatAttemptFeature(finalKey, result, true);
    if (finalized instanceof Error) return finalized;
  }
  await contract.put(recordKey, { type: 'fiat_recovery_resolution', feature_key: key, value: value });
  return { ok: true, stage: value.stage };
}

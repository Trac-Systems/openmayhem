// v31 FIAT/TNK only. Paid counters count confirmed effects. Pending counters
// count immutable, unreceipted outputs, including ones not yet dispatched.
const zero = { pending_au: '0', pending_dust_au: '0', allocated_through_au: '0' };

export const claimKey = (rail, id) => `payout/claim/${rail}/${id}`;
export const balanceKey = (rail, scope) => `payout/booked/${rail}/${scope}`;
export const revisionScope = output => `revision/${output.provider}/${output.payout_revision}`;
export const providerScope = output => `provider/${output.provider}`;

export function receiptMatchesOutput(receipt, plan, output) {
  const id = output.economic_op_id;
  return receipt?.type === `targeted_${plan.rail}_output_settlement` &&
    receipt.rail === plan.rail && receipt.epoch === plan.epoch && receipt.economic_op_id === id &&
    receipt.value?.economic_op_id === id && receipt.value.preparation_id === id &&
    receipt.value.output_index === output.output_index && receipt.value.epoch_apply_hash === plan.epoch_apply_hash &&
    receipt.value.plan_root === plan.plan_root;
}

export async function bookedBalance(contract, rail, scope) {
  const state = await contract.get(balanceKey(rail, scope)) ?? zero;
  for (const field of Object.keys(zero)) {
    if (contract.normalizeAu(state[field], 'Booked payout AU', { allowZero: true }) !== state[field])
      return new Error('Invalid booked payout balance.');
  }
  return state;
}

// Reserve transferable AU and temporarily exclude the rounding remainder of
// the frozen liability. Only paid AU advances the allocation cursor. Dust
// becomes available again when that output has a verified receipt.
export function unbookedBalance(contract, outstanding, held, booked) {
  const reserved = contract.safeAddAu(booked.pending_au, booked.pending_dust_au);
  if (reserved instanceof Error) return reserved;
  const remaining = contract.compareAu(reserved, outstanding) >= 0 ? '0' : contract.safeSubAu(outstanding, reserved);
  const remainingHeld = contract.compareAu(held, remaining) > 0 ? remaining : held;
  return { outstanding: remaining, held: remainingHeld, payable: contract.safeSubAu(remaining, remainingHeld) };
}

export function allocationCursor(contract, paid, booked) {
  const end = contract.safeAddAu(paid, booked.pending_au);
  return end instanceof Error ? end : contract.maxAu(end, booked.allocated_through_au);
}

export async function planClaimWrites(contract, plan) {
  const balances = new Map();
  const claims = [];
  for (const output of plan.outputs) {
    const key = claimKey(plan.rail, output.economic_op_id);
    if (await contract.get(key) !== null || await contract.get(
      contract.payoutPreparationRecordKey(plan.rail, output.economic_op_id)
    ) !== null) return new Error('Payout economic operation already allocated.');
    const paid = plan.rail === 'tnk' ? output.au : output.paid_au;
    const dust = plan.rail === 'tnk' ? '0' : output.dust_au;
    const scopes = output.role === 'provider' ? [revisionScope(output), providerScope(output)] : ['fee'];
    for (const scope of scopes) {
      const balance = balances.get(scope) ?? await bookedBalance(contract, plan.rail, scope);
      if (balance instanceof Error) return balance;
      const pending = contract.safeAddAu(balance.pending_au, paid);
      const pendingDust = contract.safeAddAu(balance.pending_dust_au, dust);
      if (pending instanceof Error || pendingDust instanceof Error) return new Error('Payout booking overflow.');
      const before = scope.startsWith('revision/') ? output.paid_cum_au_before : output.aggregate_paid_cum_au_before;
      const end = before === undefined ? balance.allocated_through_au : contract.safeAddAu(before, paid);
      if (end instanceof Error) return end;
      balances.set(scope, { pending_au: pending, pending_dust_au: pendingDust, allocated_through_au: end });
    }
    claims.push([key, { rail: plan.rail, epoch: plan.epoch, plan_root: plan.plan_root,
      economic_op_id: output.economic_op_id, output_index: output.output_index,
      scopes, paid_au: paid, dust_au: dust, consumed_by: null }]);
  }
  return [...claims, ...Array.from(balances, ([scope, balance]) => [balanceKey(plan.rail, scope), balance])];
}

export async function openClaim(contract, rail, plan, output, replaces = null) {
  const history = await contract.get(`payout/history/${rail}`);
  if (history && !history.complete) return new Error('Historical payout inventory is in progress.');
  const claim = await contract.get(claimKey(rail, output.economic_op_id));
  if (claim === null) {
    if (history !== null || output.role === 'operator_fee') {
      return new Error('No dispatchable claim for this historical payout. Inventory its frozen fee or liability first.');
    }
    return null;
  }
  if (claim.blocked_reason) return new Error(`Historical payout held: ${claim.blocked_reason}.`);
  if (claim.rail !== rail || claim.epoch !== plan.epoch || claim.plan_root !== plan.plan_root ||
      claim.output_index !== output.output_index || claim.economic_op_id !== output.economic_op_id ||
      claim.consumed_by !== null) return new Error('Payout claim conflicts with frozen output.');
  if (rail === 'fiat' && output.role === 'provider') {
    const review = await contract.get(`payout/legacy-review/fiat/${output.provider}`);
    if (review && ![review.old_operation, review.new_operation].includes(output.economic_op_id) &&
        !await contract.get(`payout/recovery/fiat/${review.old_operation}/resolution`)) {
      return new Error('Historical FIAT review binds this provider’s unchanged paid watermarks.');
    }
  }
  if (claim.legacy_group) {
    const group = await contract.get(claim.legacy_group);
    if (!group || group.consumed_by !== null || group.blocked ||
        group.owner !== (replaces ?? output.economic_op_id)) return new Error('Conflicting historical payout reservation.');
  }
  return claim;
}

export async function consumeClaimWrites(contract, claim, receiptKey) {
  const writes = [];
  const legacy = claim.legacy_group ? await contract.get(claim.legacy_group) : null;
  const reservedPaid = legacy?.paid_au ?? claim.paid_au;
  const reservedDust = legacy ? contract.safeSubAu(legacy.gross_au, legacy.paid_au) : claim.dust_au;
  if (reservedDust instanceof Error) return reservedDust;
  if (legacy) writes.push([claim.legacy_group, { ...legacy, consumed_by: receiptKey }]);
  for (const scope of claim.scopes) {
    const balance = await bookedBalance(contract, claim.rail, scope);
    if (balance instanceof Error) return balance;
    const pending = contract.safeSubAu(balance.pending_au, reservedPaid);
    const dust = contract.safeSubAu(balance.pending_dust_au, reservedDust);
    if (pending instanceof Error || dust instanceof Error) return new Error('Payout claim exceeds booked balance.');
    writes.push([balanceKey(claim.rail, scope), { ...balance, pending_au: pending, pending_dust_au: dust }]);
  }
  writes.push([claimKey(claim.rail, claim.economic_op_id), { ...claim, consumed_by: receiptKey }]);
  return writes;
}

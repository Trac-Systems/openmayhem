// Historical reducers had no index of open plans. Inventory the finite prefix
// once, using only deterministic point reads. Freeze and external dispatch
// use the completed index. A signed page never accepts caller-supplied sums.
import { balanceKey, bookedBalance, claimKey, revisionScope, providerScope, receiptMatchesOutput } from './payout-reservations.js';
import { fiat508FeeHeld } from './fiat-recovery.js';

export const HISTORY_EPOCHS_PER_PAGE = 32;
export const historyKey = rail => `payout/history/${rail}`;
const legacyKey = (rail, scope) => `payout/legacy/${rail}/${scope}`;
export const historyFeatureKey = async (contract, value) =>
  `payout/history-submit/${value.rail}/${value.epoch}/${await contract.opaqueHash('mayhem-payout-history-v1', value)}`;

// Historical fees lack a starting watermark. If anything swept after freeze,
// the present balance cannot authenticate that starting watermark. Hyperbee
// seq is the canonical insertion position of an immutable plan/receipt.
async function unchangedFeeSincePlan(contract, rail, fee, planKey) {
  if (fee.swept_cum_au === '0') return true;
  const last = fee.last_targeted_settlement_epoch;
  if (!Number.isSafeInteger(last) || !fee.last_targeted_settlement_transfer) return false;
  const priorPlan = await contract.get(contract.targetedPayoutEpochPlanRecordKey(rail, last));
  const output = priorPlan?.value.outputs.find(o => o.role === 'operator_fee');
  const key = output ? contract.targetedPayoutOutputRecordKey(rail, last, output.economic_op_id)
    : contract.targetedPayoutEpochCloseRecordKey(rail, last);
  const receipt = await contract.storage.get(key);
  const original = await contract.storage.get(planKey);
  if (!Number.isSafeInteger(receipt?.seq) || !Number.isSafeInteger(original?.seq) ||
      receipt.seq >= original.seq) return false;
  if (output) {
    const value = receipt.value?.value;
    return value?.plan_root === priorPlan.plan_root &&
      (rail === 'tnk' ? value?.msb_transfer?.tx_hash : value?.stripe_transfer?.ref) ===
        fee.last_targeted_settlement_transfer;
  }
  const transfers = rail === 'tnk' ? receipt.value?.msb_transfers : receipt.value?.stripe_transfers;
  return receipt.value?.type === `targeted_${rail}_settlement` &&
    transfers?.some(t => (rail === 'tnk' ? t.tx_hash : t.ref) === fee.last_targeted_settlement_transfer);
}

async function inventoryOutput(contract, writes, plan, output, planKey) {
  const { rail, epoch, plan_root } = plan;
  const id = output.economic_op_id;
  const claim = claimKey(rail, id);
  const retain = reason => writes.set(claim, { rail, epoch, plan_root,
    output_index: output.output_index, economic_op_id: id, blocked_reason: reason, consumed_by: null });
  const receiptKey = contract.targetedPayoutOutputRecordKey(rail, epoch, id);
  const receipt = await contract.get(receiptKey);
  if (receiptMatchesOutput(receipt, plan, output)) return;
  const prep = await contract.get(contract.payoutPreparationRecordKey(rail, id));
  // Old plans guaranteed IDs unique only within an epoch. A canonical
  // preparation pins the ID to exactly one set of original terms.
  if (prep && (prep.epoch !== epoch || prep.payload?.plan_root !== plan_root ||
      prep.output_index !== output.output_index)) return;
  if (writes.has(claim) || await contract.get(claim) !== null) return;
  if (prep?.superseded_by) {
    const resolution = await contract.get(`payout/recovery/${rail}/${id}/resolution`);
    if (resolution?.value.stage === 'supersede_absent' && resolution.value.old_operation === id &&
        resolution.value.new_operation === prep.superseded_by) {
      retain('historical_preparation_superseded');
      return;
    }
  }
  if (prep?.consumed && prep.consumed_by === contract.targetedPayoutEpochCloseRecordKey(rail, epoch)) {
    const bulk = await contract.get(prep.consumed_by);
    if (bulk?.type === `targeted_${rail}_settlement` && bulk.epoch === epoch &&
        bulk.preparation_ids?.[output.output_index] === id) {
      retain('historical_preparation_consumed');
      return;
    }
  }
  const inconsistent = receipt !== null || Boolean(prep?.consumed || prep?.superseded_by);
  const provider = output.role === 'provider';
  const scopes = provider ? [revisionScope(output), providerScope(output)] : ['fee'];
  const paid = BigInt(rail === 'tnk' ? output.au : output.paid_au);
  const gross = BigInt(rail === 'tnk' ? output.au : output.liability_au);
  let outstanding;
  let eligible = true;
  let before;
  if (provider) {
    const liability = await contract.get(contract.providerPayoutLiabilityKey(output.provider, rail, output.payout_revision));
    if (!liability) return new Error('Historical payout liability missing.');
    before = liability.paid_cum_au;
    outstanding = BigInt(liability.total_au) - BigInt(before);
    if (before !== output.paid_cum_au_before) {
      if (!prep && !inconsistent) {
        retain('historical_revision_watermark_already_spent');
        return;
      }
      eligible = false;
    }
  } else {
    if (rail === 'fiat' && epoch === 508 && await fiat508FeeHeld(contract, id)) {
      retain('historical_fee_fenced');
      return;
    }
    const fee = await contract.feeCumRecord(rail);
    if (fee instanceof Error) return fee;
    before = fee.swept_cum_au;
    outstanding = BigInt(fee.cum_au) - BigInt(before);
    eligible = await unchangedFeeSincePlan(contract, rail, fee, planKey);
    if (!eligible) {
      // An unprepared, unprovable fee cannot have a protocol-authorized send.
      // Fence its old ID and leave its actual outstanding AU in the ledger.
      if (!prep && !inconsistent) {
        retain('historical_fee_watermark_unproven');
        return;
      }
    }
  }
  const key = legacyKey(rail, scopes[0]);
  const old = writes.get(key) ?? await contract.get(key);
  const prepared = prep !== null || inconsistent;
  const collision = Boolean(old?.prepared && prepared);
  // The existing 508/542 review signs exact paid counters for this provider.
  // Its absence/success adjudication must still be executable after inventory.
  const reviewedPair = rail === 'fiat' && provider && old?.epoch === 508 && old.index === 0 &&
    old.prepared && epoch === 542 && output.output_index === 1 && before === old.before;
  const preferred = !old || (prepared && !old.prepared);
  const max = (a, b) => a > b ? a : b;
  const min = (a, b) => a < b ? a : b;
  // An unprepared losing candidate can never dispatch, so its overlap cannot
  // withhold a provably new tranche. A reviewed replacement can still take
  // ownership. Multiple signed effects without provably equal ranges require
  // covering their entire possible union up to the actual unpaid balance.
  const oldGross = BigInt(old?.gross_au ?? '0');
  const oldPaid = BigInt(old?.paid_au ?? '0');
  const reserved = eligible ? gross : min(gross, outstanding);
  let reservedGross = preferred ? reserved : oldGross;
  let reservedPaid = preferred ? (eligible ? paid : reserved) : oldPaid;
  if (collision) {
    reservedGross = reservedPaid = min(outstanding, oldGross + gross);
  } else if (reviewedPair) {
    reservedGross = max(oldGross, reserved);
    reservedPaid = max(oldPaid, paid);
  }
  if (reservedPaid > reservedGross) return new Error('Invalid historical payout reservation.');
  const group = {
    epoch: preferred ? epoch : old.epoch, index: preferred ? output.output_index : old.index,
    owner: preferred ? id : old.owner, prepared: prepared || old?.prepared === true,
    blocked: old?.blocked === true || collision || inconsistent || (prepared && !eligible),
    ...(reviewedPair || old?.reviewed_successor
      ? { reviewed_successor: reviewedPair ? id : old.reviewed_successor } : {}),
    paid_au: String(reservedPaid), gross_au: String(reservedGross), before,
    consumed_by: null,
  };
  writes.set(key, group);
  for (const scope of scopes) {
    const k = balanceKey(rail, scope);
    const base = writes.get(k) ?? await bookedBalance(contract, rail, scope);
    if (base instanceof Error) return base;
    const nextPaid = BigInt(base.pending_au) - BigInt(old?.paid_au ?? '0') + reservedPaid;
    const oldDust = BigInt(old?.gross_au ?? '0') - BigInt(old?.paid_au ?? '0');
    const nextDust = BigInt(base.pending_dust_au) - oldDust + reservedGross - reservedPaid;
    const pending = contract.canonicalAu(nextPaid), dust = contract.canonicalAu(nextDust);
    if (pending instanceof Error || dust instanceof Error) return new Error('Historical payout inventory overflow.');
    writes.set(k, { ...base, pending_au: pending, pending_dust_au: dust });
  }
  writes.set(claim, { rail, epoch, plan_root, output_index: output.output_index, economic_op_id: id,
    scopes, paid_au: String(paid), dust_au: String(gross-paid), legacy_group: key, consumed_by: null });
}

export async function indexPayoutHistory(contract, key, value, version) {
  const shape = contract.validateExactObjectKeys(value,
    ['op','contract_version','rail','epoch','after_epoch','admin','admin_sig'], 'payout history page');
  if (shape) return shape;
  const { rail, epoch, after_epoch: after } = value;
  if (value.contract_version !== version || !['tnk','fiat'].includes(rail) ||
      !Number.isSafeInteger(epoch) || !Number.isSafeInteger(after) || epoch < 1 || after < 0 ||
      epoch <= after || epoch-after > HISTORY_EPOCHS_PER_PAGE ||
      key !== await historyFeatureKey(contract, value)) return new Error('Invalid payout history page.');
  const auth = await contract.requireTargetedPayoutAdminSignature(value, 'Payout history');
  if (auth) return auth;
  const state = await contract.get(historyKey(rail));
  if (state?.through_epoch >= epoch) return { ok: true, idempotent: true };
  if (state?.complete || (state?.through_epoch ?? 0) !== after) return new Error('Payout history cursor changed.');
  const head = (await contract.epochApplyStateRecord()).updated_epoch;
  if (epoch > head) return new Error('Payout history page exceeds applied head.');
  const writes = new Map();
  for (let n = after+1; n <= epoch; n++) {
    const planKey = contract.targetedPayoutEpochPlanRecordKey(rail, n);
    const plan = await contract.get(planKey);
    if (!plan) continue;
    const close = await contract.get(contract.targetedPayoutEpochCloseRecordKey(rail, n));
    if (close?.type === 'targeted_payout_epoch_close' && close.rail === rail && close.epoch === n &&
        close.plan_root === plan.plan_root && close.value?.epoch_apply_hash === plan.value.epoch_apply_hash) continue;
    for (const output of plan.value.outputs) {
      const error = await inventoryOutput(contract, writes, plan.value, output, planKey);
      if (error) return error;
    }
  }
  for (const [k,v] of writes) await contract.put(k,v);
  await contract.put(historyKey(rail), { through_epoch: epoch, complete: epoch === head });
  return { ok: true, through_epoch: epoch, complete: epoch === head };
}

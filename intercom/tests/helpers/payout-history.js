import assert from 'node:assert/strict';
import b4a from 'b4a';
import { CONTRACT_VERSION, targetedPayoutControlMessage } from '../../contract/contract.js';
import { HISTORY_EPOCHS_PER_PAGE, historyFeatureKey } from '../../contract/payout-history.js';
import { executeFeature } from './contract.js';

export async function inventoryPage(ctx, rail, after, end, actor = ctx.admin.publicKey) {
  const value = {
    op: 'index_targeted_payout_history', contract_version: CONTRACT_VERSION,
    rail, epoch: end, after_epoch: after, admin: ctx.admin.publicKey,
  };
  value.admin_sig = b4a.toString(ctx.admin.wallet.sign(b4a.from(targetedPayoutControlMessage(value))), 'hex');
  return executeFeature(ctx.contract, ctx.storage, 'mayhem_feature',
    await historyFeatureKey(ctx.contract, value), value, actor);
}

export async function inventory(ctx, rail) {
  const head = (await ctx.storage.get('epoch/apply/state')).value.updated_epoch;
  let state = (await ctx.storage.get(`payout/history/${rail}`))?.value;
  while (!state?.complete) {
    const after = state?.through_epoch ?? 0;
    const result = await inventoryPage(ctx, rail, after, Math.min(head, after + HISTORY_EPOCHS_PER_PAGE));
    assert.equal(result.ok, true, result.message);
    state = (await ctx.storage.get(`payout/history/${rail}`)).value;
  }
}

export async function advanceHead(storage, epoch, hash) {
  const tx = `epoch/targeted/${epoch}/apply`;
  await storage.put(`epoch/apply-anchor/${epoch}`, {
    type: 'epoch_apply_anchor', epoch, apply_hash: hash, settlement_unix: 90_000, applied_at: tx,
  });
  await storage.put('epoch/apply/state', {
    updated_epoch: epoch, updated_at: tx, last_apply_hash: hash, last_settlement_unix: 90_000,
  });
}

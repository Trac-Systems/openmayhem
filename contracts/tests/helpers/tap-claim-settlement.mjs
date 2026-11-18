import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';

import { stableJson } from '../../../intercom/scripts/recompute-epoch-roots.mjs';
import { rollTapSettlement } from '../../scripts/tap-settlement-roller.mjs';
import { targetedTapBindingsFor } from './signed-receipt.mjs';

function memoryPreparationSubmitter() {
  const preparations = new Map();
  return async ({ plan }) => {
    const allExisting = plan.preparations.every(item => preparations.has(item.economic_op_id));
    const records = plan.preparations.map(item => {
      const existing = preparations.get(item.economic_op_id);
      if (existing) assert.deepEqual(existing, item);
      else preparations.set(item.economic_op_id, structuredClone(item));
      return { type: 'targeted_payout_preparation', ...item, consumed: false };
    });
    return { ...plan, records, all_existing: allExisting };
  };
}

export function claimSettlementInputs({ bundle, providerAccounts, tapUsdAu, tokenAddr, poolAddr }) {
  const targetedSessionBindings = targetedTapBindingsFor(bundle, providerAccounts);
  const bindings = Object.values(targetedSessionBindings);
  assert.equal(bindings.length, 1, 'claim fixture requires one targeted session');
  const [binding] = bindings;
  const grossAu = BigInt(binding.au);
  const feeAu = grossAu * BigInt(bundle.params.fee_bps) / 10_000n;
  const burnAu = grossAu * 1_000n / 10_000n;
  const rateTs = bundle.epoch * 3_600;
  return {
    bundle,
    targetedSessionBindings,
    canonicalLiabilities: [{
      provider: binding.provider,
      rail: 'tap',
      payout_revision: binding.payout_revision,
      target: binding.account.toLowerCase(),
      chain_id: binding.chain_id,
      total_au: (grossAu - feeAu - burnAu).toString(),
      held_au: '0',
      paid_cum_au: '0',
      aggregate_paid_cum_au: '0',
      updated_epoch: bundle.epoch,
      updated_at: `epoch/targeted/${bundle.epoch}/${bundle.epoch_apply_hash}`,
    }],
    tapUsdAu,
    epochApplyHash: bundle.epoch_apply_hash,
    tapRateLock: {
      type: 'tap_settlement_rate_lock',
      epoch: bundle.epoch,
      bundle_sha256: createHash('sha256').update(stableJson(bundle)).digest('hex'),
      denom: 'tap_usd_au',
      tap_usd_au: tapUsdAu,
      source: 'test-fixed-rate',
      rate_ts: rateTs,
      rate_record_key: `rate/tap/${rateTs}/${'ef'.repeat(32)}`,
      posted_by: 'aa'.repeat(32),
      posted_by_role: 'admin',
      chain_id: binding.chain_id,
      token_address: tokenAddr.toLowerCase(),
      pool_address: poolAddr.toLowerCase(),
      payment_config_ver: binding.payment_config_version,
    },
    canonicalPreparationSubmitter: memoryPreparationSubmitter(),
  };
}

export async function postAndFinalizeClaimSettlement(provider, args) {
  const posted = await rollTapSettlement(args);
  assert.equal(posted.posted, true);
  assert.equal(posted.canonical_preparation_confirmed, true);
  assert.equal(posted.awaiting_finality, true);
  assert.equal(posted.tap_settlement_checkpoint, null);
  for (let block = 0; block < 12; block++) {
    await provider.send('evm_mine', []);
  }
  const confirmed = await rollTapSettlement({ ...args, prior: posted });
  assert.equal(confirmed.awaiting_finality, false);
  const checkpoint = confirmed.tap_settlement_checkpoint;
  assert.equal(checkpoint.root_confirmed, true);
  assert.equal(checkpoint.root, posted.root);
  assert.ok(checkpoint.confirmation_depth >= 12);
  return confirmed;
}

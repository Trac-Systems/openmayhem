import assert from 'node:assert/strict';
import { proxyReceiptFixture } from './proxy-finance.js';
import { proxyEpochBundle } from './proxy-epoch.js';
import { execute, executeFeature } from './contract.js';
import { recomputeEpoch } from '../../scripts/recompute-epoch-roots.mjs';
import { nativePayoutFixture, submitNativeReservation, nativeReceiptValue, submitNativeReceipt } from './native-payout-receipt.mjs';
import { proxyOfferCost } from '../../contract/proxy-protocol.js';
import { proxyPaymentTermsDigest } from '../../contract/proxy-reservations.js';

/** Real signed proxy receipt, canonical epoch writer and contract apply. The
 * operator test consumes these exact bytes; it never adds native receipt fields
 * to the signed proxy body or invents payout liabilities for the worker. */
export async function proxyPayoutEpoch(rail, { mixed = false, retry = false } = {}) {
  const f = await proxyReceiptFixture(rail, 'llm', null, mixed);
  const heads = [];
  if (mixed) {
    f.offer.revision++;
    f.offer.rates = f.offer.rates.map(rate => ({ ...rate, per_unit_au: '1000000000000000000', granularity: 1 }));
    assert.equal((await f.submit(await f.envelope({ kind: 'set_offer', offer: f.offer }))).ok, true);
    const native = await nativePayoutFixture(rail);
    await native.storage.put('epoch/apply/state', { updated_epoch: 100, pending_epoch: null, last_apply_hash: 'a'.repeat(64), last_settlement_unix: 360000 });
    const reserved = await submitNativeReservation(native, { epoch: 101 });
    assert.equal(reserved.result.ok, true, reserved.result.message);
    const recorded = await submitNativeReceipt(native, nativeReceiptValue(native, reserved, { final: true }));
    assert.equal(recorded.result.ok, true, recorded.result.message);
    heads.push((await native.storage.get(native.contract.receiptHeadKey(reserved.value.voucher.billing_id, 0))).value);
    for (const [key, value] of f.storage.values) {
      if (!['admin', 'rules/current', 'epoch/apply/state', 'payments/current'].includes(key)) await native.storage.put(key, value);
    }
    f.storage = native.storage; f.contract = native.contract; f.admin = native.admin;
    f.read = async key => (await f.storage.get(key))?.value ?? null;
    f.ledger.get = f.read;
    f.apply = async plan => { for (const write of plan.writes) if (write.delete) await f.storage.del(write.key); else await f.storage.put(write.key, write.value); };
    f.terms.max_spend_au = proxyOfferCost(f.terms.offer, f.terms.max_usage);
    f.terms.max_total_spend_au = String(BigInt(f.terms.max_spend_au) * (retry ? 2n : 1n));
    f.terms.payment_terms_hash = await proxyPaymentTermsDigest(await f.read('rules/current'), f.payout);
    await f.storage.put(f.balanceKey, { ...f.balance, au: String(BigInt(f.terms.max_total_spend_au) + 100n) });
    await f.apply(await f.prepare(f.authorize(f.terms)));
  }
  if (retry) {
    assert.equal(mixed, true);
    await f.apply(await f.finalize(await f.receipt()));
    const first = await f.read(f.ledger.receiptHeadKey(f.terms.billing_id, 1));
    heads.push(first);
    Object.assign(f.terms, { billing_attempt: 2, prior_spend_au: first.receipt.body.au_owed_cum,
      session_id: 'b1'.repeat(32), reservation_id: 'b2'.repeat(32), capacity_lease: 'b3'.repeat(32) });
    await f.apply(await f.prepare(f.authorize(f.terms)));
    const secondAu = proxyOfferCost(f.terms.offer, Object.fromEntries(f.terms.offer.rates.map(rate => [rate.unit, 4])));
    await f.apply(await f.finalize(await f.receipt({ billing_au_owed_cum: String(BigInt(f.terms.prior_spend_au) + BigInt(secondAu)) })));
  } else await f.apply(await f.finalize(await f.receipt()));
  const head = await f.read(f.ledger.receiptHeadKey(f.terms.billing_id, f.terms.billing_attempt));
  heads.push(head);
  const epoch = head.settlement_epoch, at = epoch * 3600;
  const prior = { epoch: epoch - 1, updated_epoch: epoch - 1, pending_epoch: null,
    last_apply_hash: 'a'.repeat(64), last_settlement_unix: (epoch - 1) * 3600 };
  await f.storage.put('epoch/apply/state', prior);
  await f.storage.put(`epoch/apply-anchor/${epoch - 1}`, { type: 'epoch_apply_anchor', epoch: epoch - 1,
    apply_hash: prior.last_apply_hash, settlement_unix: prior.last_settlement_unix, applied_at: 'b'.repeat(64) });
  await f.storage.put('payments/current', { set_by_role: 'admin', tap: { chain_id: 1, pool_address: '0x' + '1'.repeat(40) } });
  const frozen = await execute(f.contract, f.storage, 'epochFreeze', { op: 'epoch_freeze', epoch, at }, f.admin.publicKey, 990);
  assert.equal(frozen.ok, true, frozen.message);
  const params = await f.ledger.activeParamsAt(at, ['fee_bps', 'epoch_seconds']);
  const bundle = await proxyEpochBundle(f.ledger, heads, epoch, { ...params, maxApplyBatch: 100 });
  const recomputed = await recomputeEpoch(bundle);
  assert.equal(recomputed.apply_pages.length, 1);
  const page = recomputed.apply_pages[0];
  const value = { op: 'commit_apply_targeted_epoch_page0', epoch, at,
    epoch_commit_hash: await f.ledger.epochCommitHash({ epoch, epoch_seconds: params.epoch_seconds,
      roots: recomputed.roots, totals: recomputed.totals }),
    receipt_index: page.receipt_index, allocations: page.allocations, debits: page.debits,
    earnings: page.earnings, last_page: page.last_page, earning_finals: page.earning_finals,
    market_usage: page.market_usage, roots: recomputed.roots, totals: recomputed.totals };
  const key = await f.ledger.commitTargetedEpochPageZeroFeatureKey(value);
  assert.ok(!(key instanceof Error), key.message);
  const applied = await executeFeature(f.contract, f.storage, 'mayhem_feature', key, value, f.admin.publicKey);
  assert.equal(applied.ok, true, applied.message);
  const state = await f.read('epoch/apply/state');
  const liability = await f.read(f.ledger.providerPayoutLiabilityKey(f.provider.publicKey, rail, f.payout.revision));
  const gross = BigInt(recomputed.earnings.find(row => row.provider === f.provider.publicKey).gross_au);
  assert.equal(liability.total_au, String(gross - gross * BigInt(recomputed.params.fee_bps) / 10000n -
    (rail === 'tap' ? gross * BigInt(recomputed.params.tap_burn_bps) / 10000n : 0n)));
  assert.equal(liability.paid_cum_au, '0');
  assert.equal(liability.rail, rail);
  const records = Object.fromEntries(await Promise.all(['epoch/apply/state', `epoch/commit/${epoch}`, `epoch/apply-anchor/${epoch}`]
    .map(async key => [key, { key, confirmed: true, value: await f.read(key) }])));
  return { bundle, recomputed, records, epoch, applyHash: state.last_apply_hash, liability, f, heads };
}

/** Advance only synthetic fixture time and use the contract's unchanged holdback
 * calculation. No fee/amount/binding is replaced, and immature evidence is kept
 * available to tests before this explicit fixture transition. */
export async function matureFixtureLiabilities(source) {
  const { f, epoch } = source;
  const atEpoch = epoch + 1000;
  const params = await f.ledger.activeParamsAt(atEpoch * 3600,
    ['holdback_epochs', 'new_provider_holdback_epochs', 'canary_probe_holdback_bps']);
  for (const [key, record] of [...f.storage.values]) {
    if (!key.startsWith('payout/liability/') && !key.startsWith('earn/')) continue;
    const provider = await f.read(`prov/${record.provider}`);
    const locked = f.ledger.providerLockedEarningEpochs(provider, params);
    const probe = await f.ledger.probeGateForEarning(record.provider, record, params);
    const dispute = await f.ledger.providerHasOpenDispute(record.provider);
    for (const value of [locked, probe, dispute]) assert.ok(!(value instanceof Error), value?.message);
    const matured = f.ledger.refreshEarningHoldback(record, atEpoch, locked, probe, dispute);
    assert.ok(!(matured instanceof Error), matured.message);
    await f.storage.put(key, matured);
  }
  return atEpoch;
}

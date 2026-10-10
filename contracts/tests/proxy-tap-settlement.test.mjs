import assert from 'node:assert/strict';
import test from 'node:test';
import Ganache from 'ganache';
import { ethers } from 'ethers';
import { deployPool } from '../scripts/deploy-local.mjs';
import { checkBundledCli } from './helpers/mixed-payout-cli.mjs';
import { createHash } from 'node:crypto';
import { proxyPayoutEpoch, matureFixtureLiabilities } from '../../intercom/tests/helpers/proxy-payout-epoch.mjs';
import { deriveTapReceiptBundle } from '../../intercom/scripts/payout-tap-receipts.mjs';
import { buildTapSettlement, buildVerifiedTapSettlement, resolveTargetedTapPayoutsFromLedger, rollTapSettlement } from '../scripts/tap-settlement-roller.mjs';

async function fixture({ retry = false } = {}) {
  const source = await proxyPayoutEpoch('tap', { mixed: true, retry });
  await matureFixtureLiabilities(source);
  const bundle = await deriveTapReceiptBundle(source.bundle, source.epoch, source.applyHash);
  const reads = [];
  const fetchImpl = async input => {
    const url = new URL(input);
    assert.equal(url.origin, 'http://fixture.invalid');
    assert.equal(url.searchParams.get('confirmed'), 'true');
    const key = url.searchParams.get('key'); reads.push(key);
    return { ok: true, json: async () => ({ key, confirmed: true, value: await source.f.read(key) }) };
  };
  const resolved = await resolveTargetedTapPayoutsFromLedger({ bundle, peerRpcUrl: 'http://fixture.invalid/v1/', fetchImpl });
  const params = await source.f.ledger.activeParamsAt(source.epoch * 3600, ['payout_min_au']);
  const args = { bundle, targetedSessionBindings: resolved.sessionBindings, canonicalLiabilities: resolved.liabilities,
    tapUsdAu: '1000000000000000000', ledgerFeeBps: source.recomputed.params.fee_bps,
    payoutMinAu: params.payout_min_au, settleThroughEpoch: source.epoch + 100 };
  return { ...source, bundle, args, fetchImpl, reads };
}

test('real mixed epoch reaches async TAP accounting with original signed receipts and exact liabilities', async () => {
  const f = await fixture(), original = structuredClone(f.bundle);
  const settlement = await buildVerifiedTapSettlement(f.args);
  assert.equal(settlement.receipt_count, 2);
  assert.equal(settlement.spent_au, f.recomputed.totals.earn_au);
  assert.equal(settlement.cumulative_spent_wei, f.recomputed.totals.use_au);
  assert.equal(settlement.checkpoint_outputs.length, 2);
  assert.deepEqual(f.bundle, original);
  assert.ok(f.reads.every(key => /^(payout\/(allocation|binding|liability)\/|earn\/tap\/)/.test(key)));
  const native = f.bundle.receipts.find(head => head.lane !== 'proxy');
  const nativeArgs = { ...f.args, bundle: { ...f.bundle, receipts: [native], proxy_acceptances: {} } };
  assert.deepEqual({ ...await buildVerifiedTapSettlement(nativeArgs), dist: null }, { ...buildTapSettlement(nativeArgs), dist: null });
  // The real downstream worker path must use the async adapter, not just expose it.
  const rolled = await rollTapSettlement({ ...f.args, ...rateLock(f), post: false });
  assert.equal(rolled.root, settlement.root);
  assert.equal(rolled.receipt_count, 2);
});

test('proxy TAP rejects signature, rail, amount, duplicate, epoch and payout-revision substitutions', async t => {
  const f = await fixture();
  for (const [name, change] of [
    ['signature', b => { b.receipts.find(h => h.lane === 'proxy').receipt.provider_sig = '0'.repeat(128); }],
    ['rail', b => { b.receipts.find(h => h.lane === 'proxy').rail = 'tnk'; }],
    ['amount override', b => { b.receipts.find(h => h.lane === 'proxy').settle_au = '1'; }],
    ['epoch override', b => { b.receipts.find(h => h.lane === 'proxy').receipt_epoch++; }],
    ['duplicate', b => { b.receipts.push(structuredClone(b.receipts.find(h => h.lane === 'proxy'))); }],
    ['accepted terms', b => { Object.values(b.proxy_acceptances)[0].authorization.terms.rail = 'fiat'; }],
  ]) await t.test(name, async () => {
    const bundle = structuredClone(f.bundle); change(bundle);
    await assert.rejects(buildVerifiedTapSettlement({ ...f.args, bundle }));
  });
    await assert.rejects(rollTapSettlement({ ...f.args, ...rateLock(f), epoch: 1, post: false }), /epoch conflicts/);
  await assert.rejects(rollTapSettlement({ ...f.args, ...rateLock(f), tapRateLock: { ...rateLock(f).tapRateLock, epoch: 1 }, post: false }), /epoch conflicts/);
  const wrongAmount = structuredClone(f.args.targetedSessionBindings);
  Object.values(wrongAmount)[0].au = '1';
  await assert.rejects(buildVerifiedTapSettlement({ ...f.args, targetedSessionBindings: wrongAmount }), /exceeds targeted session allocation/);
  const bindings = structuredClone(f.args.targetedSessionBindings);
  const proxy = f.bundle.receipts.find(h => h.lane === 'proxy');
  const key = Object.keys(bindings).find(key => key.endsWith('/' + proxy.session_id));
  bindings[key].payout_revision = '0'.repeat(64);
  await assert.rejects(buildVerifiedTapSettlement({ ...f.args, targetedSessionBindings: bindings }), /allocation.*provider/);
  const allocationKey = `payout/allocation/${f.epoch}/${proxy.session_id}`;
  const allocation = await f.f.read(allocationKey);
  await f.f.storage.put(allocationKey, { ...allocation, payout_revision: '0'.repeat(64) });
  await assert.rejects(resolveTargetedTapPayoutsFromLedger({ bundle: f.bundle, peerRpcUrl: 'http://fixture.invalid/v1/', fetchImpl: f.fetchImpl }), /allocation/);
});

function rateLock(f, pool = '0x' + '1'.repeat(40), token = '0x' + '2'.repeat(40)) {
  return { epochApplyHash: f.applyHash, tapRateLock: { type: 'tap_settlement_rate_lock', epoch: f.epoch,
    bundle_sha256: createHash('sha256').update(JSON.stringify(f.bundle)).digest('hex'),
    denom: 'tap_usd_au', tap_usd_au: f.args.tapUsdAu, source: 'isolated-fixed-rate', rate_ts: f.epoch * 3600,
    rate_record_key: `rate/tap/${f.epoch * 3600}/${'e'.repeat(64)}`, posted_by: f.f.admin.publicKey,
    posted_by_role: 'admin', chain_id: 1, token_address: token, pool_address: pool, payment_config_ver: 1 } };
}

test('actual mixed TAP roller resumes after confirmed external fee transfer with one root and no duplicate transfer', { timeout: 60_000 }, async t => {
  const f = await fixture();
  const chain = Ganache.provider({ logging: { quiet: true }, chain: { chainId: 1 }, wallet: { totalAccounts: 3 } });
  t.after(async () => { await chain.disconnect(); });
  const provider = new ethers.BrowserProvider(chain);
  const owner = await provider.getSigner(0), buyer = await provider.getSigner(1), treasury = await provider.getSigner(2);
  const { token, pool, poolAddr, governanceWallet } = await deployPool(owner, { governanceDelay: 3600n, maxEpochDelta: ethers.parseEther('20') });
  await (await token.mint(await buyer.getAddress(), ethers.parseEther('20'))).wait();
  await (await token.connect(buyer).approve(poolAddr, ethers.parseEther('20'))).wait();
  await (await pool.connect(buyer).deposit(ethers.parseEther('20'))).wait();
  await checkBundledCli(t, f, chain, pool, token, owner, governanceWallet, treasury);
  // Synthetic external environment only. Retained preparation identities are
  // the real roller's exact plans, while the fixture supplies confirmation.
  const preparations = new Map();
  const canonicalPreparationSubmitter = async ({ plan }) => {
    const allExisting = plan.preparations.every(item => preparations.has(item.economic_op_id));
    const records = plan.preparations.map(item => {
      const previous = preparations.get(item.economic_op_id);
      if (previous) assert.deepEqual(previous, item);
      else preparations.set(item.economic_op_id, structuredClone(item));
      return { type: 'targeted_payout_preparation', ...item, consumed: false };
    });
    return { ...plan, records, all_existing: allExisting };
  };
  const args = { ...f.args, ...rateLock(f, poolAddr, await token.getAddress()), pool, ownerSigner: owner, governanceSigner: governanceWallet,
    operatorAddress: await treasury.getAddress(), canonicalPreparationSubmitter, post: true };
  let armed = true, feeSends = 0;
  const wrap = target => new Proxy(target, { get(object, key) {
    if (key === 'connect') return signer => wrap(object.connect(signer));
    const value = Reflect.get(object, key, object);
    if (key !== 'withdrawOperator') return typeof value === 'function' ? value.bind(object) : value;
    const call = async (...values) => {
      feeSends++;
      const sent = await value(...values);
      return { hash: sent.hash, wait: async () => { const result = await sent.wait();
        if (armed) { armed = false; throw new Error('isolated crash after confirmed fee transfer before ACK'); }
        return result; } };
    };
    call.staticCall = (...values) => value.staticCall(...values);
    call.estimateGas = (...values) => value.estimateGas(...values);
    return call;
  } });
  const pending = await rollTapSettlement({ ...args, pool: wrap(pool) });
  assert.equal(pending.root_pending, true);
  await provider.send('evm_increaseTime', [3601]); await provider.send('evm_mine', []);
  await assert.rejects(rollTapSettlement({ ...args, pool: wrap(pool) }), /crash after confirmed fee transfer/);
  assert.equal(feeSends, 1);
  const paid = await token.balanceOf(await treasury.getAddress());
  assert.equal(paid, BigInt(f.recomputed.totals.fee_au));
  const resumed = await rollTapSettlement({ ...args, pool: wrap(pool) });
  for (let count = 0; count < 12; count++) await provider.send('evm_mine', []);
  await new Promise(resolve => setTimeout(resolve, 300));
  const confirmed = await rollTapSettlement({ ...args, pool: wrap(pool), prior: resumed });
  assert.equal(confirmed.root_confirmed, true, JSON.stringify({ resumed: { root: resumed.root, blocked: resumed.blocked, reasons: resumed.reasons, awaiting_finality: resumed.awaiting_finality }, confirmed: { root: confirmed.root, blocked: confirmed.blocked, reasons: confirmed.reasons, awaiting_finality: confirmed.awaiting_finality } }));
  assert.ok(confirmed.tap_settlement_checkpoint);
  assert.equal(feeSends, 1);
  assert.equal(await token.balanceOf(await treasury.getAddress()), paid);
  assert.equal((await pool.queryFilter(pool.filters.RootProposed())).length, 1);
  assert.equal((await pool.queryFilter(pool.filters.RootPosted())).length, 1);
  assert.equal(await pool.totalBurned(), BigInt(f.recomputed.totals.burn_au));
  assert.equal(preparations.size, 3, 'two provider liabilities and one root intent');
  assert.equal(confirmed.tap_settlement_checkpoint.outputs.length, 2);
  assert.deepEqual(confirmed.tap_settlement_checkpoint.outputs.map(o => o.paid_au).sort(),
    f.args.canonicalLiabilities.map(l => l.total_au).sort());
  const replay = await rollTapSettlement({ ...args, pool: wrap(pool), prior: confirmed, bundle: { ...f.bundle, receipts: [] } });
  assert.equal(replay.root, confirmed.root, 'accepted checkpoint replay precedes fresh receipt access');
  assert.equal(feeSends, 1);
});

test('real sequential proxy attempts settle their own signed amount once, not the logical cumulative spend', async () => {
  const f = await fixture({ retry: true });
  const heads = f.bundle.receipts.filter(head => head.lane === 'proxy');
  assert.equal(heads.length, 2);
  assert.equal(heads[1].receipt.body.billing_au_owed_cum, '16000000000000000000');
  assert.equal(heads[1].receipt.body.au_owed_cum, '8000000000000000000');
  const result = await buildVerifiedTapSettlement(f.args);
  assert.equal(result.receipt_count, 3);
  assert.equal(result.cumulative_spent_wei, '18000000000000000000');
  assert.equal(result.spent_au, f.recomputed.totals.earn_au);
  const reversed = await buildVerifiedTapSettlement({ ...f.args, bundle: { ...f.bundle, receipts: [...f.bundle.receipts].reverse() } });
  assert.equal(reversed.root, result.root);
});

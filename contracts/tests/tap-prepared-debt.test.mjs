import assert from 'node:assert/strict';
import test from 'node:test';
import ganache from '../test-chain/index.mjs';
import { BrowserProvider, Contract, ContractFactory, Wallet, toBeHex } from 'ethers';
import { compileAll } from '../scripts/compile.mjs';
import { signRootProposal } from '../scripts/pool-governance.mjs';
import Mayhem, { CONTRACT_VERSION, payoutPreparationMessage, targetedPayoutControlMessage } from '../../intercom/contract/contract.js';
import { assertTapDebtNativeReplay } from '../../intercom/tests/helpers/tap-debt-feature-replay.mjs';
import Mayhem31 from '../../intercom/contract/v31.js';
import { tapDebtEnvelope, tapDebtKey } from '../../intercom/contract/tap-prepared-debt-v32.js';
import { tapEpochSlot, tapClaimedSlot } from '../../intercom/contract/tap-epoch-proof.js';
import { MemoryStorage, makeIdentity, makeVerifier, executeFeature, executeRateFeature } from '../../intercom/tests/helpers/contract.js';
import { collectTapEpochProof, preparedTapReplay, prepareTapDebtRecovery, restoreTapDebtPrefix, verifyPreparedTapFinancialPolicy,
  confirmTapDebtRecovery, prepareTapAuAlternative, confirmTapAuAlternative, settleTapAuAlternative, rebuildPreparedTapSource, certifyTapDebtBasis, postedTapDebtClaims, screenTapAuProposal } from '../scripts/tap-prepared-debt.mjs';
import { buildCanonicalTapPreparationPlan, settlementBundleSha256, submitCanonicalTapPreparationPlan } from '../scripts/tap-settlement-roller.mjs';
import { MAINNET_POOL as pool, MAINNET_TOKEN as token } from '../scripts/tap-liability-sweep.mjs';

const id = i => toBeHex(i, 32).slice(2);
const evm = i => toBeHex(i, 32);
const au = 10n ** 18n;
const at = e => 100_000 + e * 5000;
const address = n => toBeHex(n, 20);
const originalBundle = (epoch, refunds = []) => ({ epoch, epoch_apply_hash: id(epoch), at: at(epoch), rail: 'tap', receipts: [],
  tap_liability_sweep: { schema_version: 2, signed_length: 31, after: null }, refunds });
const included = value => assert.equal(value?.ok, true, value?.message);
const scope = { chain_id: 1, pool_address: pool, token_address: token, payment_config_ver: 1 };

async function fixture(version = CONTRACT_VERSION) {
  const Implementation = version === 31 ? Mayhem31 : Mayhem;
  const admin = await makeIdentity();
  const sign = msg => Buffer.from(admin.wallet.sign(Buffer.from(msg))).toString('hex');
  const contract = new Implementation({ peer: { wallet: makeVerifier(admin.wallet) } }, {});
  const storage = new MemoryStorage({ admin: admin.publicKey, 'payments/current': { ver: 1, denom: 'au_usd',
    set_by: admin.publicKey, set_by_role: 'admin', rails: ['tap'], tnk: { network: 'mainnet' }, tap: { chain_id: 1, pool_address: pool, token_address: token } } });
  const view = Object.assign(Object.create(Implementation.prototype), { get: async key => (await storage.get(key))?.value ?? null });
  const feature = (key, value, sender = admin.publicKey) => executeFeature(contract, storage, 'mayhem_feature', key, value, sender);
  const scopeRows = async (provider, revision, target, total, epoch) => {
    const earning = { provider, rail: 'tap', denom: 'au_usd', total_au: String(total), held_au: '0', paid_cum_au: '0',
      holdbacks: [], updated_epoch: epoch, updated_at: 'epoch/targeted/' + epoch, last_holdback_release_epoch: epoch,
      chain_id: 1, pool_address: pool };
    await storage.put('prov/' + provider, { provider, status: 'active' });
    const earningKey = view.earningKey(provider, 'tap');
    const aggregate = await view.get(earningKey);
    await storage.put(earningKey, { ...earning, ...(aggregate ?? {}), updated_epoch: epoch, total_au: String(BigInt(aggregate?.total_au ?? 0) + total) });
    const revisionKey = view.providerPayoutLiabilityKey(provider, 'tap', revision);
    const prior = await view.get(revisionKey);
    await storage.put(revisionKey, { ...earning, ...(prior ?? {}), type: 'provider_payout_liability',
      total_au: String(BigInt(prior?.total_au ?? 0) + total), updated_epoch: epoch, revision, target, currency: null });
    await storage.put(view.providerPayoutBindingFeatureKey('tap', provider, revision), { type: 'provider_payout_binding',
      verified: true, provider, rail: 'tap', revision, target, currency: null, chain_id: 1,
      network: 'mainnet', payment_config_version: 1, activation_epoch: epoch });
  };
  async function rate(ts, price = au) {
    included(await executeRateFeature(contract, storage, { op: 'tap_rate_oracle', tap_usd_au: String(price),
      ts, source: 'uniswap-v2-twap-median' }, admin.publicKey));
    return await view.get('tap/rate/latest');
  }
  async function makePlan(epoch, specs, refunds = [], prior, rawBundle = originalBundle(epoch, refunds)) {
    const r = await rate(at(epoch));
    const cursors = new Map(), claims = new Map((prior?.provider_entries ?? []).map(r => [r.account, BigInt(r.cumulative_wei)]));
    const outputs = [];
    for (const [who, rev, account, paid] of specs) {
      await scopeRows(who, rev, account, paid, epoch);
      const before = cursors.get(who) ?? BigInt((await view.get(view.earningKey(who, 'tap'))).paid_cum_au), priorWei = claims.get(account) ?? 0n;
      outputs.push({ provider: who, payout_revision: rev, to: account, net_au_paid: String(paid), tap_wei: String(paid),
        paid_cum_au_before: (await view.get(view.providerPayoutLiabilityKey(who, 'tap', rev))).paid_cum_au, aggregate_paid_cum_au_before: String(before),
        previous_cumulative_wei: String(priorWei), new_cumulative_wei: String(priorWei + paid) });
      cursors.set(who, before + paid);
      claims.set(account, priorWei + paid);
    }
    const rows = m => [...m].sort(([a],[b]) => a.localeCompare(b)).map(([account,n]) => ({ account, cumulative_wei: String(n) }));
    const providers = rows(claims);
    const amount = [...claims.values()].reduce((a,b) => a + b, 0n);
    for (const row of refunds) claims.set(row.account, (claims.get(row.account) ?? 0n) + BigInt(row.cumulative_wei));
    const entries = rows(claims);
    const tapRateLock = { type: 'tap_settlement_rate_lock', epoch, bundle_sha256: settlementBundleSha256(rawBundle), denom: r.denom,
      tap_usd_au: r.tap_usd_au, source: r.source, rate_ts: r.ts, rate_record_key: r.updated_at,
      posted_by: r.posted_by, posted_by_role: 'admin', ...scope };
    return buildCanonicalTapPreparationPlan({ epoch, epochApplyHash: id(epoch), bundle: { epoch, at: at(epoch) },
      tapRateLock, settlement: { root: view.targetedTapSettlementRoot(entries), ledger_fee_bps: 1500,
        cumulative_spent_wei: String((amount * 4n + 2n) / 3n), provider_claimed_wei: String(amount),
        buyer_refund_wei: String(refunds.reduce((a,r) => a + BigInt(r.cumulative_wei), 0n)),
        checkpoint_outputs: outputs, providers, entries, refunds } });
  }
  const prepare = async (plan, start = 0, end = plan.preparations.length) => {
    for (const p of plan.preparations.slice(start, end)) {
      const v = await tapDebtEnvelope(view, plan.intent, p, admin.publicKey, version);
      v.admin_sig = sign(payoutPreparationMessage(v));
      included(await feature(await view.targetedPayoutPreparationFeatureKey(v), v));
    }
  };
  const checkpoint = (plan, extra = {}) => {
    const { intent, root_preparation_id, preparation_ids, external_effect_ids } = plan;
    const n = intent.epoch * 10;
    return { ...intent, root_preparation_id, preparation_ids, external_effect_ids, root_confirmed: true,
      proposal_tx: evm(n), proposal_block_number: n, proposal_block_hash: evm(n+1), proposal_block_timestamp: intent.at,
      execution_tx: evm(n+2), execution_status: 1, execution_block_number: n+1, execution_block_hash: evm(n+3),
      finalized_block_number: n+13, confirmation_depth: 12, confirmation_policy: 'finalized-tag', ...extra };
  };
  async function settle(cp) {
    const key = await view.targetedTapSettlementFeatureKey(cp);
    return key instanceof Error ? key : feature(key, cp);
  }
  for (const e of [1,2,3,4]) {
    await storage.put(view.epochApplyAnchorKey(e), { type: 'epoch_apply_anchor', epoch: e, apply_hash: id(e),
      settlement_unix: at(e), applied_at: 'epoch/' + e });
  }
  await storage.put('epoch/apply/state', { updated_epoch: 4, last_apply_hash: id(4), last_settlement_unix: at(4), pending_epoch: null });
  return { admin, storage, contract, view, sign, feature, makePlan, prepare, checkpoint, settle, rate };
}

async function ethereumProof(t, f, basis, source, originallyPosted, claimed, openingCheckpoint, untrackedHead, previousCheckpoint = openingCheckpoint) {
  const chain = ganache.provider({ logging: { quiet: true }, wallet: { deterministic: true, totalAccounts: 1 } });
  t.after(() => chain.disconnect());
  const rpc = (method, params) => chain.request({ method, params });
  const state = [basis.intent.root, basis.intent.epoch, basis.intent.cumulative_spent_wei];
  const opening = untrackedHead ?? (previousCheckpoint && [previousCheckpoint.root, previousCheckpoint.epoch, previousCheckpoint.cumulative_spent_wei]);
  for (let i = 0; i < (previousCheckpoint?.execution_block_number ?? 0); i++) await rpc('evm_mine', []);
  const headSlots = [18, 20, 21];
  if (opening) for (const [i, val] of opening.entries()) {
    await rpc('evm_setAccountStorageAt', [pool, evm(headSlots[i]), evm(BigInt(val))]);
  }
  if (claimed) await rpc('evm_setAccountStorageAt', [pool, tapClaimedSlot(source.intent.outputs[0].to), toBeHex(claimed, 32)]);
  if (originallyPosted) await rpc('evm_setAccountStorageAt', [pool, tapEpochSlot(source.intent.epoch), originallyPosted]);
  // Update all post-execution slots in one block so the archived parent is
  // the real prior head. The compiled-pool test below checks the same layout.
  const store = (slot, value) => '7f' + id(BigInt(value)) + '7f' + id(BigInt(slot)) + '55';
  const code = '0x' + store(tapEpochSlot(basis.intent.epoch), basis.intent.root) +
    headSlots.map((slot, i) => store(slot, state[i])).join('') + '00';
  await rpc('evm_setAccountCode', [pool, code]);
  const [sender] = await rpc('eth_accounts', []);
  const hash = await rpc('eth_sendTransaction', [{ from: sender, to: pool, gas: '0x50000' }]);
  assert.equal((await rpc('eth_getTransactionReceipt', [hash])).status, '0x1');
  const block = await rpc('eth_getBlockByNumber', ['latest', false]);
  const number = Number(BigInt(block.number));
  const cp = f.checkpoint(basis, { execution_block_hash: block.hash, execution_block_number: number, proposal_block_number: number - 1,
    finalized_block_number: number + 12 });
  included(await f.settle(cp));
  // Ganache accepts a block number here. Pin and check the requested hash,
  // then feed Ganache's actual serialized MPT nodes to the reducer.
  const read = async (method, params) => {
    if (method !== 'eth_getProof') return rpc(method, params);
    assert([block.hash, block.parentHash].includes(params[2].blockHash) && params[2].requireCanonical);
    const pinned = await rpc('eth_getBlockByHash', [params[2].blockHash, false]);
    return rpc(method, [params[0], params[1], pinned.number]);
  };
  return collectTapEpochProof({ checkpoint: cp, source, openingCheckpoint, previousCheckpoint, read });
}

test('no later provider: same prepared root accepts only a renewed observation at the mined proposal', async () => {
  const f = await fixture();
  const source = await f.makePlan(1, [[id(41), id(51), address(21), 3n*au]], [{ account: address(21), cumulative_wei: '507' }]);
  await f.prepare(source);
  const financialAt = at(1) + 3000;
  const cp = f.checkpoint(source, { proposal_block_timestamp: financialAt });
  assert.match((await f.settle(cp)).message, /stale/);
  const renew = async (ts, price) => (await f.rate(ts, price)).updated_at;
  const changed = await renew(financialAt - 1, au+1n);
  assert.match((await f.settle({ ...cp, proposal_rate_record_key: changed })).message, /changes the prepared price/);
  const key = await renew(financialAt, au);
  const fetchImpl = async url => {
    const k = url.searchParams.get('key');
    return Response.json({ key: k, value: await f.view.get(k), confirmed: true, signed_length: 82 });
  };
  const policy = { source, financialAt, rateRecordKey: key, peerRpcUrl: 'http://fixture.invalid', fetchImpl };
  const basis = { epoch: 0, root: evm(0), spent: 0n };
  await assert.rejects(verifyPreparedTapFinancialPolicy({ ...policy, poolBasis: { ...basis, spent: 1n } }), /pool does not match/);
  const { checkpoint_fields, plan } = await verifyPreparedTapFinancialPolicy({ ...policy, poolBasis: basis });
  assert.deepEqual(plan, source);
  const receipt = { ...cp, ...checkpoint_fields };
  assert.match((await f.settle({ ...receipt, proposal_block_timestamp: financialAt-1 })).message, /future/);
  assert.match((await f.settle({ ...receipt, proposal_block_timestamp: financialAt+2701 })).message, /stale/);
  included(await f.settle(receipt));
  await renew(financialAt + 7_000, au*2n);
  const state = f.storage.snapshotBytes();
  assert.equal((await f.settle(receipt)).idempotent, true);
  assert.equal(f.storage.snapshotBytes(), state);
  assert.equal((await f.view.get('settle/targeted/tap/1')).proposal_rate_record_key, key);
  assert.deepEqual((await verifyPreparedTapFinancialPolicy(policy)).checkpoint, receipt);
});

async function recoveryFixture(t, { originallyPosted = false, prefix = false, overlap = false, claimed = 0n, untrackedHead } = {}) {
  const f = await fixture(), who = id(41), other = id(42), payee = address(21);
  const source = await f.makePlan(1, [[who, id(51), payee, 3n*au],[who, id(52), payee, 6n*au]],
    [{ account: payee, cumulative_wei: '507' }]);
  const frozen = new Map(f.storage.values);
  frozen.set('epoch/apply/state', { updated_epoch: 1, last_apply_hash: id(1), pending_epoch: null,
    last_settlement_unix: at(1), updated_at: 'epoch/1' });
  const frozenFetch = async url => {
    const q = url.searchParams;
    assert.equal(q.get('signed_length'), '31');
    const key = q.get('key');
    const body = key ? { key, value: frozen.get(key) ?? null }
      : { prefix: q.get('prefix'), values: [...frozen].filter(([k]) => k.startsWith(q.get('prefix'))).map(([key,value]) => ({ key,value })),
        truncated: false, next_cursor: null };
    return Response.json({ ...body, confirmed: true, signed_length: 31 });
  };
  await f.prepare(source, 0, prefix ? 1 : undefined);
  const disjoint = await f.makePlan(2, [[other, id(53), overlap ? payee : address(22), 12n*au]],
    [{ account: address(23), cumulative_wei: '191' }]);
  await f.prepare(disjoint);
  const proof = await ethereumProof(t, f, disjoint, source, originallyPosted && source.intent.root, claimed, undefined, untrackedHead);
  const rate = await f.rate(at(3));
  const args = { contract: f.view, source, openingEpoch: 0, basisEpoch: 2, epoch: 3, epochApplyHash: id(3),
    rateRecordKey: rate.updated_at, financialAt: rate.ts, skippedEpochProof: proof, sign: f.sign };
  return { ...f, args, source, who, payee, frozenFetch };
}

test('finalized skipped epoch, two old revisions, merged refund, partial prepare and new earnings', async t => {
  const f = await recoveryFixture(t, { prefix: true });
  await assert.rejects(prepareTapDebtRecovery(f.args), /unconsumed canonical preparation/);
  const restore = { contract: f.view, source: f.source, openingEpoch: 0, basisEpoch: 2, skippedEpochProof: f.args.skippedEpochProof,
    originalBundle: originalBundle(1, f.source.intent.refunds), frozenFetch: f.frozenFetch, sign: f.sign, sealed: { epoch: 1, epoch_apply_hash: id(1),
      root_preparation_id: f.source.root_preparation_id, preparation_ids: f.source.preparation_ids,
      external_effect_ids: f.source.external_effect_ids } };
  await assert.rejects(restoreTapDebtPrefix({ ...restore, originalBundle: { ...restore.originalBundle, refunds: [] } }), /digest/);
  const rebuilt = await rebuildPreparedTapSource(restore);
  assert.deepEqual(rebuilt, f.source);
  const restored = await restoreTapDebtPrefix({ ...restore, source: rebuilt });
  assert.equal(restored.features.length, 2);
  for (const { key, value } of restored.features) included(await f.feature(key, value));
  assert.deepEqual((await restoreTapDebtPrefix(restore)).features, []);
  for (const key of [f.view.earningKey(f.who, 'tap'), f.view.providerPayoutLiabilityKey(f.who, 'tap', id(51))]) {
    const row = await f.view.get(key);
    await f.storage.put(key, { ...row, total_au: String(BigInt(row.total_au) + 5n*au), updated_epoch: 3 });
  }
  const { plan, feature: { key, value } } = await prepareTapDebtRecovery(f.args);
  const before = f.storage.snapshotBytes();
  const reject = async (mutate, pattern, sender = f.admin.publicKey) => {
    const v = structuredClone(value); mutate(v); v.admin_sig = f.sign(targetedPayoutControlMessage(v));
    const r = await f.feature(await tapDebtKey(f.view, v), v, sender);
    assert.match(r.message, pattern);
    assert.equal(f.storage.snapshotBytes(), before);
  };
  await reject(v => { v.preparation_signatures[2] = '00'.repeat(64); }, /signature/);
  await reject(v => { v.source.intent.refunds[0].cumulative_wei = '510'; }, /root mismatch|do not match|do not merge/);
  await reject(v => { v.skipped_epoch_proof.skipped = []; }, /exclusion/);
  await reject(v => { v.skipped_epoch_proof.header += '00'; }, /exclusion/);
  await reject(v => { v.skipped_epoch_proof.parent_header += '00'; }, /exclusion/);
  await reject(v => { v.source.intent.outputs[0].paid_au = String(8n*au); }, /locked rate/);
  await reject(v => {}, /admin required|authority/i, id(99));
  const confirm = () => confirmTapDebtRecovery({ contract: f.view, plan, feature: { key, value } });
  await assert.rejects(confirm(), /No matching confirmed recovery/);
  included(await f.feature(key, value));
  const transferred = f.storage.snapshotBytes();
  assert(await f.settle(f.checkpoint(f.source)) instanceof Error);
  assert.equal(f.storage.snapshotBytes(), transferred);
  assert.equal((await confirm()).status, 'prepared');
  assert.equal((await f.feature(key, value)).idempotent, true);
  assert.equal((await f.view.get(f.view.earningKey(f.who, 'tap'))).paid_cum_au, '0');
  assert.equal(plan.intent.provider_paid_au, String(9n*au));
  assert.deepEqual(plan.intent.outputs, f.source.intent.outputs);
  assert.deepEqual(plan.intent.refunds, [{ account: address(21), cumulative_wei: '507' }, { account: address(23), cumulative_wei: '191' }]);
  assert.equal(plan.intent.cumulative_spent_wei, String(28n*au));
  const cp = f.checkpoint(plan);
  included(await f.settle(cp));
  assert.equal((await confirm()).status, 'settled');
  const after = f.storage.snapshotBytes();
  assert.equal((await f.settle(cp)).idempotent, true);
  assert.equal((await f.feature(key, value)).idempotent, true);
  assert.equal(f.storage.snapshotBytes(), after);
  const earn = await f.view.get(f.view.earningKey(f.who, 'tap'));
  assert.equal(earn.paid_cum_au, String(9n*au));
  assert.equal(BigInt(earn.total_au)-BigInt(earn.paid_cum_au), 5n*au);
  const tail = await f.view.get(f.view.payoutPreparationAggregateTailKey('tap', f.who));
  assert.equal(tail.economic_op_id, plan.preparation_ids[1]);
  assert.equal(tail.consumed_by, 'settle/targeted/tap/3');
  // Replay from the prefix through real dispatch, including root and effect locks.
  const replay = MemoryStorage.fromSnapshotBytes(before);
  const c = new Mayhem({ peer: { wallet: makeVerifier(f.admin.wallet) } }, {});
  included(await executeFeature(c, replay, 'mayhem_feature', key, value, f.admin.publicKey));
  included(await executeFeature(c, replay, 'mayhem_feature', await f.view.targetedTapSettlementFeatureKey(cp), cp, f.admin.publicKey));
  assert.equal(replay.snapshotBytes(), after);
});

test('archived historic root remains claimable; later execution cannot release the debt', async t => {
  await assert.rejects(recoveryFixture(t, { originallyPosted: true }), /exclusion/);
});

test('later entitlement for same payee blocks substitution even with an authentic absence proof', async t => {
  const f = await recoveryFixture(t, { overlap: true });
  await assert.rejects(prepareTapDebtRecovery(f.args), /intervening entitlement/);
});

test('unreconciled historical on-chain claim at the affected address blocks recovery', async t => {
  await assert.rejects(recoveryFixture(t, { claimed: 1n }), /exclusion/);
});

test('an untracked intervening pool head blocks recovery even before anyone has claimed it', async t => {
  await assert.rejects(recoveryFixture(t, { untrackedHead: [evm(71), 1, 12n*au] }), /exclusion/);
});

test('restores the exact increment above a nonzero claim, aggregate, revision and refund prefix', async t => {
  const f = await fixture(), who = id(41), rev = id(51), payee = address(21);
  const opening = await f.makePlan(1, [[who, rev, payee, 3n*au]], [{ account: payee, cumulative_wei: '200' }]);
  await f.prepare(opening);
  included(await f.settle(f.checkpoint(opening)));
  const source = await f.makePlan(2, [[who, rev, payee, 6n*au]], [{ account: payee, cumulative_wei: '225' }], opening.intent);
  await f.prepare(source);
  const basis = await f.makePlan(3, [[id(42), id(52), address(22), 12n*au]], [
    { account: payee, cumulative_wei: '200' }, { account: address(23), cumulative_wei: '191' }], opening.intent);
  await f.prepare(basis);
  const proof = await ethereumProof(t, f, basis, source, false, 3n*au+185n, f.checkpoint(opening));
  const rate = await f.rate(at(4));
  const { plan, feature } = await prepareTapDebtRecovery({ contract: f.view, source, openingEpoch: 1, basisEpoch: 3,
    epoch: 4, epochApplyHash: id(4), rateRecordKey: rate.updated_at, financialAt: rate.ts, skippedEpochProof: proof, sign: f.sign });
  included(await f.feature(feature.key, feature.value));
  included(await f.settle(f.checkpoint(plan)));
  assert.equal(plan.intent.provider_paid_au, String(6n*au));
  assert.equal(plan.intent.outputs[0].paid_cum_au_before, String(3n*au));
  assert.equal(plan.intent.outputs[0].prior_cumulative_claim_wei, String(3n*au));
  assert.equal(plan.intent.entries.find(r => r.account === payee).cumulative_wei, String(9n*au+225n));
  assert.equal(plan.intent.buyer_refund_wei, '416');
  assert.equal(plan.intent.cumulative_spent_wei, String(28n*au));
});

test('authenticates the actual compiled pool layout and a governance-posted root using in-process Ethereum proofs', async t => {
  const chain = ganache.provider({ logging: { quiet: true }, chain: { chainId: 1 }, wallet: { deterministic: true } });
  const provider = new BrowserProvider(chain);
  t.after(async () => { provider.destroy(); await chain.disconnect(); });
  const owner = await provider.getSigner(), governance = Wallet.createRandom(), artifacts = compileAll();
  const deploy = async (artifact, args) => {
    const deployed = await new ContractFactory(artifact.abi, artifact.bytecode, owner).deploy(...args);
    return deployed.waitForDeployment();
  };
  const token = await deploy(artifacts.MockTTAP, []);
  const pool = await deploy(artifacts.MayhemInferencePool, [await token.getAddress(), await owner.getAddress(), governance.address, 3600, 1_000_000]);
  const root = evm(7);
  const sig = await signRootProposal({ signer: governance, pool, merkleRoot: root, newEpoch: 2, newCumulativeSpent: 0 });
  await (await pool.proposeRoot(root, 2, 0, sig)).wait();
  const rpc = (method, params) => chain.request({ method, params });
  await rpc('evm_increaseTime', [3601]);
  await rpc('evm_mine', []);
  const receipt = await (await pool.executeRoot()).wait();
  for (let i = 0; i < 12; i++) await rpc('evm_mine', []);
  const checkpoint = { chain_id: 1, root, pool_address: (await pool.getAddress()).toLowerCase(), epoch: 2, cumulative_spent_wei: '0', root_confirmed: true,
    execution_block_hash: receipt.blockHash, execution_block_number: receipt.blockNumber, execution_status: 1,
    finalized_block_number: receipt.blockNumber+12, confirmation_depth: 12, confirmation_policy: 'finalized-tag' };
  const read = async (method, params) => method === 'eth_getProof'
    ? rpc(method, [params[0], params[1], (await rpc('eth_getBlockByHash', [params[2].blockHash, false])).number]) : rpc(method, params);
  const proof = await collectTapEpochProof({ checkpoint, source: { intent: { epoch: 1, outputs: [{ to: address(21) }], refunds: [] } }, read });
  assert(proof.account.length && proof.posted.length && proof.skipped.length && proof.claimed[address(21)].length);
});

test('different oracle values may renew only if all integer payouts agree and refunds are carried intact', async () => {
  const run = async withRefund => {
    const f = await fixture(), payee = address(21);
    const source = await f.makePlan(1, [[id(41), id(51), payee, 1000n]],
      withRefund ? [{ account: payee, cumulative_wei: '41' }] : []);
    await f.prepare(source);
    const r = await f.rate(at(1)+3000, au - 1n);
    const cp = f.checkpoint(source, { proposal_block_timestamp: r.ts, proposal_rate_record_key: r.updated_at });
    return f.settle(cp);
  };
  included(await run(false));
  assert.match((await run(true)).message, /cannot revalue an original refund/);
});

test('a conflicting aggregate tail rejects the old checkpoint without paid writes', async () => {
  const f = await fixture(), who = id(41);
  const source = await f.makePlan(1, [[who, id(51), address(21), 3n*au]]);
  await f.prepare(source);
  const tailKey = f.view.payoutPreparationAggregateTailKey('tap', who);
  await f.storage.put(tailKey, { ...await f.view.get(tailKey), economic_op_id: id(11) });
  const before = f.storage.snapshotBytes();
  assert.match((await f.settle(f.checkpoint(source))).message, /aggregate tail mismatch/);
  assert.equal(f.storage.snapshotBytes(), before);
});

// Use the actual public pool runtime and storage layout at the fixed addresses
// in a private in-process chain. Only the test token and synthetic genesis
// constructor values are installed. All economic transitions run as bytecode.
async function executeAuWinner(t, f, plan, source, { candidate, proposalAt = plan.intent.tap_rate_lock.rate_ts } = {}) {
  const chain = ganache.provider({ logging: { quiet: true }, chain: { chainId: 1, time: new Date((at(1)-50)*1000) },
    wallet: { deterministic: true } });
  const provider = new BrowserProvider(chain, undefined, { cacheTimeout: -1 }), rpc = (method, params) => chain.request({ method, params });
  t.after(async () => { provider.destroy(); await chain.disconnect(); });
  const owner = await provider.getSigner(), addr = await owner.getAddress(), governance = Wallet.createRandom();
  const artifacts = compileAll();
  async function install(artifact, args, address) {
    const template = await new ContractFactory(artifact.abi, artifact.bytecode, owner).deploy(...args);
    await template.waitForDeployment();
    await rpc('evm_setAccountCode', [address, await provider.getCode(await template.getAddress())]);
    return new Contract(address, artifact.abi, owner);
  }
  const tap = await install(artifacts.MockTTAP, [], token);
  const poolContract = await install(artifacts.MayhemInferencePool, [token, addr, governance.address, 3600, 100n*au], pool);
  await rpc('evm_setAccountStorageAt', [pool, evm(0), evm(BigInt(addr))]);
  await rpc('evm_setAccountStorageAt', [pool, evm(2), evm(100n*au)]);
  await (await tap.mint(addr, 50n*au)).wait();
  await (await tap.approve(pool, 50n*au)).wait();
  await (await poolContract.deposit(50n*au)).wait();
  const signature = intent => signRootProposal({ signer: governance, pool: poolContract, merkleRoot: intent.root,
    newEpoch: intent.epoch, newCumulativeSpent: BigInt(intent.cumulative_spent_wei) });
  const old = source.intent, next = plan.intent, originalSig = await signature(old);
  await rpc('evm_setTime', [proposalAt * 1000]);
  await rpc('evm_mine', []);
  const fetchImpl = async url => {
    const key = url.searchParams.get('key');
    return Response.json({ key, value: await f.view.get(key), confirmed: true, signed_length: 77 });
  };
  const screen = candidate && await screenTapAuProposal({ candidate, pool: poolContract, peerRpcUrl: 'http://fixture.invalid', fetchImpl });
  if (candidate) assert.equal(screen.status, 'ready');
  const sig = screen ? await signRootProposal({ ...screen.proposal, signer: governance, pool: poolContract }) : await signature(next);
  const proposed = await (await poolContract.proposeRoot(next.root, next.epoch, next.cumulative_spent_wei, sig)).wait();
  const originalAttempt = () => poolContract.proposeRoot.staticCall(old.root, old.epoch, old.cumulative_spent_wei, originalSig);
  await assert.rejects(originalAttempt, /root pending/);
  if (candidate) assert.equal((await screenTapAuProposal({ candidate, pool: poolContract, peerRpcUrl: 'http://fixture.invalid', fetchImpl })).status, 'pending');
  await rpc('evm_increaseTime', [3601]);
  await rpc('evm_mine', []);
  const executed = await (await poolContract.executeRoot()).wait();
  await assert.rejects(originalAttempt, /epoch !monotonic/);
  const payee = next.entries[0].account;
  if (old.root !== next.root) await assert.rejects(poolContract.claim.staticCall(old.epoch, payee, old.entries[0].cumulative_wei, []), /bad proof/);
  await (await poolContract.claim(next.epoch, payee, next.entries[0].cumulative_wei, [])).wait();
  assert.equal(await tap.balanceOf(payee), BigInt(next.entries[0].cumulative_wei));
  assert.equal(await poolContract.operatorClaimable(), BigInt(next.cumulative_spent_wei) * 15n / 100n);
  assert.equal(await poolContract.burnClaimable(), BigInt(next.cumulative_spent_wei) / 10n);
  for (let i = 0; i < 12; i++) await rpc('evm_mine', []);
  const cp = f.checkpoint(plan, { proposal_tx: proposed.hash, execution_tx: executed.hash,
    proposal_block_number: proposed.blockNumber, proposal_block_hash: proposed.blockHash,
    proposal_block_timestamp: Number(BigInt((await rpc('eth_getBlockByHash', [proposed.blockHash, false])).timestamp)),
    execution_block_hash: executed.blockHash, execution_block_number: executed.blockNumber,
    finalized_block_number: executed.blockNumber + 12 });
  const read = async (method, params) => method === 'eth_getProof'
    ? rpc(method, [params[0], params[1], (await rpc('eth_getBlockByHash', [params[2].blockHash, false])).number]) : rpc(method, params);
  return { cp, proof: await collectTapEpochProof({ checkpoint: cp, source, read }) };
}

test('one waiting AU payee, doubled market price, no later epoch or business: only the finalized winner moves money', async t => {
  const f = await fixture(), who = id(41), payee = address(21), rev = id(51);
  const refunds = [{ account: payee, cumulative_wei: '507' }];
  const source = await f.makePlan(1, [[who, rev, payee, 3n*au]], refunds);
  await f.prepare(source);
  const raw = originalBundle(1, refunds);
  const fresh = await f.rate(at(1) + 3000, 2n*au);
  const args = { contract: f.view, source, originalBundle: raw, openingEpoch: 0, rateRecordKey: fresh.updated_at,
    financialAt: fresh.ts, sign: f.sign };
  const candidate = await prepareTapAuAlternative(args), { feature, plan } = candidate;
  const confirm = () => confirmTapAuAlternative({ contract: f.view, candidate });
  await assert.rejects(confirm(), /No confirmed/);
  assert.equal(plan.intent.epoch, source.intent.epoch);
  assert.equal(plan.intent.provider_paid_au, String(3n*au));
  assert.equal(plan.intent.provider_tap_wei, String(3n*au/2n));
  assert.deepEqual(plan.intent.refunds, refunds);
  for (const key of [f.view.earningKey(who, 'tap'), f.view.providerPayoutLiabilityKey(who, 'tap', rev)]) {
    const row = await f.view.get(key);
    await f.storage.put(key, { ...row, total_au: String(8n*au), updated_epoch: 3, last_holdback_release_epoch: 3,
      held_au: String(2n*au), holdbacks: [{ epoch: 3, au: String(2n*au), locked_epochs: 12 }] });
  }
  const snapshot = f.storage.snapshotBytes();
  included(await f.feature(feature.key, feature.value));
  assert.equal((await confirm()).status, 'authorized');
  const authorized = f.storage.snapshotBytes();
  assert.equal((await f.feature(feature.key, feature.value)).idempotent, true);
  assert.equal(f.storage.snapshotBytes(), authorized);
  const oldRecord = await f.view.get(f.view.payoutPreparationRecordKey('tap', source.preparation_ids[0]));
  const originalLock = f.view.payoutPreparationLiabilityLockKey('tap', oldRecord.liability);
  assert.equal((await f.view.get(originalLock)).economic_op_id, source.preparation_ids[0]);
  assert.equal(await f.view.get(f.view.payoutPreparationRecordKey('tap', plan.preparation_ids[0])), null);
  const { proof, cp } = await executeAuWinner(t, f, plan, source, { candidate });
  assert((await f.settle(cp)) instanceof Error);
  assert.equal(f.storage.snapshotBytes(), authorized);
  const settle = await settleTapAuAlternative({ contract: f.view, candidate, checkpoint: cp, executionProof: proof, sign: f.sign });
  const reject = async (mutate, pattern) => {
    const v = structuredClone(settle.feature.value);
    mutate(v);
    v.admin_sig = f.sign(targetedPayoutControlMessage(v));
    const result = await f.feature(await tapDebtKey(f.view, v), v);
    assert.match(result.message, pattern);
    assert.equal(f.storage.snapshotBytes(), authorized);
  };
  await reject(v => { v.execution_proof.posted = []; }, /exclusion/);
  await reject(v => { v.checkpoint.proposal_block_timestamp += 3000; }, /exclusion|stale/);
  await reject(v => { v.checkpoint.outputs[0].paid_au = String(4n*au); }, /authorized candidate/);
  included(await f.feature(settle.feature.key, settle.feature.value));
  assert.equal((await confirm()).status, 'settled');
  for (const key of [f.view.earningKey(who, 'tap'), f.view.providerPayoutLiabilityKey(who, 'tap', rev)]) {
    const paid = await f.view.get(key);
    assert.equal(paid.paid_cum_au, String(3n*au));
    assert.equal(paid.total_au, String(8n*au));
    assert.equal(paid.held_au, String(2n*au));
    assert.equal(paid.last_holdback_release_epoch, 3);
    assert.equal(paid.updated_epoch, 3);
  }
  const receipt = 'settle/targeted/tap/1';
  assert.equal((await f.view.get(f.view.payoutPreparationRecordKey('tap', plan.preparation_ids[0]))).consumed_by, receipt);
  assert.equal((await f.view.get(f.view.payoutPreparationRecordKey('tap', source.preparation_ids[0]))).superseded_by, plan.preparation_ids[0]);
  const after = f.storage.snapshotBytes();
  assert.equal((await f.feature(settle.feature.key, settle.feature.value)).idempotent, true);
  assert.equal((await f.feature(feature.key, feature.value)).idempotent, true);
  assert((await f.settle(f.checkpoint(source))) instanceof Error);
  assert.equal(f.storage.snapshotBytes(), after);
  const replay = MemoryStorage.fromSnapshotBytes(snapshot);
  const c = new Mayhem({ peer: { wallet: makeVerifier(f.admin.wallet) } }, {});
  for (const env of [feature, settle.feature]) included(await executeFeature(c, replay, 'mayhem_feature', env.key, env.value, f.admin.publicKey));
  assert.equal(replay.snapshotBytes(), after);
  await assertTapDebtNativeReplay(t, f, MemoryStorage.fromSnapshotBytes(snapshot), [feature, settle.feature]);
});

test('C32 roller resumes the exact confirmed C31 liability and root without signing or posting', async () => {
  const f = await fixture(31);
  const source = await f.makePlan(1, [[id(41),id(51),address(21),3n*au]]);
  await f.prepare(source);
  const args = { plan:source, requireExisting:true, peerRpcUrl:'http://fixture.invalid/v1/',
    adminSigner:{publicKey:f.admin.publicKey,sign:()=>assert.fail('must reuse original signatures')},
    fetchImpl:async(url,options)=>{
      assert.equal(options?.method ?? 'GET','GET');
      const key=new URL(url).searchParams.get('key');
      return Response.json({key,confirmed:true,signed_length:70,value:await f.view.get(key)});
    } };
  assert.equal((await submitCanonicalTapPreparationPlan(args)).all_existing,true);
  const key=f.view.payoutPreparationRecordKey('tap',source.preparation_ids[0]);
  const original=await f.view.get(key);
  assert.equal(original.contract_version,31);
  for (const patch of [{contract_version:29},{contract_version:33},{epoch:2},{admin_sig:'00'.repeat(64)}]) {
    const altered={...original,...patch};
    if (!patch.admin_sig)altered.admin_sig=f.sign(payoutPreparationMessage(altered));
    await f.storage.put(key,altered);
    await assert.rejects(submitCanonicalTapPreparationPlan(args),/conflicts with the planned operation/);
  }
});

test('C32 settles a C31 authorized conditional root using only the C31 component signatures', async t => {
  const f = await fixture(31);
  const origin = await f.makePlan(1, [[id(41), id(51), address(21), 3n * au]]);
  await f.prepare(origin);
  const rate = await f.rate(at(1) + 3000, 2n * au);
  const candidate = await prepareTapAuAlternative({ contract:f.view, source:origin, originalBundle:originalBundle(1),
    openingEpoch:0, rateRecordKey:rate.updated_at, financialAt:rate.ts, sign:f.sign });
  const offer = candidate.feature.value;
  offer.contract_version = 31;
  offer.preparation_signatures = [];
  for (const preparation of candidate.plan.preparations) {
    const signed = await tapDebtEnvelope(f.view,candidate.plan.intent,preparation,f.admin.publicKey,31);
    offer.preparation_signatures.push(f.sign(payoutPreparationMessage(signed)));
  }
  offer.admin_sig = f.sign(targetedPayoutControlMessage(offer));
  candidate.feature.key = await tapDebtKey(f.view,offer);
  included(await f.feature(candidate.feature.key,offer));
  const upgraded = new Mayhem(f.contract.protocol,{});
  const reader = Object.assign(Object.create(Mayhem.prototype),{get:key=>f.view.get(key)});
  const apply = (key,value,storage=f.storage)=>executeFeature(upgraded,storage,'mayhem_feature',key,value,f.admin.publicKey);
  assert.equal((await confirmTapAuAlternative({contract:reader,candidate})).status,'authorized');
  const {proof,cp}=await executeAuWinner(t,f,candidate.plan,origin,{candidate});
  const settle=await settleTapAuAlternative({contract:reader,candidate,checkpoint:cp,executionProof:proof,sign:f.sign});
  const component = await tapDebtEnvelope(reader,candidate.plan.intent,candidate.plan.preparations[0],f.admin.publicKey,31);
  component.admin_sig=offer.preparation_signatures[0];
  assert((await reader.targetedPayoutPreparationFeatureKey(component)) instanceof Error,
    'a stored conditional authorization cannot open a fresh C31 preparation Feature on C32');
  const frozen=f.storage.snapshotBytes();
  const storedKey='payout/recovery/tap/au-candidate/'+candidate.plan.root_preparation_id;
  for(const field of ['contract_version','preparation_signatures','to']) {
    const damaged=MemoryStorage.fromSnapshotBytes(frozen);
    const record=structuredClone((await damaged.get(storedKey)).value);
    if(field==='contract_version')record.value.contract_version=29;
    if(field==='preparation_signatures')record.value.preparation_signatures[0]='00'.repeat(64);
    if(field==='to')record.value.source.intent.outputs[0].to=address(99);
    await damaged.put(storedKey,record);
    const afterEdit=damaged.snapshotBytes();
    assert((await apply(settle.feature.key,settle.feature.value,damaged)) instanceof Error);
    assert.equal(damaged.snapshotBytes(),afterEdit);
  }
  included(await apply(settle.feature.key,settle.feature.value));
  const paid=await reader.get(reader.payoutPreparationRecordKey('tap',candidate.plan.preparation_ids[0]));
  assert.equal(paid.contract_version,31);
  assert.equal(paid.admin_sig,offer.preparation_signatures[0]);
  assert.equal(paid.consumed,true);
  const after=f.storage.snapshotBytes();
  assert.equal((await apply(settle.feature.key,settle.feature.value)).idempotent,true);
  assert.equal(f.storage.snapshotBytes(),after);
});

test('timely original wins after conditional AU preparation with a materially different market', async () => {
  const f = await fixture(), who = id(41);
  const source = await f.makePlan(1, [[who, id(51), address(21), 3n*au]]);
  await f.prepare(source);
  const r = await f.rate(at(1)+3000, au/2n);
  const candidate = await prepareTapAuAlternative({ contract: f.view, source, originalBundle: originalBundle(1),
    openingEpoch: 0, rateRecordKey: r.updated_at, financialAt: r.ts, sign: f.sign });
  included(await f.feature(candidate.feature.key, candidate.feature.value));
  included(await f.settle(f.checkpoint(source)));
  assert.equal((await confirmTapAuAlternative({ contract: f.view, candidate })).status, 'original_settled');
  assert.equal((await f.view.get('settle/targeted/tap/1')).provider_tap_wei, String(3n*au));
  await assert.rejects(settleTapAuAlternative({ contract: f.view, candidate, checkpoint: f.checkpoint(candidate.plan), sign: f.sign }), /Another root won/);
  assert.equal((await f.view.get(f.view.earningKey(who,'tap'))).paid_cum_au, String(3n*au));
});

test('an existing proposal-time freshness schedule authorizes the fixed outcome after a market change', async () => {
  const f = await fixture(), source = await f.makePlan(1, [[id(41), id(51), address(21), 3n*au]]);
  await f.prepare(source);
  const ts = at(1)+3000;
  await f.rate(ts-1, 2n*au);
  await f.storage.put('params/rate_staleness_seconds', { key: 'rate_staleness_seconds',
    current: { ver: 1, effective_at: 0, value: 2700 }, pending: { ver: 2, effective_at: ts, value: 3600 } });
  await assert.rejects(preparedTapReplay({ contract: f.view, source, financialAt: ts-1 }), /stale/);
  const replay = await preparedTapReplay({ contract: f.view, source, financialAt: ts });
  assert.deepEqual(replay.checkpoint_fields, {});
  included(await f.settle({ ...f.checkpoint(source), proposal_block_timestamp: ts }));
});

test('after a finalized skip a signed AU sweep only revalues its original provider AU', async t => {
  const f = await recoveryFixture(t);
  const r = await f.rate(at(3)+1, 2n*au);
  const args = { ...f.args, rateRecordKey: r.updated_at, financialAt: r.ts };
  await assert.rejects(prepareTapDebtRecovery(args), /changes the prepared price/);
  const { plan, feature } = await prepareTapDebtRecovery({ ...args, amountBasis: 'authorized_au', originalBundle: originalBundle(1, f.source.intent.refunds) });
  assert.equal(plan.intent.provider_tap_wei, String(9n*au/2n));
  assert.equal(plan.intent.provider_paid_au, String(9n*au));
  assert.deepEqual(plan.intent.refunds, [{ account: address(21), cumulative_wei: '507' },{ account: address(23), cumulative_wei: '191' }]);
  included(await f.feature(feature.key, feature.value));
  included(await f.settle(f.checkpoint(plan, { proposal_block_timestamp: r.ts })));
  assert.equal((await f.view.get(f.view.earningKey(f.who,'tap'))).paid_cum_au, String(9n*au));
});

test('a second later root uses the authenticated first exclusion without an epoch scan', async t => {
  const f = await recoveryFixture(t);
  const first = await f.view.get('settle/targeted/tap/2');
  const next = await f.makePlan(3, [[id(44),id(54),address(24),3n*au]], first.refunds, first);
  await f.prepare(next);
  const proof = await ethereumProof(t,f,next,f.source,null,null,undefined,undefined,first);
  const r = await f.rate(at(4));
  const args = { ...f.args, basisEpoch: 3, previousEpoch: 2, epoch: 4, epochApplyHash: id(4), rateRecordKey: r.updated_at,
    financialAt: r.ts, skippedEpochProof: proof };
  await assert.rejects(prepareTapDebtRecovery(args), /missing certified predecessor/);
  const { feature: certificate } = await certifyTapDebtBasis(f.args);
  included(await f.feature(certificate.key, certificate.value));
  assert.equal((await f.view.get(f.view.earningKey(f.who,'tap'))).paid_cum_au, '0');
  assert.equal((await f.feature(certificate.key, certificate.value)).idempotent, true);
  const recovered = await prepareTapDebtRecovery(args);
  included(await f.feature(recovered.feature.key, recovered.feature.value));
  included(await f.settle(f.checkpoint(recovered.plan)));
});

test('AU alternative keeps a changed AU-denominated buyer refund and sub-wei provider receivable on hold', async () => {
  const f = await fixture(), account = address(21), refund = [{ account, cumulative_wei: '507' }];
  const raw = originalBundle(1, [{ account, amount_au: '507' }]);
  const source = await f.makePlan(1, [[id(41), id(51), account, 3n*au]], refund, null, raw);
  await f.prepare(source);
  const r = await f.rate(at(1)+3000, 2n*au);
  const args = { contract:f.view, source, originalBundle:raw, openingEpoch:0, rateRecordKey:r.updated_at, financialAt:r.ts, sign:f.sign };
  const before = f.storage.snapshotBytes();
  await assert.rejects(prepareTapAuAlternative(args), /buyer refund/);
  await assert.rejects(prepareTapAuAlternative({ ...args, originalBundle: originalBundle(1, refund) }), /unbound AU liability sweep/);
  assert.equal(f.storage.snapshotBytes(), before);
  const g = await fixture(), tiny = await g.makePlan(1, [[id(41),id(51),account,au]]);
  await g.prepare(tiny);
  const extreme = await g.rate(at(1)+3000, 2n*au*au);
  await assert.rejects(prepareTapAuAlternative({ contract:g.view, source:tiny, originalBundle:originalBundle(1), openingEpoch:0,
    rateRecordKey:extreme.updated_at, financialAt:extreme.ts, sign:g.sign }), /rounds to zero/);
});

test('a stale original that really executed has immutable claim proofs and cannot authorize another payment', async t => {
  const f = await fixture(), who = id(41), payee = address(21);
  const source = await f.makePlan(1, [[who,id(51),payee,3n*au]]);
  await f.prepare(source);
  await f.rate(at(1)+2999, 2n*au);
  const { proof, cp } = await executeAuWinner(t, f, source, source, { proposalAt: at(1)+3000 });
  const prior = f.storage.snapshotBytes();
  assert.match((await f.settle(cp)).message, /stale/);
  const result = await postedTapDebtClaims({ contract: f.view, source, checkpoint: cp, openingEpoch: 0, executionProof: proof });
  assert.equal(result.original_quote, 'expired');
  assert.deepEqual(result.claims, [{ account: payee, cumulative_wei: String(3n*au), pool, epoch: 1, proof: [] }]);
  assert.equal(f.storage.snapshotBytes(), prior);
});

test('at the original opening a known partial Core submission can be restored without an Ethereum absence assertion', async () => {
  const f = await fixture();
  const source = await f.makePlan(1, [[id(41),id(51),address(21),3n*au]]);
  await f.prepare(source, 0, 1);
  const sealed = { epoch: 1, epoch_apply_hash: id(1), root_preparation_id: source.root_preparation_id,
    preparation_ids: source.preparation_ids, external_effect_ids: source.external_effect_ids };
  const args = { contract: f.view, sealed, originalBundle: originalBundle(1), openingEpoch: 0 };
  const rebuilt = await rebuildPreparedTapSource(args);
  assert.deepEqual(rebuilt, source);
  const { features } = await restoreTapDebtPrefix({ ...args, source: rebuilt, sign: f.sign });
  assert.equal(features.length, 1);
  included(await f.feature(features[0].key, features[0].value));
  assert.deepEqual(await restoreTapDebtPrefix({ ...args, source: rebuilt, sign: f.sign }), { features: [] });
  assert.equal((await f.view.get(f.view.earningKey(id(41), 'tap'))).paid_cum_au, '0');
});

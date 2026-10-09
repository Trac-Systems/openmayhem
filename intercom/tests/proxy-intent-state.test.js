import test from 'node:test';
import assert from 'node:assert/strict';
import { proxyReservationFixture } from './helpers/proxy-finance.js';
import { proxySpendTermsDigest } from '../contract/proxy-finance.js';
import { readProxyIntentState, validateProxyIntentStateRequest } from '../features/mayhem/proxy-intent-state.js';

const h = n => n.toString(16).padStart(64, '0');
function query(f, requester = f.buyer.publicKey) {
  const { terms, buyer_sig } = f.authorize(f.terms).authorization;
  return { intent: { terms: structuredClone(terms), buyer_sig }, requester, request_nonce: h(999) };
}
async function observe(f, { request = query(f), epoch = 100, hook = null } = {}) {
  let checks = 0; const reads = new Set();
  return readProxyIntentState({ request, verifySignature: f.peer.wallet.verify,
    withCanonicalSnapshot: async (body, options) => {
      assert.equal(options.financial, true);
      const result = await body({ context: { ...f.context, epoch },
        proof: { view_key: h(1), tree_hash: h(2), signed_length: 10, fork: 0 },
        read: key => { reads.add(key); assert.ok(reads.size <= 10); return f.read(key); },
        assertCurrent: async () => { if (hook) await hook(++checks); else checks++; } });
      assert.equal(checks, 2);
      return result;
    } });
}

test('intention is open until completed epoch advances; accepted work never becomes unadmitted', async () => {
  for (const family of ['llm', 'decisions']) for (const rail of ['fiat', 'tnk', 'tap']) {
    const f = await proxyReservationFixture(rail, family);
    const before = JSON.stringify([...f.storage.values]);
    for (const requester of [f.buyer.publicKey, f.provider.publicKey]) {
      const request = query(f, requester);
      assert.equal((await observe(f, { request })).status, 'open');
      const expired = await observe(f, { request, epoch: 101 });
      assert.equal(expired.status, 'expired');
      assert.equal(expired.authorization, null);
      assert.equal(expired.accepted_terms, await proxySpendTermsDigest(f.terms));
      assert.ok(!JSON.stringify(expired).includes(f.payout.target));
    }
    assert.equal(JSON.stringify([...f.storage.values]), before);
    await f.storage.put('epoch/apply/state', { updated_epoch: 100, pending_epoch: 101 });
    assert.equal((await observe(f)).status, 'open', 'pending epoch is not final release evidence');
    await f.storage.put('epoch/apply/state', { updated_epoch: 101, pending_epoch: null });
    await assert.rejects(f.prepare(f.authorize(f.terms)), /active billing epoch/);
    await f.storage.put('epoch/apply/state', { updated_epoch: 100, pending_epoch: null });
    await f.apply(await f.prepare(f.authorize(f.terms)));
    const admitted = await observe(f, { epoch: 200 });
    assert.equal(admitted.status, 'admitted');
    assert.deepEqual(admitted.authorization, f.authorize(f.terms).authorization);
  }
});

test('withdrawn offers, disabled policies, missing payout/funding data and old contracts do not block historical intent recovery', async () => {
  const f = await proxyReservationFixture();
  const request = query(f);
  await f.submit(await f.envelope({ kind: 'withdraw_offer', market_id: f.offer.market_id,
    endpoint: f.offer.endpoint, ctx_bracket: f.offer.ctx_bracket, outcome_class: f.offer.outcome_class,
    revision: f.offer.revision + 1 }));
  for (const key of [f.bindingKey, f.balanceKey, f.summaryKey, `proxy/v1/settlement-policy/${f.terms.settlement_policy_hash}`]) {
    await f.storage.put(key, { invalid: true });
  }
  f.context.contract_version++;
  const before = JSON.stringify([...f.storage.values]);
  assert.equal((await observe(f, { request, epoch: 101 })).status, 'expired');
  assert.equal(JSON.stringify([...f.storage.values]), before);
});

test('foreign callers, unsigned or changed intentions and inconsistent financial footprints fail closed', async () => {
  const f = await proxyReservationFixture();
  assert.throws(() => validateProxyIntentStateRequest({ ...query(f), requester: h(987) }), /party/);
  assert.throws(() => validateProxyIntentStateRequest({ ...query(f), extra: true }), /invalid query/);
  const bad = query(f); bad.intent.buyer_sig = '0'.repeat(128);
  await assert.rejects(observe(f, { request: bad }), /signature/);
  const moved = query(f); moved.intent.terms.network_id = '999';
  moved.intent.buyer_sig = f.authorize(moved.intent.terms).authorization.buyer_sig;
  await assert.rejects(observe(f, { request: moved }), /network/);
  const digest = await proxySpendTermsDigest(f.terms);
  await f.storage.put(f.ledger.receiptBillingKey(f.terms.billing_id), { latest_accepted_terms: digest });
  await assert.rejects(observe(f, { epoch: 200 }), /financial footprint/);
  await f.storage.del(f.ledger.receiptBillingKey(f.terms.billing_id));
  await f.storage.put(f.ledger.targetedSpendSessionIndexKey(f.terms.buyer_pubkey, f.terms.rail, f.terms.session_id),
    { reservation_id: f.terms.reservation_id });
  await assert.rejects(observe(f, { epoch: 200 }), /financial footprint/);
});

test('canonical epoch/fork changes cannot return reusable absence; a later admission is observed afresh', async () => {
  const f = await proxyReservationFixture();
  assert.equal((await observe(f)).status, 'open');
  await assert.rejects(observe(f, { epoch: 101, hook: n => { if (n === 2) throw new Error('snapshot changed'); } }), /snapshot changed/);
  await f.apply(await f.prepare(f.authorize(f.terms)));
  assert.equal((await observe(f, { epoch: 101 })).status, 'admitted');
});

import test from 'node:test';
import assert from 'node:assert/strict';
import { proxyReservationFixture } from './helpers/proxy-finance.js';
import { readProxyQuoteState, validateProxyQuoteStateRequest } from '../features/mayhem/proxy-quote-state.js';

const h = n => n.toString(16).padStart(64, '0');
function query(f) {
  return { requester: f.buyer.publicKey, request_nonce: h(123), billing_id: f.terms.billing_id,
    offer: f.offer, rail: f.terms.rail, settlement_policy_hash: f.terms.settlement_policy_hash };
}
async function quote(f, request = query(f), hook = null) {
  const reads = new Set(); let checks = 0;
  const result = await readProxyQuoteState({ request, verifySignature: f.peer.wallet.verify,
    withCanonicalSnapshot: async (body, options) => {
      assert.equal(options.financial, true);
      return body({ context: { ...f.context, epoch: 100 },
        proof: { view_key: h(1), tree_hash: h(2), signed_length: 10, fork: 0 },
        read: key => { reads.add(key); assert.ok(reads.size <= 128); return f.read(key); },
        assertCurrent: async () => { checks++; if (hook) await hook(checks); } });
    } });
  assert.equal(checks, 2);
  return result;
}

test('quote reads all families and rails without allocating funds or exposing payout targets', async () => {
  for (const family of ['llm', 'decisions']) for (const rail of ['fiat', 'tnk', 'tap']) {
    const f = await proxyReservationFixture(rail, family);
    const before = JSON.stringify([...f.storage.values]);
    const q = await quote(f);
    assert.equal(q.billing, null);
    assert.equal(q.billing_epoch, 101);
    assert.deepEqual(q.offer, f.offer);
    assert.equal(q.payout_revision, f.terms.payout_revision);
    assert.equal(q.payment_terms_hash, f.terms.payment_terms_hash);
    assert.equal(q.funding.reserved_au, '50');
    assert.equal(BigInt(q.funding.available_au), BigInt(f.balance.au) - 50n);
    assert.ok(!JSON.stringify(q).includes(f.payout.target));
    assert.equal(JSON.stringify([...f.storage.values]), before, 'quote performs no writes');
    await f.apply(await f.prepare(f.authorize(f.terms)));
    const held = await quote(f);
    assert.equal(held.billing.active_reservation_id, f.terms.reservation_id);
    assert.equal(held.billing.retry_blocked, false);
    assert.equal(held.funding.reserved_au, String(50n + BigInt(f.terms.max_spend_au)));
  }
});

test('stale, withdrawn, revoked, incompatible and unready-payout offers cannot quote', async () => {
  for (const fault of ['price', 'withdraw', 'revoke', 'policy', 'rail', 'payout', 'quota', 'tap-chain']) {
    const f = await proxyReservationFixture('tap');
    const request = query(f);
    if (fault === 'price') request.offer = { ...f.offer, revision: f.offer.revision + 1 };
    if (fault === 'withdraw') await f.submit(await f.envelope({ kind: 'withdraw_offer',
      market_id: f.offer.market_id, endpoint: f.offer.endpoint, ctx_bracket: f.offer.ctx_bracket,
      outcome_class: f.offer.outcome_class, revision: f.offer.revision + 1 }));
    if (fault === 'revoke') await f.storage.put(`proxy/v1/provider-revoked/${f.provider.publicKey}`, { revoked: true });
    if (fault === 'policy') await f.storage.put(`proxy/v1/settlement-policy/${f.terms.settlement_policy_hash}`, { enabled: false });
    if (fault === 'rail') await f.storage.put(`prov/${f.provider.publicKey}`, { status: 'active', accepted_rails: ['tnk'] });
    if (fault === 'payout') await f.storage.put(f.bindingKey, { ...f.payout, verified: false });
    if (fault === 'quota') await f.storage.put('proxy/v1/reservation-budget', { epoch: 101, count: 1000 });
    if (fault === 'tap-chain') await f.storage.put(f.balanceKey, { ...f.balance, chain_id: 2 });
    await assert.rejects(quote(f, request), undefined, fault);
  }
});

test('quote rejects foreign billing and a changing canonical view without returning partial data', async () => {
  const f = await proxyReservationFixture();
  await f.apply(await f.prepare(f.authorize(f.terms)));
  await assert.rejects(quote(f, { ...query(f), requester: f.provider.publicKey }), /billing identity/);
  await assert.rejects(quote(f, query(f), n => { if (n === 2) throw new Error('canonical fork changed'); }), /canonical fork/);
  for (const extra of [{ buyer: f.buyer.publicKey }, { limits: {} }]) {
    assert.throws(() => validateProxyQuoteStateRequest({ ...query(f), ...extra }));
  }
});

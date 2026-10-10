import test from 'node:test';
import assert from 'node:assert/strict';
import { proxyReservationFixture } from './helpers/proxy-finance.js';
import { readProxyQuoteState, validateProxyQuoteStateRequest } from '../features/mayhem/proxy-quote-state.js';

import { readProxyOfferState, validateProxyOfferStateRequest } from '../features/mayhem/proxy-offer-state.js';

const h = n => n.toString(16).padStart(64, '0');
function query(f) {
  return { requester: f.buyer.publicKey, request_nonce: h(123), billing_id: f.terms.billing_id,
    offer: f.offer, rail: f.terms.rail, settlement_policy_hash: f.terms.settlement_policy_hash };
}
async function quote(f, request = query(f), hook = null, reader = readProxyQuoteState, onRead = null) {
  const reads = new Set(); let checks = 0;
  const result = await reader({ request, verifySignature: f.peer.wallet.verify,
    withCanonicalSnapshot: async (body, options) => {
      assert.equal(options.financial, true);
      return body({ context: { ...f.context, epoch: 100 },
        proof: { view_key: h(1), tree_hash: h(2), signed_length: 10, fork: 0 },
        read: key => { if (onRead) onRead(key); reads.add(key); assert.ok(reads.size <= 128); return f.read(key); },
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
    if (fault !== 'tap-chain') {
      const { billing_id, ...providerRequest } = request;
      providerRequest.requester = f.provider.publicKey;
      await assert.rejects(quote(f, providerRequest, null, readProxyOfferState), undefined, fault);
    }
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

function offerQuery(f) {
  const { billing_id, ...request } = query(f);
  return { ...request, requester: f.provider.publicKey };
}

test('provider observation works without buyer funding and exposes only owned offer payment inputs', async () => {
  for (const family of ['llm', 'decisions']) for (const rail of ['fiat', 'tnk', 'tap']) {
    const f = await proxyReservationFixture(rail, family);
    const buyer = await quote(f);
    await f.storage.put(f.balanceKey, { corrupt: true });
    await f.storage.put(f.summaryKey, { corrupt: true });
    const before = JSON.stringify([...f.storage.values]);
    const provider = await quote(f, offerQuery(f), null, readProxyOfferState, key => {
      assert.notEqual(key, f.balanceKey);
      assert.notEqual(key, f.summaryKey);
      assert.notEqual(key, f.ledger.receiptBillingKey(f.terms.billing_id));
    });
    for (const field of ['context', 'proof', 'billing_epoch', 'market', 'membership',
      'settlement_policy', 'payout_revision', 'payment_terms_hash', 'rules_ver']) {
      assert.deepEqual(provider[field], buyer[field], field);
    }
    assert.equal(provider.requester, f.provider.publicKey);
    assert.equal(provider.funding, undefined);
    assert.equal(provider.billing, undefined);
    assert.equal(provider.billing_id, undefined);
    assert.ok(!JSON.stringify(provider).includes(f.payout.target));
    assert.equal(JSON.stringify([...f.storage.values]), before);
  }
});

test('provider observation rejects foreign identity, extra private fields and canonical changes', async () => {
  const f = await proxyReservationFixture();
  const request = offerQuery(f);
  for (const extra of [{ requester: f.buyer.publicKey }, { billing_id: f.terms.billing_id }, { buyer: f.buyer.publicKey }, { funding: {} }]) {
    assert.throws(() => validateProxyOfferStateRequest({ ...request, ...extra }));
  }
  await assert.rejects(quote(f, request, n => { if (n === 2) throw new Error('canonical fork changed'); },
    readProxyOfferState), /canonical fork/);
});

test('only provider availability follows signed rate changes; exact buyer and provider quotes remain exact', async () => {
  for (const family of ['llm', 'decisions']) for (const rail of ['fiat', 'tnk', 'tap']) {
    const f = await proxyReservationFixture(rail, family);
    const original = offerQuery(f);
    const follow = { ...original, follow_rates: true };
    const first = await quote(f, follow, null, readProxyOfferState);
    assert.deepEqual(first.current_offer, f.offer);
    // Actual signed registry publications, including a later price decrease.
    let previous = f.offer;
    for (const addition of [10n, 1n]) {
      const offer = { ...f.offer, revision: previous.revision + 1,
        rates: f.offer.rates.map(r => ({ ...r, per_unit_au: String(BigInt(r.per_unit_au) + addition) })),
        per_request_au: String(BigInt(f.offer.per_request_au) + addition) };
      assert.equal((await f.submit(await f.envelope({ kind: 'set_offer', offer }))).ok, true);
      const before = JSON.stringify([...f.storage.values]);
      const read = await quote(f, follow, null, readProxyOfferState);
      assert.deepEqual(read.offer, f.offer, 'request echo remains bound to original challenge');
      assert.deepEqual(read.current_offer, offer);
      assert.equal(JSON.stringify([...f.storage.values]), before, 'observation never writes');
      await assert.rejects(quote(f, original, null, readProxyOfferState), /no longer active/);
      await assert.rejects(quote(f), /no longer active/);
      assert.deepEqual((await quote(f, { ...original, offer }, null, readProxyOfferState)).offer, offer);
      previous = offer;
    }
    assert.throws(() => validateProxyQuoteStateRequest({ ...query(f), follow_rates: true }));
    assert.throws(() => validateProxyOfferStateRequest({ ...original, follow_rates: false }));
    assert.throws(() => validateProxyOfferStateRequest({ ...follow, requester: f.buyer.publicKey }));
    await assert.rejects(quote(f, { ...follow, offer: { ...previous, revision: previous.revision + 1 } },
      null, readProxyOfferState), /went backwards/);
    await assert.rejects(quote(f, { ...follow, offer: { ...previous, per_request_au: '99999' } },
      null, readProxyOfferState), /outside its rates/);
    await assert.rejects(quote(f, follow, n => { if (n === 2) throw new Error('canonical fork changed'); },
      readProxyOfferState), /canonical fork/);
    assert.equal((await f.submit(await f.envelope({ kind: 'withdraw_offer', market_id: previous.market_id,
      endpoint: previous.endpoint, ctx_bracket: previous.ctx_bracket, outcome_class: previous.outcome_class,
      revision: previous.revision + 1 }))).ok, true);
    await assert.rejects(quote(f, follow, null, readProxyOfferState), /no longer active/);
  }
});

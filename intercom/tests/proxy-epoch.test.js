import assert from 'node:assert/strict';
import crypto from 'node:crypto';
import test from 'node:test';
import { recomputeEpoch,stableJson } from '../scripts/recompute-epoch-roots.mjs';
import { proxyReceiptFixture } from './helpers/proxy-finance.js';
import { proxyEpochBundle } from './helpers/proxy-epoch.js';
import { nextAttempt } from './helpers/proxy-closure.js';
import { proxyOfferCost } from '../contract/proxy-protocol.js';

async function fixture(rail='tnk',family='llm') {
  const f=await proxyReceiptFixture(rail,family);await f.apply(await f.finalize(await f.receipt()));
  const head=await f.read(f.ledger.receiptHeadKey(f.terms.billing_id,1));
  return {...f,head,bundle:await proxyEpochBundle(f.ledger,[head],101)};
}
function rehash(bundle) {
  const {snapshot_sha256,...snapshot}=bundle.receipt_snapshot;
  bundle.receipt_snapshot.snapshot_sha256=crypto.createHash('sha256').update(stableJson(snapshot)).digest('hex');
}
for(const family of ['llm','decisions'])for(const rail of ['fiat','tnk','tap']) {
  test(`${family}/${rail}: proxy epoch uses actual signed price, preserves backing and has no native demand`,async()=>{
    const f=await fixture(rail,family),r=await recomputeEpoch(f.bundle);
    assert.equal(r.totals.use_au,f.head.incremental_au);
    assert.equal(r.debits[0].rail,rail);assert.equal(r.allocations[0].payout_revision,f.terms.payout_revision);
    assert.deepEqual(r.market_usage,[]);assert.deepEqual(r.market_activity,[]);
    assert.equal(r.roots.use,await f.ledger.merkleRoot('use',[await f.ledger.usageLeafHash(f.head.receipt)]));
    const gross=BigInt(f.head.incremental_au);
    assert.equal(r.totals.fee_au,String(gross*1500n/10000n));
    assert.equal(r.totals.burn_au,String(rail==='tap'?gross*1000n/10000n:0n));
    assert.equal(r.apply_pages.length,1);assert.deepEqual(r.apply_pages[0].market_usage,[]);
  });
}

test('writer rejects altered proxy terms, receipt, policy, identity, lane and unsigned amount overrides',async()=>{
  const f=await fixture();
  const changes=[
    b=>delete b.proxy_acceptances,
    b=>b.proxy_acceptances['0'.repeat(64)]=Object.values(b.proxy_acceptances)[0],
    b=>Object.values(b.proxy_acceptances)[0].authorization.buyer_sig='0'.repeat(128),
    b=>Object.values(b.proxy_acceptances)[0].authorization.terms.offer.per_request_au='1',
    b=>Object.values(b.proxy_acceptances)[0].settlement_policy.allow_checkpoints=false,
    b=>b.receipts[0].receipt.buyer_sig='0'.repeat(128),
    b=>b.receipts[0].receipt.provider_sig=b.receipts[0].receipt.buyer_sig,
    b=>b.receipts[0].receipt.body.usage.input_token=99999,
    b=>b.receipts[0].provider='0'.repeat(64),
    b=>b.receipts[0].payout_revision='0'.repeat(64),
    b=>b.receipts[0].rail='fiat',
    b=>b.receipts[0].settlement_ready=false,
    b=>b.receipts[0].lane='native',
    b=>b.receipts[0].receipt.body.lane='native',
    b=>b.receipts[0].settle_au='1',
    b=>b.receipts[0].au_delta='1',
  ];
  for(const change of changes) {
    const b=structuredClone(f.bundle);change(b);rehash(b);
    await assert.rejects(recomputeEpoch(b),undefined,String(change));
  }
});

test('sequential proxy retry settles its own charge even when prior paid attempt belongs to another epoch',async()=>{
  const f=await fixture(),prior=f.head.incremental_au;
  await f.storage.put('epoch/apply/state',{updated_epoch:101,pending_epoch:null});
  const max_usage=Object.fromEntries(f.terms.offer.rates.map(rate=>[rate.unit,1]));
  Object.assign(f.terms,nextAttempt(f.terms,{billing_epoch:102,acceptance_expires_after_epoch:102,
    prior_spend_au:prior,max_usage,max_spend_au:proxyOfferCost(f.terms.offer,max_usage)}));
  await f.apply(await f.prepare(f.authorize(f.terms)));
  const amount=f.terms.max_spend_au;
  await f.apply(await f.finalize(await f.receipt({quantity:1,billing_au_owed_cum:String(BigInt(prior)+BigInt(amount))})));
  const head=await f.read(f.ledger.receiptHeadKey(f.terms.billing_id,2));
  const result=await recomputeEpoch(await proxyEpochBundle(f.ledger,[head],102));
  assert.equal(result.totals.use_au,amount,'prior epoch charge cannot be billed twice');
  assert.equal(result.totals.use_count,1);
});

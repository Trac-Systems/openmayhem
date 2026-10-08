import assert from 'node:assert/strict';
import test from 'node:test';
import { proxyReservationFixture, proxyReceiptFixture } from './helpers/proxy-finance.js';
import { closure, expiry, nextAttempt } from './helpers/proxy-closure.js';
import { prepareProxyClose, prepareProxyExpiry } from '../contract/proxy-closure.js';
import { proxyOfferCost } from '../contract/proxy-protocol.js';
import { proxyReservationKeys } from '../contract/proxy-reservations.js';
import { proxySettlementPolicyDigest, proxySpendTermsDigest } from '../contract/proxy-finance.js';
const close = (f,e,context=f.context) => prepareProxyClose(f.ledger,e,context,f.peer.wallet.verify);
const expire = (f,e,context=f.context) => prepareProxyExpiry(f.ledger,e,context,f.peer.wallet.verify);

async function expiring() {
  const f = await proxyReservationFixture();
  f.settlementPolicy.hold_expiry = 'release_unfinalized_and_block_retry';
  f.terms.settlement_policy_hash = await proxySettlementPolicyDigest(f.settlementPolicy);
  await f.storage.put(proxyReservationKeys.settlementPolicy(f.terms.settlement_policy_hash),
    {enabled:true,policy:f.settlementPolicy});
  await f.apply(await f.prepare(f.authorize(f.terms)));
  const e = await expiry(f);
  f.context.epoch = e.expiry.body.observed_epoch;
  return f;
}

for (const family of ['llm','decisions']) for (const rail of ['fiat','tnk','tap']) {
  test(`${family}/${rail}: known closure releases once, retains native holds, permits sequential retry`, async () => {
    const f = await proxyReceiptFixture(rail,family), e = await closure(f);
    const before = f.storage.snapshotBytes(); f.reads.length = 0;
    const p = await close(f,e); assert.equal(f.storage.snapshotBytes(),before);
    assert.ok(f.reads.length < 30); assert.equal(p.result.released_au,f.terms.max_spend_au);
    assert.equal(p.result.retry_safe,true); await f.apply(p);
    assert.equal((await f.read(f.summaryKey)).reserved_au,'50');
    assert.deepEqual(await f.read(f.balanceKey),f.balance);
    assert.equal(await f.read(f.ledger.receiptEpochIndexKey(101)),null);
    assert.equal((await f.ledger.targetedSpendReservationState(f.buyer.publicKey,rail,f.terms.reservation_id,f.terms.session_id)).kind,'missing');
    const again = structuredClone(e);
    again.closure.body = Object.fromEntries(Object.entries(e.closure.body).reverse());
    assert.equal((await close(f,again)).duplicate,true);
    const t = nextAttempt(f.terms);
    await f.apply(await f.prepare(f.authorize(t)));
    const anchor = await f.read(f.ledger.receiptBillingKey(t.billing_id));
    assert.equal(anchor.latest_attempt,2); assert.equal(anchor.spent_au,'0');
    assert.equal(anchor.latest_accepted_terms,await proxySpendTermsDigest(t));
    assert.equal((await close(f,e)).duplicate,true,'old ACK recovery must survive next attempt');
    await assert.rejects(f.finalize(await f.receipt()),/anchor|billing|session|reservation/);
    assert.deepEqual(await f.read('payout/epoch/542'),{status:'prepared',native:true});
  });
}

test('known closure validates roles/network/evidence without relying on current offer readiness',async()=>{
  const f=await proxyReceiptFixture(); const e=await closure(f);
  const before=f.storage.snapshotBytes();
  for(const bad of [ {...e,provider:'0'.repeat(64)},
    {...e,closure:{...e.closure,buyer_sig:'0'.repeat(128)}},
    {...e,closure:{...e.closure,provider_sig:e.closure.buyer_sig}},
    {...e,closure:{...e.closure,body:{...e.closure.body,evidence_hash:'0'.repeat(64)}}}]) await assert.rejects(close(f,bad));
  await assert.rejects(close(f,e,{...f.context,network_id:'wrong'}),/network/);
  assert.equal(f.storage.snapshotBytes(),before);
  await f.storage.put(`proxy/v1/provider-revoked/${f.provider.publicKey}`,{reason_hash:'f'.repeat(64)});
  await f.storage.put(proxyReservationKeys.settlementPolicy(f.terms.settlement_policy_hash),{enabled:false});
  await f.apply(await close(f,e,{...f.context,contract_version:99}));
  await assert.rejects(close(f,await closure(f,'failed')),/cannot change/);
});

test('checkpoints cannot claim nonexecution or invent a payable final; genuine known failure can waive charge',async()=>{
  const f=await proxyReceiptFixture(); await f.apply(await f.finalize(await f.receipt({final:false})));
  await assert.rejects(close(f,await closure(f)),/checkpoint/i);
  await f.apply(await close(f,await closure(f,'failed')));
  assert.equal((await f.read(f.summaryKey)).reserved_au,'50');
  assert.equal(await f.read(f.ledger.receiptEpochIndexKey(101)),null);
  await assert.rejects(f.finalize(await f.receipt({seq:2})),/anchor|billing|session/);
});

for(const outcome of ['cancelled','failed','completed_unbilled'])test(`known ${outcome} resolves with actual dual signatures`,async()=>{
  const f=await proxyReceiptFixture(); await f.apply(await close(f,await closure(f,outcome)));
  assert.equal((await f.read(f.summaryKey)).reserved_au,'50');
});

test('expiry requires original opt-in and completed canonical grace, never just clock time or pending epoch',async()=>{
  const absent=await proxyReceiptFixture(), e0=await expiry(absent);
  await assert.rejects(expire(absent,e0,{...absent.context,epoch:999}),/not enabled/);
  const f=await expiring(), e=await expiry(f), deadline=e.expiry.body.observed_epoch-1;
  await assert.rejects(expire(f,e,{...f.context,epoch:deadline}),/grace/);
  await assert.rejects(expire(f,await expiry(f,{observed_epoch:deadline})),/grace/);
  await assert.rejects(expire(f,{...e,buyer:'0'.repeat(64)}),/buyer/);
  await assert.rejects(expire(f,{...e,expiry:{...e.expiry,buyer_sig:'0'.repeat(128)}}),/signature/);
  const before=f.storage.snapshotBytes(),p=await expire(f,e);
  assert.equal(f.storage.snapshotBytes(),before); assert.equal(p.result.retry_safe,false);
  await f.apply(p); assert.equal((await f.read(f.summaryKey)).reserved_au,'50');
  assert.equal((await expire(f,e)).duplicate,true);
  await assert.rejects(f.prepare(f.authorize(nextAttempt(f.terms))),/recover/);
  assert.equal((await f.read(f.ledger.receiptBillingKey(f.terms.billing_id))).retry_blocked,true);
  const known=await close(f,await closure(f,'cancelled'));
  assert.equal(known.result.released_au,'0'); assert.equal(known.result.retry_safe,true);
  assert.equal(known.result.execution,undefined);
  await f.apply(known); assert.equal((await f.read(f.summaryKey)).reserved_au,'50');
  await f.apply(await f.prepare(f.authorize(nextAttempt(f.terms))));
  assert.equal((await expire(f,e)).duplicate,true,'historical expiry ACK cannot alter newer attempt');
});

test('final paid receipt cannot be waived/expired; retry accounts for its retained spend',async()=>{
  const f=await proxyReceiptFixture(); const receipt=await f.receipt();
  await f.apply(await f.finalize(receipt));
  await assert.rejects(close(f,await closure(f)),/anchor|billing|finalized/);
  const paid=receipt.receipt.body.au_owed_cum;
  await assert.rejects(f.prepare(f.authorize(nextAttempt(f.terms))),/prior exposure/);
  await f.storage.put(f.balanceKey,{...f.balance,au:String(BigInt(f.balance.au)*3n)});
  const max_usage=Object.fromEntries(f.terms.offer.rates.map(rate=>[rate.unit,1]));
  const t=nextAttempt(f.terms,{prior_spend_au:paid,max_usage,max_spend_au:proxyOfferCost(f.terms.offer,max_usage)});
  await f.apply(await f.prepare(f.authorize(t)));
  assert.equal((await f.read(f.summaryKey)).reserved_au,String(50n+BigInt(paid)+BigInt(t.max_spend_au)));
  assert.equal((await f.read(f.ledger.receiptBillingKey(t.billing_id))).spent_au,paid);
  assert.equal((await f.finalize(receipt)).duplicate,true,'old final ACK must not change new attempt');
  Object.assign(f.terms,t);
  const second=await f.receipt({quantity:1,billing_au_owed_cum:String(BigInt(paid)+BigInt(t.max_spend_au))});
  await f.apply(await f.finalize(second));
  assert.equal((await f.read(f.ledger.receiptEpochIndexKey(101))).count,2);
  assert.equal((await f.read(f.ledger.receiptBillingKey(t.billing_id))).spent_au,second.receipt.body.billing_au_owed_cum);
  assert.equal((await f.read(f.summaryKey)).reserved_au,String(50n+BigInt(second.receipt.body.billing_au_owed_cum)));

});

test('retry cannot change logical identity, skip attempt, reuse capacity or reset cumulative cap',async()=>{
  const f=await proxyReceiptFixture(); await f.apply(await close(f,await closure(f)));
  const next=nextAttempt(f.terms), before=f.storage.snapshotBytes();
  const alterations=[{billing_attempt:3},{request_hash:'1'.repeat(64)},{endpoint_contract:'2'.repeat(64)},
    {max_total_spend_au:String(BigInt(f.terms.max_total_spend_au)+1n)},{prior_spend_au:'1'},
    {prior_reserved_au:'1'}, {session_id:f.terms.session_id},{reservation_id:f.terms.reservation_id},
    {capacity_lease:f.terms.capacity_lease}];
  for(const changes of alterations)await assert.rejects(f.prepare(f.authorize({...next,...changes})),undefined,JSON.stringify(changes));
  assert.equal(f.storage.snapshotBytes(),before);
});

test('sequential retry can switch to a separately admitted provider without changing buyer rail or logical request',async()=>{
  const f=await proxyReceiptFixture();const original=structuredClone(f.terms);
  await f.apply(await close(f,await closure(f)));
  const {makeIdentity}=await import('./helpers/contract.js');
  const second=await makeIdentity();Object.assign(f.provider,second);
  for(const [i,key] of ['entitlement_id','invoice_commitment','evidence_commitment','nonce'].entries())f.permit[key]=(8000+i).toString(16).padStart(64,'0');
  const member={...f.membership,provider_pubkey:second.publicKey};
  assert.equal((await f.submit(await f.envelope({kind:'join_market',membership:member},{admission:true}))).ok,true);
  const offer={...original.offer,provider_pubkey:second.publicKey};
  assert.equal((await f.submit(await f.envelope({kind:'set_offer',offer}))).ok,true);
  const payout={...f.payout,provider:second.publicKey,target:'second-fixture-target'};
  await f.storage.put(`prov/${second.publicKey}`,{status:'active',accepted_rails:['tnk']});
  await f.storage.put(`payout/binding/tnk/${second.publicKey}/${payout.revision}`,payout);
  await f.storage.put(`payout/current/tnk/${second.publicKey}`,{provider:second.publicKey,rail:'tnk',current_revision:payout.revision,pending_revision:null,pending_activation_epoch:null});
  const {proxyPaymentTermsDigest}=await import('../contract/proxy-reservations.js');
  const next=nextAttempt(original,{offer,payment_terms_hash:await proxyPaymentTermsDigest(await f.read('rules/current'),payout)});
  await f.apply(await f.prepare(f.authorize(next)));
  const state=await f.ledger.targetedSpendReservationState(f.buyer.publicKey,'tnk',next.reservation_id,next.session_id);
  assert.equal(state.session.provider,second.publicKey);
  assert.notEqual(state.session.provider,original.offer.provider_pubkey);
  assert.equal(state.session.authorization.terms.request_hash,original.request_hash);
});

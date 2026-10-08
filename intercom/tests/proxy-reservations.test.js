import assert from 'node:assert/strict';
import test from 'node:test';
import b4a from 'b4a';
import MayhemContract,{CONTRACT_VERSION,closeUsageReservationMessage} from '../contract/contract.js';
import {execute,executeFeature} from './helpers/contract.js';
import {prepareProxyReservation,proxyReservationFeatureKey,
  proxyReservationKeys,normalizeProxySpendSessionRecord,prepareProxyUsageReceipt,
  validateProxyCanonicalReceiptHead} from '../contract/proxy-reservations.js';

import {proxyReservationFixture as fixture,proxyReceiptFixture as receiptFixture} from './helpers/proxy-finance.js';
const clone=structuredClone;
const h=n=>n.toString(16).padStart(64,'0');
const sign=(wallet,bytes)=>b4a.toString(wallet.sign(bytes),'hex');

for(const family of ['llm','decisions'])for(const rail of ['fiat','tnk','tap']) {
  test(`canonical ${family}/${rail} reservation plan shares native holds without moving balances`,async()=>{
    const f=await fixture(rail,family); const before=f.storage.snapshotBytes();
    const envelope=f.authorize(f.terms); const plan=await f.prepare(envelope);
    assert.equal(f.storage.snapshotBytes(),before,'planning must not write');
    assert.equal(plan.writes.length,9); assert.equal(plan.result.available_au,'50');
    assert.equal(plan.result.reserved_au,String(BigInt(f.terms.max_spend_au)+50n));
    assert.ok(f.reads.length<50,'exact-key reads must remain bounded');
    assert.ok(plan.writes.every(w=>!w.key.startsWith('bal/')&&!w.key.startsWith('payout/')&&!w.key.startsWith('price/')));
    await f.apply(plan);
    assert.deepEqual(await f.read(f.balanceKey),f.balance);
    const accounting=await f.ledger.targetedSpendAccountingState(f.buyer.publicKey,rail);
    assert.equal(accounting.total_reserved_au,plan.result.reserved_au);
    const state=await f.ledger.targetedSpendReservationState(f.buyer.publicKey,rail,f.terms.reservation_id,f.terms.session_id);
    assert.equal(state.kind,'sharded'); assert.equal(state.session.lane,'proxy');
    assert.equal(state.session.authorization.terms.offer.provider_pubkey,f.provider.publicKey);
    assert.equal(state.session.enclave_id,undefined);
    const after=f.storage.snapshotBytes();
    const again=await f.prepare(envelope); assert.equal(again.duplicate,true); assert.deepEqual(again.writes,[]);
    assert.equal(f.storage.snapshotBytes(),after);
    assert.deepEqual(await f.read('payout/epoch/542'),{status:'prepared',native:true});
  });
}

test('native held credit prevents proxy over-reservation; other payment rails stay separate',async()=>{
  const f=await fixture('tnk');
  await f.storage.put(f.balanceKey,{...f.balance,au:String(BigInt(f.terms.max_spend_au)+49n)});
  await f.storage.put(`bal/${f.buyer.publicKey}/fiat`,{...f.balance,rail:'fiat',au:'9999999999999999'});
  const before=f.storage.snapshotBytes();
  await assert.rejects(f.prepare(f.authorize(f.terms)),/Insufficient unreserved/);
  assert.equal(f.storage.snapshotBytes(),before);
});

for(const family of ['llm','decisions'])for(const rail of ['fiat','tnk','tap']) {
  test(`verified ${family}/${rail} receipt releases excess, indexes once and preserves original economics`,async()=>{
    const f=await receiptFixture(rail,family);
    const checkpoint=await f.receipt({quantity:2,final:false});
    const partial=await f.finalize(checkpoint);
    assert.equal(partial.writes.length,1); await f.apply(partial);
    assert.equal((await f.ledger.targetedSpendAccountingState(f.buyer.publicKey,rail)).total_reserved_au,
      String(BigInt(f.terms.max_spend_au)+50n));
    assert.equal(await f.read(f.ledger.receiptEpochIndexKey(101)),null);
    assert.equal((await f.finalize(checkpoint)).duplicate,true);
    // Existing accepted work must finish despite current quote/connection/policy
    // withdrawal and a later contract. None of these mutable keys is re-read.
    await f.storage.put(`proxy/v1/provider-revoked/${f.provider.publicKey}`,{reason_hash:h(1)});
    await f.storage.put(proxyReservationKeys.settlementPolicy(f.terms.settlement_policy_hash),{enabled:false});
    await f.storage.put(f.bindingKey,{...f.payout,verified:false});
    await f.storage.put('rules/current',{ver:2,hash:h(999)});
    const e=await f.receipt({quantity:4,seq:2});
    const before=f.storage.snapshotBytes(); f.reads.length=0;
    const final=await prepareProxyUsageReceipt(f.ledger,e,{...f.context,contract_version:31},f.peer.wallet.verify);
    assert.equal(f.storage.snapshotBytes(),before); assert.ok(f.reads.length<30);
    assert.ok(f.reads.every(k=>!k.startsWith('rules/')&&!k.startsWith('payout/')&&!k.startsWith('bal/')&&!k.startsWith('price/')));
    assert.ok(final.writes.every(w=>!w.key.startsWith('bal/')&&!w.key.startsWith('payout/')&&!w.key.startsWith('price/')));
    await f.apply(final);
    const amount=e.receipt.body.au_owed_cum;
    assert.equal(final.result.au,amount); assert.equal(final.result.epoch,101);
    assert.equal((await f.read(f.summaryKey)).reserved_au,String(BigInt(amount)+50n));
    assert.deepEqual(await f.read(f.balanceKey),f.balance,'debit occurs only during shared epoch settlement');
    const state=await f.ledger.targetedSpendReservationState(f.buyer.publicKey,rail,f.terms.reservation_id,f.terms.session_id);
    assert.equal(state.session.max_spend_au,amount); assert.equal(state.session.settlement_ready,true);
    const head=await f.read(f.ledger.receiptHeadKey(f.terms.billing_id,1));
    await validateProxyCanonicalReceiptHead(f.ledger,head);
    assert.equal(head.incremental_au,amount,'settle the full attempt, not only the increment since checkpoint');
    assert.equal(head.receipt.body.usage.input_token??head.receipt.body.usage.decision,4);
    const index=await f.read(f.ledger.receiptEpochIndexKey(101));
    assert.equal(index.count,1); assert.equal(index.revision,1);
    const anchor=await f.read(f.ledger.receiptBillingKey(f.terms.billing_id));
    assert.equal(anchor.spent_au,amount); assert.equal(anchor.reserved_au,'0'); assert.equal(anchor.active_reservation_id,null);
    const close=await f.read(f.ledger.receiptReservationCloseKey(f.terms.reservation_id));
    assert.equal(close.retained_au,amount); assert.equal(close.released_au,String(BigInt(f.terms.max_spend_au)-BigInt(amount)));
    const after=f.storage.snapshotBytes(); const again=await f.finalize(e);
    assert.equal(again.duplicate,true); assert.deepEqual(again.writes,[]); assert.deepEqual(again.result,final.result);
    assert.equal(f.storage.snapshotBytes(),after);
    // Recovery still works after canonical settlement removes the held session.
    await f.storage.del(state.sessionKey);
    await f.storage.put(f.ledger.receiptConsumedKey(f.terms.billing_id,1),{type:'receipt_consumption'});
    assert.equal((await f.finalize(e)).duplicate,true);
    await assert.rejects(f.finalize(await f.receipt({quantity:5,seq:3})),/cannot advance|cannot change/);
    assert.deepEqual(await f.read('payout/epoch/542'),{status:'prepared',native:true});
  });
}

test('zero-cost final receipt releases all proxy funds and creates no settlement obligation',async()=>{
  const f=await receiptFixture(); const e=await f.receipt({quantity:0});
  const p=await f.finalize(e); await f.apply(p);
  assert.equal(p.result.au,'0'); assert.equal(p.result.epoch,null); assert.equal(p.result.final,true);
  assert.equal((await f.read(f.summaryKey)).reserved_au,'50');
  assert.equal(await f.read(f.ledger.receiptEpochIndexKey(101)),null);
  assert.equal((await f.ledger.targetedSpendReservationState(f.buyer.publicKey,'tnk',f.terms.reservation_id,f.terms.session_id)).kind,'missing');
  assert.equal((await f.read(f.ledger.receiptReservationCloseKey(f.terms.reservation_id))).retained_au,'0');
  assert.equal((await f.finalize(e)).duplicate,true);
  assert.deepEqual(await f.read(f.balanceKey),f.balance);
});

test('signed checkpoints cannot reduce usage, exceed authorization or change identity; failures do not mutate',async()=>{
  const f=await receiptFixture(); await f.apply(await f.finalize(await f.receipt({quantity:4,seq:2,final:false})));
  for(const changes of [{quantity:3,seq:3},{quantity:5,seq:1},{quantity:11,seq:3},
    {quantity:5,seq:3,at_ms:100},{quantity:5,seq:3,au_owed_cum:'1'},
    {quantity:5,seq:3,billing_au_owed_cum:'1'},{quantity:5,seq:3,outcome:'partial'},
    {quantity:5,seq:3,accepted_terms:h(999)}]) {
    const before=f.storage.snapshotBytes(); await assert.rejects(f.finalize(await f.receipt(changes)));
    assert.equal(f.storage.snapshotBytes(),before);
  }
  const forged=await f.receipt({seq:3,quantity:5}); forged.receipt.buyer_sig='0'.repeat(128);
  await assert.rejects(f.finalize(forged),/signature/);
  const wrongNetwork={...f.context,network_id:'wrong-network'};
  await assert.rejects(prepareProxyUsageReceipt(f.ledger,await f.receipt({seq:3}),wrongNetwork,f.peer.wallet.verify),/network/);
});

test('receipt preparation rejects corrupt shared accounting instead of accepting signed overcharges',async()=>{
  for(const fault of ['summary','anchor','reservation','closed','consumed','frozen','epoch']) {
    const f=await receiptFixture(), t=f.terms;
    if(fault==='summary')await f.storage.put(f.summaryKey,{...await f.read(f.summaryKey),reserved_au:'0'});
    const anchorKey=f.ledger.receiptBillingKey(t.billing_id), reservationKey=f.ledger.receiptReservationKey(t.reservation_id);
    if(fault==='anchor')await f.storage.put(anchorKey,{...await f.read(anchorKey),spent_au:'1'});
    if(fault==='reservation')await f.storage.put(reservationKey,{...await f.read(reservationKey),rail:'fiat'});
    if(fault==='closed')await f.storage.put(reservationKey,{...await f.read(reservationKey),status:'closed'});
    if(fault==='consumed')await f.storage.put(f.ledger.receiptConsumedKey(t.billing_id,1),{consumed:true});
    if(fault==='frozen')await f.storage.put('epoch/freeze/101',{frozen:true});
    if(fault==='epoch')await f.storage.put('epoch/apply/state',{epoch:99,updated_epoch:99,pending_epoch:null});
    const before=f.storage.snapshotBytes(); await assert.rejects(f.finalize(await f.receipt()),undefined,fault);
    assert.equal(f.storage.snapshotBytes(),before,fault);
  }
});

test('frozen epoch handoff uses shared receipt ingress and never resurrects an expired quote',async()=>{
  const f=await receiptFixture();
  await f.storage.put('epoch/freeze/101',{frozen:true});
  await f.storage.put('receipt/ingress',{type:'receipt_ingress',next_epoch:102,activated_epoch:101});
  const p=await f.finalize(await f.receipt()); await f.apply(p);
  assert.equal(p.result.epoch,102); assert.equal(await f.read(f.ledger.receiptEpochIndexKey(101)),null);
  assert.equal((await f.read(f.ledger.receiptEpochIndexKey(102))).count,1);
});

const allocation=head=>Object.fromEntries(['session_id','billing_id','billing_attempt','billing_epoch',
  'receipt_seq','receipt_hash','user','rail','provider','payout_revision'].map(k=>[k,head[k]]).concat([['au',head.incremental_au]]));

async function epochFixture(rail,{two=false}={}) {
  const f=await receiptFixture(rail);
  await f.apply(await f.finalize(await f.receipt()));
  const head=await f.read(f.ledger.receiptHeadKey(f.terms.billing_id,1));
  const heads=[head];
  if(two) {
    await f.storage.put(f.balanceKey,{...f.balance,au:String(BigInt(f.balance.au)*3n)});
    const first=clone(f.terms);
    Object.assign(f.terms,{billing_id:h(970),session_id:h(971),reservation_id:h(972)});
    await f.apply(await f.prepare(f.authorize(f.terms)));
    await f.apply(await f.finalize(await f.receipt({quantity:6})));
    heads.push(await f.read(f.ledger.receiptHeadKey(f.terms.billing_id,1)));
    Object.assign(f.terms,first);
  }
  const epoch=head.settlement_epoch, at=epoch*3600;
  const prior={epoch:100,updated_epoch:100,pending_epoch:null,last_apply_hash:h(980),last_settlement_unix:100*3600};
  await f.storage.put('epoch/apply/state',prior);
  await f.storage.put('epoch/apply-anchor/100',{type:'epoch_apply_anchor',epoch:100,apply_hash:prior.last_apply_hash,
    settlement_unix:prior.last_settlement_unix,applied_at:h(981)});
  await f.storage.put('payments/current',{set_by_role:'admin',tap:{chain_id:1,pool_address:'0x'+'1'.repeat(40)}});
  const frozen=await execute(f.contract,f.storage,'epochFreeze',{op:'epoch_freeze',epoch,at},f.admin.publicKey,990);
  assert.equal(frozen.ok,true,frozen.message);
  const params=await f.ledger.activeParamsAt(at,['fee_bps','epoch_seconds']);
  const gross=heads.reduce((sum,h)=>sum+BigInt(h.incremental_au),0n), fee=gross*BigInt(params.fee_bps)/10000n;
  const burn=rail==='tap'?gross*1000n/10000n:0n, net=gross-fee-burn;
  const final={rail,provider:f.provider.publicKey,gross_au:String(gross),net_au:String(net),cumulative_au:String(net)};
  const roots={dep:await f.ledger.merkleRoot('dep',[]),
    use:await f.ledger.merkleRoot('use',await Promise.all(heads.map(h=>f.ledger.usageLeafHash(h.receipt)))),
    earn:await f.ledger.merkleRoot('earn',[await f.ledger.opaqueHash('mayhem-earn-leaf-v1',final)]),
    fee:await f.ledger.opaqueHash('mayhem-fee-root-v1',{epoch,fee_au:String(fee),fee_cum_au:String(fee),
      burn_au:String(burn),burn_cum_au:String(burn),tap_burn_bps:1000}),price:await f.ledger.priceDerivationRoot([])};
  const totals={dep_count:0,dep_au:'0',use_count:heads.length,use_au:String(gross),provider_count:1,earn_au:String(net),
    fee_au:String(fee),fee_cum_au:String(fee),burn_au:String(burn),burn_cum_au:String(burn),price_count:0};
  const v={op:'commit_apply_targeted_epoch_page0',epoch,at,
    epoch_commit_hash:await f.ledger.epochCommitHash({epoch,epoch_seconds:params.epoch_seconds,roots,totals}),
    receipt_index:await f.read(f.ledger.receiptEpochIndexKey(epoch)),
    debits:[{rail,user:f.buyer.publicKey,au:String(gross)}],
    earnings:[{rail,provider:f.provider.publicKey,gross_au:String(gross),payout_revision:f.payout.revision}],
    allocations:[allocation(head)],last_page:true,roots,totals,market_usage:[],earning_finals:[final]};
  const settle=async(value=v)=>{
    const key=await f.ledger.commitTargetedEpochPageZeroFeatureKey(value);
    assert.ok(!(key instanceof Error),key.message);
    return executeFeature(f.contract,f.storage,'mayhem_feature',key,value,f.admin.publicKey);
  };
  return {...f,head,heads,v,settle,gross,fee,burn,net};
}

for(const rail of ['fiat','tnk','tap'])test(`shared ${rail} epoch settlement debits proxy work once and creates the normal payout liability`,async()=>{
  const f=await epochFixture(rail), before=BigInt((await f.read(f.balanceKey)).au);
  const result=await f.settle(); assert.equal(result.ok,true,result.message);
  assert.equal(BigInt((await f.read(f.balanceKey)).au),before-f.gross);
  const liability=await f.read(f.ledger.providerPayoutLiabilityKey(f.provider.publicKey,rail,f.payout.revision));
  assert.equal(liability.total_au,String(f.net)); assert.equal(liability.paid_cum_au,'0');
  assert.equal(liability.rail,rail); assert.equal(liability.target,f.payout.target);
  assert.equal(liability.currency,f.payout.currency); assert.equal(liability.chain_id,f.payout.chain_id);
  assert.equal((await f.read(f.summaryKey)).reserved_au,'50','unrelated native hold remains intact');
  assert.equal(await f.read(f.ledger.targetedSpendSessionKey(f.buyer.publicKey,rail,f.terms.reservation_id)),null);
  assert.equal((await f.read('epoch/apply/state')).last_receipt_proxy_use_au,String(f.gross));
  assert.equal((await f.read('epoch/apply/state')).last_receipt_market_count,0);
  assert.ok([...f.storage.values.keys()].every(k=>!k.startsWith('epoch/market-usage/101/')&&!k.startsWith('price/')));
  assert.equal((await f.read(f.ledger.receiptConsumedKey(f.terms.billing_id,1))).au,String(f.gross));
  assert.deepEqual(await f.read('payout/epoch/542'),{status:'prepared',native:true});
  const after=f.storage.snapshotBytes(); const replay=await f.settle();
  assert.equal(replay.ok,true,replay.message); assert.equal(replay.idempotent,true);
  assert.equal(f.storage.snapshotBytes(),after,'replaying a paid page cannot debit or earn twice');
});

test('proxy canonical allocations reject mismatched owners/rails/amounts without moving money',async()=>{
  for(const field of ['user','rail','provider','payout_revision','au','receipt_hash']) {
    const f=await epochFixture('tnk'), bad=clone(f.v);
    bad.allocations[0][field]=field==='rail'?'fiat':field==='au'?'1':h(999);
    const before=f.storage.snapshotBytes(), result=await f.settle(bad);
    assert.ok(result instanceof Error,field); assert.equal(f.storage.snapshotBytes(),before,field);
  }
});

for(const rail of ['fiat','tnk','tap'])test(`paged ${rail} proxy settlement recovers after restart and conserves shared fee/rail totals`,async()=>{
  const f=await epochFixture(rail,{two:true});
  const first=clone(f.v); first.last_page=false;
  first.debits[0].au=f.head.incremental_au; first.earnings[0].gross_au=f.head.incremental_au;
  delete first.earning_finals; delete first.market_usage;
  const p0=await f.settle(first); assert.equal(p0.ok,true,p0.message);
  const after0=f.storage.snapshotBytes();
  const replay0=await f.settle(first); assert.equal(replay0.ok,true,replay0.message);
  assert.equal(replay0.idempotent,true); assert.equal(f.storage.snapshotBytes(),after0);
  const state=await f.read('epoch/apply/state');
  assert.equal(state.pending_receipt_proxy_use_au,f.head.incremental_au);
  assert.equal(state.pending_receipt_market_count,0);
  // Restart the contract instance. The aggregate and paid page identity must be
  // recovered from canonical state, not a process-local accumulator.
  const resumed=new MayhemContract({peer:f.peer},{});
  const second={op:'apply_targeted_epoch',epoch:f.v.epoch,at:f.v.at,epoch_commit_hash:f.v.epoch_commit_hash,
    receipt_index:f.v.receipt_index,page:1,last_page:true,allocations:[allocation(f.heads[1])],
    debits:[{...f.v.debits[0],au:f.heads[1].incremental_au}],
    earnings:[{...f.v.earnings[0],gross_au:f.heads[1].incremental_au}],
    earning_finals:f.v.earning_finals,market_usage:[]};
  const key=await f.ledger.targetedEpochFeatureKey(second); assert.ok(!(key instanceof Error),key.message);
  const done=await executeFeature(resumed,f.storage,'mayhem_feature',key,second,f.admin.publicKey);
  assert.equal(done.ok,true,done.message);
  assert.equal((await f.read('epoch/apply/state')).last_receipt_proxy_use_au,String(f.gross));
  assert.equal((await f.read('epoch/apply/state')).pending_receipt_proxy_use_au,null);
  assert.equal((await f.read(f.summaryKey)).reserved_au,'50');
  assert.equal((await f.read(f.ledger.providerPayoutLiabilityKey(f.provider.publicKey,rail,f.payout.revision))).total_au,String(f.net));
  assert.equal(BigInt((await f.read(f.balanceKey)).au),BigInt(f.balance.au)*3n-f.gross);
  const after=f.storage.snapshotBytes();
  const replay=await executeFeature(resumed,f.storage,'mayhem_feature',key,second,f.admin.publicKey);
  assert.equal(replay.ok,true,replay.message); assert.equal(replay.idempotent,true);
  assert.equal(f.storage.snapshotBytes(),after);
});

test('native close cannot bypass the proxy attempt recovery or accepted outcome policy',async()=>{
  const f=await receiptFixture(), t=f.terms;
  const unsigned={op:'close_usage_reservation',contract_version:CONTRACT_VERSION,
    ...Object.fromEntries(['billing_id','billing_attempt','billing_epoch','session_id','reservation_id',
      'reservation_expires_after_epoch','reservation_receipt_grace_epochs','rail','payout_revision'].map(k=>[k,t[k]])),
    user:t.buyer_pubkey,provider:t.offer.provider_pubkey,latest_receipt_seq:null,latest_receipt_hash:null,
    at:2000,reason:'session_closed',actor:f.provider.publicKey,actor_role:'provider',actor_sig:''};
  const value={...unsigned,actor_sig:sign(f.provider.wallet,b4a.from(closeUsageReservationMessage(unsigned)))};
  const key=await f.ledger.closeUsageReservationFeatureKey(value);
  assert.ok(!(key instanceof Error),key.message);
  const before=f.storage.snapshotBytes();
  const result=await executeFeature(f.contract,f.storage,'mayhem_feature',key,value,f.provider.publicKey);
  assert.ok(result instanceof Error); assert.match(result.message,/Proxy reservations require/);
  assert.equal(f.storage.snapshotBytes(),before);
});

test('independent reservations cannot overspend one shared balance after sequential revalidation',async()=>{
  const f=await fixture();
  const first=await f.prepare(f.authorize(f.terms)); await f.apply(first);
  const next={...f.terms,billing_id:h(901),session_id:h(902),reservation_id:h(903)};
  const before=f.storage.snapshotBytes();
  await assert.rejects(f.prepare(f.authorize(next)),/Insufficient unreserved/);
  assert.equal(f.storage.snapshotBytes(),before);
});

test('TAP scoped backing and payout chain must agree; no silent rail conversion',async()=>{
  const f=await fixture('tap');
  await f.storage.put(f.balanceKey,{...f.balance,chain_id:2});
  const before=f.storage.snapshotBytes();
  await assert.rejects(f.prepare(f.authorize(f.terms)),/chains differ/);
  assert.equal(f.storage.snapshotBytes(),before);
});

test('settlement policy must have its real digest and cannot change the meaning of an accepted hash',async()=>{
  const f=await fixture();
  const before=await f.read(proxyReservationKeys.settlementPolicy(f.terms.settlement_policy_hash));
  const changed={...f.settlementPolicy,payable_outcomes:['cancelled','complete']};
  const bad=await f.policy({kind:'set_settlement',policy_hash:f.terms.settlement_policy_hash,enabled:true,policy:changed});
  assert.ok(bad instanceof Error); assert.match(bad.message,/hash mismatch/);
  assert.deepEqual(await f.read(proxyReservationKeys.settlementPolicy(f.terms.settlement_policy_hash)),before);
});

test('changed or revoked offers, payment readiness and signed network bindings reject before writes',async()=>{
  for(const fault of ['forged','network','epoch','contract','payout','unverified','rail','policy','rules','revoked','withdrawn']) {
    const f=await fixture('fiat'); let t=clone(f.terms);
    if(fault==='network')t.subnet_bootstrap=h(999);
    if(fault==='epoch')t.billing_epoch=100;
    if(fault==='contract')t.contract_version=29;
    if(fault==='payout')t.payout_revision=h(999);
    if(fault==='unverified')await f.storage.put(f.bindingKey,{...f.payout,verified:false});
    if(fault==='rail')await f.storage.put(`prov/${f.provider.publicKey}`,{status:'active',accepted_rails:['tap']});
    if(fault==='policy')assert.equal((await f.policy({kind:'set_settlement',policy_hash:t.settlement_policy_hash,enabled:false,policy:f.settlementPolicy})).ok,true);
    if(fault==='rules')await f.storage.put('rules/current',{ver:2,hash:h(888)});
    if(fault==='revoked')await f.storage.put(`proxy/v1/provider-revoked/${f.provider.publicKey}`,{reason_hash:h(1)});
    if(fault==='withdrawn')assert.equal((await f.submit(await f.envelope({kind:'withdraw_offer',market_id:t.offer.market_id,endpoint:t.offer.endpoint,ctx_bracket:t.offer.ctx_bracket,outcome_class:t.offer.outcome_class,revision:t.offer.revision+1}))).ok,true);
    const e=f.authorize(t); if(fault==='forged')e.authorization.buyer_sig='0'.repeat(128);
    const before=f.storage.snapshotBytes(); await assert.rejects(f.prepare(e),undefined,fault);
    assert.equal(f.storage.snapshotBytes(),before,fault);
  }
});

test('accepted retry returns original terms despite offer withdrawal, disabled policy or later contract',async()=>{
  const f=await fixture(); const e=f.authorize(f.terms); const first=await f.prepare(e); await f.apply(first);
  await f.storage.put(`proxy/v1/provider-revoked/${f.provider.publicKey}`,{reason_hash:h(1)});
  await f.storage.put(proxyReservationKeys.settlementPolicy(f.terms.settlement_policy_hash),{enabled:false,policy:f.settlementPolicy});
  const result=await prepareProxyReservation(f.ledger,e,{...f.context,contract_version:31},f.peer.wallet.verify);
  assert.equal(result.duplicate,true); assert.deepEqual(result.result,first.result); assert.deepEqual(result.writes,[]);
  const next={...f.terms,billing_attempt:2,reservation_id:h(901),session_id:h(902)};
  await assert.rejects(f.prepare(f.authorize(next)));
});

test('proxy sessions reject forged common identity, holds and policy; native records cannot impersonate proxy',async()=>{
  const f=await fixture(); const p=await f.prepare(f.authorize(f.terms));
  const session=p.writes.find(w=>w.value.type==='targeted_spend_session').value;
  for(const [field,value]of [['user',h(1)],['rail','fiat'],['provider',h(1)],['payout_revision',h(1)],['max_spend_au','1'],
    ['closed_at','fake'],['settlement_ready',true],['authorization',{}],['enclave_id',h(1)]]) {
    await assert.rejects(normalizeProxySpendSessionRecord({...session,[field]:value},f.buyer.publicKey,'tnk'),undefined,field);
  }
  assert.equal((await f.ledger.normalizeTargetedSpendSessionRecord({...session,lane:'native'},f.buyer.publicKey,'tnk')) instanceof Error,true);
  assert.ok((await proxyReservationFeatureKey(f.authorize(f.terms))).length<=256);
});

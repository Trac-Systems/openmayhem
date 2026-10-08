import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import b4a from 'b4a';
import * as f from '../contract/proxy-finance.js';
import { proxyOfferCost } from '../contract/proxy-protocol.js';
import { financialCases } from './helpers/proxy-finance-fixtures.js';
import { makeIdentity, makeVerifier } from './helpers/contract.js';

const cases=JSON.parse(fs.readFileSync(new URL('../../crates/mayhem-proto/tests/fixtures/proxy-finance-v1.json',import.meta.url))).cases;
const clone=structuredClone;
test('all twelve endpoint/rail finance fixtures reproduce exact Rust bytes and digests',async()=>{
  assert.deepEqual(await financialCases(),cases);
  for(const r of cases) {
    await f.validateProxyReceiptFor(r.receipt,r.terms,r.policy,r.receipt);
    const reordered=Object.fromEntries(Object.entries(r.terms).reverse());
    assert.equal(await f.proxySpendTermsDigest(reordered),r.digests.terms);
  }
});

for(const [field,value] of [
  ['lane','native'],['schema_version',2],['contract_version',0],['network_id',''],['buyer_pubkey','ABC'],
  ['billing_attempt',0],['billing_attempt',MAX()+1],['billing_epoch',50.1],['max_spend_au','99999999'],
  ['max_spend_au','0001'],['max_spend_au',1],['max_total_spend_au','1'],['prior_spend_au',String((1n<<128n)-1n)],
  ['acceptance_expires_after_epoch',49],['reservation_expires_after_epoch',51],['reservation_receipt_grace_epochs',MAX()],
  ['served_context',4294967296],['rail','btc'],['max_usage',{input_token:1}],['max_usage',{input_token:0,output_token:0}],
  ['max_usage',{input_token:1.1,output_token:1}],['max_usage',{input_token:-1,output_token:1}],
  ['max_usage',{input_token:1,output_token:1,hidden:1}],['private_url','must-not-be-in-wire'],
]) test(`financial terms reject ${field}=${JSON.stringify(value)}`,()=>{
  const v=clone(cases[0].terms); v[field]=value; assert.throws(()=>f.validateProxySpendTerms(v));
});
function MAX(){return Number.MAX_SAFE_INTEGER;}

test('unresolved exposure consumes total authorization without becoming delivered work',async()=>{
  const {terms:t,receipt:r,policy:p}=cases[0];
  assert.ok(BigInt(t.prior_reserved_au)>0n);
  assert.equal(BigInt(r.billing_au_owed_cum),BigInt(t.prior_spend_au)+BigInt(r.au_owed_cum));
  assert.throws(()=>f.validateProxySpendTerms({...t,max_total_spend_au:String(BigInt(t.max_total_spend_au)-BigInt(t.prior_reserved_au))}));
  assert.throws(()=>f.validateProxySpendTerms({...t,prior_reserved_au:String((1n<<128n)-1n)}));
  await assert.rejects(f.validateProxyReceiptFor(r,{...t,prior_reserved_au:'0'},p));
});

test('new acceptance checks latest offer and membership; settled work keeps its historical price',async()=>{
  const r=clone(cases[0]); const repriced=clone(r.terms.offer); repriced.revision++; repriced.rates[0].per_unit_au='99';
  await assert.rejects(f.validateProxyNewAcceptance(r.terms,r.market,r.membership,repriced,r.policy,51),/superseded/);
  await assert.rejects(f.validateProxyNewAcceptance(r.terms,r.market,r.membership,r.terms.offer,r.policy,52),/epoch/);
  for(const field of ['recipe_hash','connection_revision','served_context','endpoint_contract']) {
    const t=clone(r.terms); t[field]=typeof t[field]==='number'?9:'f'.repeat(64);
    await assert.rejects(f.validateProxyNewAcceptance(t,r.market,r.membership,r.terms.offer,r.policy,51),/membership/);
  }
  await f.validateProxyReceiptFor(r.receipt,r.terms,r.policy);
});

test('all ownership, rail, offer, payout, budget and network changes invalidate a receipt',async()=>{
  const r=cases[0];
  for(const field of ['buyer_pubkey','billing_id','session_id','reservation_id','payout_revision',
    'request_hash','endpoint_contract','recipe_hash','connection_digest','capacity_lease',
    'payment_terms_hash','settlement_policy_hash','msb_bootstrap','subnet_bootstrap']) {
    const t=clone(r.terms); t[field]='0'.repeat(64);
    await assert.rejects(f.validateProxyReceiptFor(r.receipt,t,r.policy));
  }
  for(const [field,value] of [['rail','tap'],['network_id','another-network'],['contract_version',29],
    ['billing_attempt',3],['rules_ver',2],['prior_spend_au','28'],['max_total_spend_au','99999']]) {
    const t=clone(r.terms); t[field]=value; await assert.rejects(f.validateProxyReceiptFor(r.receipt,t,r.policy));
  }
});

test('receipt quantities, money and sequence cannot retreat; final heads allow only exact replay',async()=>{
  const r=cases[0];
  for(const [field,value] of [['seq',0],['seq',1],['at_ms',0],['final',false],['outcome','unknown'],['outcome','partial'],
    ['au_owed_cum','1'],['billing_au_owed_cum','1'],['usage',{input_token:99999,output_token:7}],
    ['usage',{input_token:7}],['extra',true]]) {
    const v=clone(r.receipt); v[field]=value;
    await assert.rejects(f.validateProxyReceiptFor(v,r.terms,r.policy,r.checkpoint));
  }
  const changed={...r.receipt,seq:3,at_ms:3000};
  await assert.rejects(f.validateProxyReceiptFor(changed,r.terms,r.policy,r.receipt),/cannot advance/);
  const decreased=clone(r.receipt); decreased.usage.input_token=0;
  decreased.au_owed_cum=proxyOfferCost(r.terms.offer,decreased.usage);
  decreased.billing_au_owed_cum=String(BigInt(r.terms.prior_spend_au)+BigInt(decreased.au_owed_cum));
  await assert.rejects(f.validateProxyReceiptFor(decreased,r.terms,r.policy,r.checkpoint),/cannot advance/);
  await assert.rejects(f.validateProxyReceiptFor({...r.checkpoint,result_hash:'0'.repeat(64)},r.terms,r.policy,r.checkpoint),/cannot advance/);
});

test('refusals, partial work and cancellation cannot acquire payable status after acceptance',async()=>{
  const r=cases[0];
  for(const outcome of ['partial','cancelled','refused']) {
    const body={...r.receipt,outcome};
    await assert.rejects(f.validateProxyReceiptFor(body,r.terms,r.policy),/not payable/);
    const p={...r.policy,payable_outcomes:['complete',outcome].sort()};
    await assert.rejects(f.validateProxyReceiptFor(body,r.terms,p),/policy mismatch/);
    const t={...r.terms,settlement_policy_hash:await f.proxySettlementPolicyDigest(p)};
    body.accepted_terms=await f.proxySpendTermsDigest(t);
    await f.validateProxyReceiptFor(body,t,p);
  }
  assert.throws(()=>f.validateProxySettlementPolicy({...r.policy,payable_outcomes:['complete','running']}));
  const p={...r.policy,allow_checkpoints:false}; const t={...r.terms,settlement_policy_hash:await f.proxySettlementPolicyDigest(p)};
  const body={...r.checkpoint,accepted_terms:await f.proxySpendTermsDigest(t)};
  await assert.rejects(f.validateProxyReceiptFor(body,t,p),/not payable/);
});

test('real buyer and provider signatures bind both roles, exact bodies and retained terms on all rails',async()=>{
  const buyer=await makeIdentity(); const provider=await makeIdentity(); const other=await makeIdentity();
  const verify=makeVerifier(buyer.wallet).verify;
  const sign=(identity,bytes)=>b4a.toString(identity.wallet.sign(bytes),'hex');
  for(const row of cases) {
    const r=clone(row); r.terms.buyer_pubkey=buyer.publicKey; r.terms.offer.provider_pubkey=provider.publicKey;
    r.receipt.accepted_terms=await f.proxySpendTermsDigest(r.terms);
    const auth={terms:r.terms,buyer_sig:sign(buyer,f.proxyBuyerSpendSigningBytes(r.terms)),provider_sig:sign(provider,f.proxyProviderSpendSigningBytes(r.terms))};
    f.verifyProxySpendAuthorization(auth,verify);
    assert.throws(()=>f.verifyProxySpendAuthorization({...auth,buyer_sig:auth.provider_sig},verify));
    assert.throws(()=>f.verifyProxySpendAuthorization({...auth,provider_sig:sign(other,f.proxyProviderSpendSigningBytes(r.terms))},verify));
    assert.throws(()=>f.verifyProxySpendAuthorization({...auth,provider_sig:sign(provider,f.proxyBuyerSpendSigningBytes(r.terms))},verify));
    const envelope={body:r.receipt,buyer_sig:sign(buyer,f.proxyBuyerReceiptSigningBytes(r.receipt)),provider_sig:sign(provider,f.proxyProviderReceiptSigningBytes(r.receipt))};
    await f.verifyProxyUsageReceipt(envelope,r.terms,r.policy,null,verify);
    await assert.rejects(f.verifyProxyUsageReceipt({...envelope,buyer_sig:auth.buyer_sig},r.terms,r.policy,null,verify));
    await assert.rejects(f.verifyProxyUsageReceipt({...envelope,provider_sig:sign(provider,f.proxyBuyerReceiptSigningBytes(r.receipt))},r.terms,r.policy,null,verify));
    await assert.rejects(f.verifyProxyUsageReceipt({...envelope,body:{...r.receipt,result_hash:'0'.repeat(64)}},r.terms,r.policy,null,verify));
    // Verification requires a synchronous cryptographic true, never a truthy promise/string.
    assert.throws(()=>f.verifyProxySpendAuthorization(auth,()=>Promise.resolve(true)));
  }
});

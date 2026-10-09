import assert from 'node:assert/strict';
import fs from 'node:fs';
import b4a from 'b4a';
import MayhemContract from '../../contract/contract.js';
import {proxyContractFixture} from './proxy.js';
import {makeIdentity} from './contract.js';
import {proxyOfferCost} from '../../contract/proxy-protocol.js';
import {proxyBuyerSpendSigningBytes,proxyProviderSpendSigningBytes,proxySettlementPolicyDigest,
  proxySpendTermsDigest,proxyBuyerReceiptSigningBytes,proxyProviderReceiptSigningBytes} from '../../contract/proxy-finance.js';
import {prepareProxyReservation,proxyPaymentTermsDigest,prepareProxyUsageReceipt} from '../../contract/proxy-reservations.js';
const cases=JSON.parse(fs.readFileSync(new URL('../../../crates/mayhem-proto/tests/fixtures/proxy-finance-v1.json',import.meta.url))).cases;
const clone=structuredClone;
const h=n=>n.toString(16).padStart(64,'0');
const sign=(wallet,bytes)=>b4a.toString(wallet.sign(bytes),'hex');

export async function proxyReservationFixture(rail='tnk',family='llm',execution=null,expiryPolicy=false) {
  const f=await proxyContractFixture(family,rail,execution);
  assert.equal((await f.submit(await f.create())).ok,true);
  assert.equal((await f.submit(await f.envelope({kind:'set_offer',offer:f.offer}))).ok,true);
  assert.equal((await f.policy({kind:'configure_finance',policy:{enabled:true,
    max_reservations_per_epoch:1000,max_reservations_per_provider_epoch:100,max_checkpoints_per_reservation:8}})).ok,true);
  const buyer=await makeIdentity();
  const row=clone(cases.find(r=>r.terms.rail===rail&&r.terms.offer.endpoint===(family==='llm'?'openai_chat_completions':'mayhem_decisions')));
  const rules={ver:1,hash:h(800)};
  await f.storage.put('rules/current',rules);
  await f.storage.put('epoch/apply/state',{epoch:100,updated_epoch:100,pending_epoch:null});
  await f.storage.put(`prov/${f.provider.publicKey}`,{status:'active',accepted_rails:['fiat','tnk','tap']});
  const payout={type:'provider_payout_binding',provider:f.provider.publicKey,rail,revision:h(801),verified:true,
    activation_epoch:1,target:rail==='fiat'?'acct_proxy_fixture':rail==='tap'?'0x'+'2'.repeat(40):'fixture-target',
    currency:rail==='fiat'?'eur':null,stripe_processor_revision:h(802),chain_id:rail==='tap'?1:null};
  const bindingKey=`payout/binding/${rail}/${f.provider.publicKey}/${payout.revision}`;
  await f.storage.put(bindingKey,payout);
  await f.storage.put(`payout/current/${rail}/${f.provider.publicKey}`,{provider:f.provider.publicKey,rail,
    current_revision:payout.revision,pending_revision:null,pending_activation_epoch:null});
  if(rail==='fiat') {
    const verification={type:'stripe_payout_verification',provider:f.provider.publicKey,target:payout.target,
      revision:h(803),processor_revision:payout.stripe_processor_revision,ready:true};
    await f.storage.put('payout/stripe-verified/fixture',verification);
    await f.storage.put(f.contract.providerStripePayoutVerificationTargetKey(f.provider.publicKey,payout.target),
      {...verification,record_key:'payout/stripe-verified/fixture'});
  }
  const policy=execution?.settlement_policy??row.policy;
  if(expiryPolicy)policy.hold_expiry='release_unfinalized_and_block_retry';
  const policyHash=await proxySettlementPolicyDigest(policy);
  assert.equal((await f.policy({kind:'set_settlement',policy_hash:policyHash,enabled:true,policy})).ok,true);
  const terms={...row.terms,...f.network,buyer_pubkey:buyer.publicKey,billing_attempt:1,
    billing_epoch:101,acceptance_expires_after_epoch:101,reservation_expires_after_epoch:120,
    payout_revision:payout.revision,offer:f.offer,served_context:f.membership.served_context,
    recipe_hash:f.membership.recipe_hash,connection_revision:f.membership.connection_revision,
    endpoint_contract:f.membership.endpoints.find(e=>e.endpoint===f.offer.endpoint).contract_hash,
    settlement_policy_hash:policyHash,payment_terms_hash:await proxyPaymentTermsDigest(rules,payout),
    max_usage:Object.fromEntries(f.offer.rates.map(r=>[r.unit,10])),prior_spend_au:'0',prior_reserved_au:'0'};
  if(execution) {
    terms.request_hash=execution.request_hash;
    terms.connection_digest=execution.connection_digest;
    terms.max_usage=Object.fromEntries(f.offer.rates.map(r=>[r.unit,100000]));
  }
  terms.max_spend_au=proxyOfferCost(terms.offer,terms.max_usage);
  terms.max_total_spend_au=String(BigInt(terms.max_spend_au)+1000n);
  const balanceKey=`bal/${buyer.publicKey}/${rail}`;
  const balance={user:buyer.publicKey,rail,denom:'au_usd',au:String(BigInt(terms.max_spend_au)+100n),updated_epoch:100,updated_at:null};
  if(rail==='tap')Object.assign(balance,{chain_id:1,pool_address:'0x'+'1'.repeat(40)});
  await f.storage.put(balanceKey,balance);
  // This aggregate represents funds already reserved by the existing native path.
  // The test verifies shared accounting, not another live/native inference run.
  const summaryKey=f.contract.targetedSpendSummaryKey(buyer.publicKey,rail);
  await f.storage.put(summaryKey,{type:'targeted_spend_summary',user:buyer.publicKey,rail,denom:'au_usd',
    reserved_au:'50',balance_au_at_last_reserve:balance.au,updated_at:'test/native-reserve'});
  const ledger=new MayhemContract({peer:f.peer},{});
  const reads=[];
  ledger.get=async key=>{reads.push(key);return f.read(key);};
  ledger.put=ledger.del=()=>{throw new Error('reservation planner attempted a write');};
  const authorize=t=>({op:'proxy_spend_reserve',at:1000,authorization:{terms:t,
    buyer_sig:sign(buyer.wallet,proxyBuyerSpendSigningBytes(t)),provider_sig:sign(f.provider.wallet,proxyProviderSpendSigningBytes(t))}});
  const prepare=e=>prepareProxyReservation(ledger,e,f.context,f.peer.wallet.verify);
  const apply=async p=>{for(const w of p.writes)if(w.delete)await f.storage.del(w.key);else await f.storage.put(w.key,w.value);};
  return {...f,buyer,ledger,terms,settlementPolicy:policy,authorize,prepare,apply,reads,balance,balanceKey,summaryKey,payout,bindingKey};
}
export async function proxyReceiptFixture(rail='tnk',family='llm',execution=null,deferred=false,expiryPolicy=false) {
  const f=await proxyReservationFixture(rail,family,execution,expiryPolicy);
  if(!deferred)await f.apply(await f.prepare(f.authorize(f.terms)));
  const receipt=async({quantity=4,seq=1,final=true,...changes}={})=>{
    const usage=Object.fromEntries(f.terms.offer.rates.map(r=>[r.unit,quantity]));
    const au=proxyOfferCost(f.terms.offer,usage);
    const body={schema_version:1,lane:'proxy',accepted_terms:await proxySpendTermsDigest(f.terms),
      seq,final,outcome:final?'complete':'running',result_hash:h(950),observation_hash:h(951),
      usage,au_owed_cum:au,billing_au_owed_cum:au,at_ms:2000+seq,...changes};
    return {op:'proxy_record_usage',provider:f.provider.publicKey,receipt:{body,buyer_sig:sign(f.buyer.wallet,proxyBuyerReceiptSigningBytes(body)),
      provider_sig:sign(f.provider.wallet,proxyProviderReceiptSigningBytes(body))}};
  };
  const finalize=e=>prepareProxyUsageReceipt(f.ledger,e,f.context,f.peer.wallet.verify);
  return {...f,receipt,finalize};
}

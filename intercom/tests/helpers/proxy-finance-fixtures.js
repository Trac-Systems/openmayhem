// Synthetic deterministic wire fixtures only; never keys, quotes or live authority.
import fs from 'node:fs';
import { proxyMarketId, proxyOfferCost } from '../../contract/proxy-protocol.js';
import * as f from '../../contract/proxy-finance.js';

export async function financialCases() {
  const base = JSON.parse(fs.readFileSync(new URL('../../../crates/mayhem-proto/tests/fixtures/proxy-wire-v1.json',import.meta.url)));
  const cases = [];
  for (const endpoint of ['openai_chat_completions','openai_completions','openai_responses','mayhem_decisions']) {
    for (const rail of ['fiat','tnk','tap']) {
      const source = structuredClone(base.cases.find(r => r.name === (endpoint === 'mayhem_decisions' ? 'decisions' : 'llm')));
      const { market, membership, offer } = source;
      if (endpoint === 'mayhem_decisions') market.metering.units = ['decision'];
      offer.market_id = membership.market_id = await proxyMarketId(market);
      offer.endpoint = endpoint;
      offer.rates = market.metering.units.map((unit,i) => ({unit,per_unit_au:String(7+i*6),granularity:3}));
      offer.per_request_au = '5'; offer.min_session_au = '11';
      const policy = {schema_version:1,lane:'proxy',payable_outcomes:['complete'],allow_checkpoints:true};
      const terms = {
        schema_version:1,lane:'proxy',network_id:'test-network',msb_bootstrap:'1'.repeat(64),subnet_bootstrap:'2'.repeat(64),
        contract_version:30,buyer_pubkey:'3'.repeat(64),billing_id:'4'.repeat(64),billing_attempt:2,
        session_id:'5'.repeat(64),reservation_id:'6'.repeat(64),billing_epoch:51,
        acceptance_expires_after_epoch:51,reservation_expires_after_epoch:70,reservation_receipt_grace_epochs:4,
        payout_revision:'7'.repeat(64),request_hash:'8'.repeat(64),endpoint_contract:membership.endpoints.find(e=>e.endpoint===endpoint).contract_hash,
        recipe_hash:membership.recipe_hash,connection_digest:'9'.repeat(64),connection_revision:membership.connection_revision,
        capacity_lease:'a'.repeat(64),offer,rail,served_context:membership.served_context,
        settlement_policy_hash:await f.proxySettlementPolicyDigest(policy),payment_terms_hash:'c'.repeat(64),rules_ver:1,
        max_usage:Object.fromEntries(offer.rates.map(r=>[r.unit,1024])),max_spend_au:'0',prior_spend_au:'29',prior_reserved_au:'17',max_total_spend_au:'0',
      };
      terms.max_spend_au = proxyOfferCost(offer,terms.max_usage);
      terms.max_total_spend_au = String(BigInt(terms.max_spend_au)+BigInt(terms.prior_spend_au)+BigInt(terms.prior_reserved_au));
      const checkpoint = {
        schema_version:1,lane:'proxy',accepted_terms:await f.proxySpendTermsDigest(terms),seq:1,final:false,outcome:'running',
        result_hash:'d'.repeat(64),observation_hash:'e'.repeat(64),usage:Object.fromEntries(offer.rates.map(r=>[r.unit,1])),
        au_owed_cum:'0',billing_au_owed_cum:'0',at_ms:1000,
      };
      checkpoint.au_owed_cum=proxyOfferCost(offer,checkpoint.usage);
      checkpoint.billing_au_owed_cum=String(BigInt(checkpoint.au_owed_cum)+BigInt(terms.prior_spend_au));
      const receipt = {...checkpoint,seq:2,final:true,outcome:'complete',at_ms:2000,
        usage:Object.fromEntries(offer.rates.map(r=>[r.unit,7]))};
      receipt.au_owed_cum=proxyOfferCost(offer,receipt.usage);
      receipt.billing_au_owed_cum=String(BigInt(receipt.au_owed_cum)+BigInt(terms.prior_spend_au));
      await f.validateProxyNewAcceptance(terms,market,membership,offer,policy,51);
      await f.validateProxyReceiptFor(checkpoint,terms,policy);
      await f.validateProxyReceiptFor(receipt,terms,policy,checkpoint);
      cases.push({name:`${endpoint}/${rail}`,market,membership,policy,terms,checkpoint,receipt,
        digests:{policy:await f.proxySettlementPolicyDigest(policy),terms:await f.proxySpendTermsDigest(terms),receipt:await f.proxyReceiptDigest(receipt)},
        signing_utf8:{buyer_terms:f.proxyBuyerSpendSigningBytes(terms).toString('utf8'),provider_terms:f.proxyProviderSpendSigningBytes(terms).toString('utf8'),
          buyer_receipt:f.proxyBuyerReceiptSigningBytes(receipt).toString('utf8'),provider_receipt:f.proxyProviderReceiptSigningBytes(receipt).toString('utf8')},
      });
    }
  }
  return cases;
}

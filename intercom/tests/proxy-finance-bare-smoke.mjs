// Run from the checkout root with the release-supported Bare runtime.
import fs from 'fs';
import b4a from 'b4a';
import PeerWallet from 'trac-wallet';
import * as f from '../contract/proxy-finance.js';
const check=(v,m)=>{if(!v)throw new Error(m);};
const cases=JSON.parse(fs.readFileSync('crates/mayhem-proto/tests/fixtures/proxy-finance-v1.json','utf8')).cases;
for(const r of cases) {
  check(await f.proxySpendTermsDigest(r.terms)===r.digests.terms,'terms digest changed');
  check(await f.proxyReceiptDigest(r.receipt)===r.digests.receipt,'receipt digest changed');
  await f.validateProxyNewAcceptance(r.terms,r.market,r.membership,r.terms.offer,r.policy,51);
  await f.validateProxyReceiptFor(r.receipt,r.terms,r.policy,r.checkpoint);
  await f.validateProxyReceiptFor(r.receipt,r.terms,r.policy,r.receipt);
}
const buyer=new PeerWallet(); const provider=new PeerWallet();
await buyer.ready; await provider.ready; await buyer.generateKeyPair(); await provider.generateKeyPair();
const r=JSON.parse(JSON.stringify(cases[0]));
r.terms.buyer_pubkey=b4a.toString(buyer.publicKey,'hex');
r.terms.offer.provider_pubkey=b4a.toString(provider.publicKey,'hex');
r.receipt.accepted_terms=await f.proxySpendTermsDigest(r.terms);
const verify=(s,m,k)=>buyer.verify(b4a.from(s,'hex'),m,b4a.from(k,'hex'));
const sign=(w,m)=>b4a.toString(w.sign(m),'hex');
const auth={terms:r.terms,buyer_sig:sign(buyer,f.proxyBuyerSpendSigningBytes(r.terms)),provider_sig:sign(provider,f.proxyProviderSpendSigningBytes(r.terms))};
f.verifyProxySpendAuthorization(auth,verify);
const receipt={body:r.receipt,buyer_sig:sign(buyer,f.proxyBuyerReceiptSigningBytes(r.receipt)),provider_sig:sign(provider,f.proxyProviderReceiptSigningBytes(r.receipt))};
await f.verifyProxyUsageReceipt(receipt,r.terms,r.policy,null,verify);
let rejected=false;
try { await f.verifyProxyUsageReceipt({...receipt,buyer_sig:auth.buyer_sig},r.terms,r.policy,null,verify); } catch { rejected=true; }
check(rejected,'cross-domain signature accepted');
console.log('Bare proxy finance: 12 endpoint/rail wire vectors, receipt progression/replay and real role-separated signatures passed.');

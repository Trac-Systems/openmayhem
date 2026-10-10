import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {generateKeyPairSync,randomBytes,randomUUID} from 'node:crypto';
import {digest,invoiceCommitment} from '../../scripts/proxy-admission-wire.mjs';
import {signed} from '../../scripts/proxy-admission-refund-common.mjs';
const fixtures=JSON.parse(fs.readFileSync(new URL('../fixtures/proxy-admission-worker-v1.json',import.meta.url)));
const h=()=>randomBytes(32).toString('hex');
const key=()=>{const k=generateKeyPairSync('ed25519');return {...k,hex:k.publicKey.export({format:'der',type:'spki'}).subarray(-32).toString('hex')};};
export async function tapRefundWork(t,{receiver,token='0x'+'1'.repeat(40),destination='0x'+'2'.repeat(40),chainId=31337,root=null,policy=null,executor=null,review=null}={}) {
 const original=structuredClone(fixtures.cases.find(c=>c.rail==='tap')),issuer=key(),id=randomUUID();
 review??=key();executor??=key();
 const invoice={...original.verify_work.invoice,provider_pubkey:h(),entitlement_id:h(),issuer_pubkey:issuer.hex,
  collection:{...original.verify_work.invoice.collection,chain_id:chainId,token_contract:token,destination:receiver}};
 invoice.invoice_commitment=await invoiceCommitment(id,invoice);
 const reference={chain_id:chainId,token_contract:token,transaction_hash:'0x'+h(),log_index:0};
 const receipt={...original.evidence_completion.receipt,...reference,amount_base_units:String(BigInt(invoice.amount_base_units)+10n),to_address:receiver};
 receipt.physical_key=`tap/${chainId}/${token}/${reference.transaction_hash}/0`;
 const evidence={receipt,canonical_epoch:100,evidence_commitment:await digest('mayhem/proxy/admission-evidence/v1',{invoice_commitment:invoice.invoice_commitment,payment_reference:reference,receipt})};
 policy??={enabled:true,policy_hash:h(),authorizers:[review.hex],executors:[executor.hex],reasons:['excess'],rails:['tap'],max_authorization_ms:60000,lease_ms:60000};
 const now=Date.now(),body={schema_version:1,purpose:'proxy_admission_refund',refund_id:randomUUID(),invoice_id:id,payment_id:randomUUID(),
  invoice_commitment:invoice.invoice_commitment,evidence_commitment:evidence.evidence_commitment,physical_key:receipt.physical_key,policy_hash:policy.policy_hash,
  authorizer_pubkey:review.hex,reason:'excess',amount_base_units:'10',destination:{rail:'tap',chain_id:chainId,token_contract:token,address:destination},
  authorization_method:'operator_review',return_evidence_hash:h(),approved_at_ms:now-1,expires_at_ms:now+59000};
 const authorization=signed('mayhem/proxy/admission-refund-authorization/v1',body,review.privateKey);
 const work={refund_id:body.refund_id,action:'prepare',lease_token:h(),lease_expires_at_ms:now+60000,authorization,
  authorization_digest:await digest('mayhem/proxy/admission-refund-authorization/v1',body),invoice,payment_reference:reference,payment_evidence:evidence,preparation:null,first_dispatch_at_ms:null};
 const grant={schema_version:1,purpose:'proxy_admission_refund',refund_id:body.refund_id,action:'dispatch',first_dispatch_at_ms:now};
 if(!root){root=fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(),'refund-tap-journal-')));fs.chmodSync(root,0o700);t.after(()=>fs.rmSync(root,{recursive:true,force:true}));}
 return {work,grant,root,policy,executor,review,resumed:p=>({...work,action:'reconcile',preparation:p,first_dispatch_at_ms:grant.first_dispatch_at_ms})};
}

// Shared admission return authorization and bounded immutable custody journals.
import fs from 'node:fs';
import path from 'node:path';
import { createPublicKey, sign, verify } from 'node:crypto';
import { isAddressValid } from 'trac-msb/src/core/state/utils/address.js';
import { proxyCanonicalSigningBytes } from '../contract/proxy-protocol.js';
import { amount, digest, hex, need, shape, uint, validateInvoice, invoiceCommitment, validateEvidence } from './proxy-admission-wire.mjs';
import { readCustodyFile } from './proxy-admission-custody.mjs';
import { ReviewWork } from './retail-crypto-verification.mjs';
const AUTH = 'mayhem/proxy/admission-refund-authorization/v1';
export const PREPARE = 'mayhem/proxy/admission-refund-preparation/v1';
export const DELIVERY = 'mayhem/proxy/admission-refund-delivery/v1';
export const RECOVERY = 'mayhem/proxy/admission-refund-recovery/v1';
const opaque = v => typeof v === 'string' && /^[A-Za-z0-9_-]{1,128}$/.test(v);
export const signed = (domain, body, key) => ({ body, signature: sign(null, proxyCanonicalSigningBytes(domain, body), key).toString('hex') });
function verified(domain, envelope, key) {
  shape(envelope, ['body', 'signature']); need(hex(key) && /^[0-9a-f]{128}$/.test(envelope.signature), 'invalid refund signature');
  const publicKey = createPublicKey({ key: Buffer.concat([Buffer.from('302a300506032b6570032100','hex'),Buffer.from(key,'hex')]), format:'der', type:'spki' });
  need(verify(null, proxyCanonicalSigningBytes(domain, envelope.body), publicKey, Buffer.from(envelope.signature,'hex')), 'refund signature differs');
}
export { verified as verifyRefundSignature };
export function same(a,b) { return proxyCanonicalSigningBytes('refund-record',a).equals(proxyCanonicalSigningBytes('refund-record',b)); }
function fsyncDirectory(dir) { const fd=fs.openSync(dir,fs.constants.O_RDONLY); try { fs.fsyncSync(fd); } finally { fs.closeSync(fd); } }

/** Fixed names and bounded immutable files; no history scan or whole-wallet
 * journal. A corrupt/insecure record stops execution rather than recreating it. */
export class RefundJournal {
  constructor(root, commitment) {
    need(process.platform!=='win32' && typeof process.getuid==='function' && typeof root==='string' && path.isAbsolute(root)
      && fs.realpathSync(root)===path.resolve(root) && hex(commitment),'canonical protected refund journal required');
    this.dir=path.join(root,commitment);
    for(const dir of [root,this.dir]) {
      if(dir!==root) try { fs.mkdirSync(dir,{mode:0o700}); } catch(e) { if(e.code!=='EEXIST') throw e; }
      const stat=fs.lstatSync(dir);
      need(stat.isDirectory()&&!stat.isSymbolicLink()&&stat.uid===process.getuid()&&(stat.mode&0o077)===0,'owner-only refund directory required');
    }
    fsyncDirectory(root);
  }
  file(kind) { need(['request','refund'].includes(kind),'unsupported journal record'); return path.join(this.dir,`${kind}.json`); }
  get(kind) {
    const file=this.file(kind); try { fs.lstatSync(file); } catch(e) { if(e.code==='ENOENT') return null; throw e; }
    const raw=readCustodyFile(file,{max:16384}); try { return JSON.parse(raw.toString('utf8')); } finally { raw.fill(0); }
  }
  retain(kind,value) {
    const file=this.file(kind), data=Buffer.from(JSON.stringify(value)); need(data.length>0&&data.length<=16384,'refund journal exceeds bound');
    let fd;
    try { fd=fs.openSync(file,fs.constants.O_WRONLY|fs.constants.O_CREAT|fs.constants.O_EXCL|fs.constants.O_NOFOLLOW,0o600); }
    catch(e) { if(e.code!=='EEXIST') throw e; need(same(this.get(kind),value),'immutable refund journal differs'); return value; }
    try { fs.writeFileSync(fd,data); fs.fsyncSync(fd); } finally { data.fill(0); fs.closeSync(fd); }
    fsyncDirectory(this.dir); return value;
  }
}

export async function validateRefundWork(work,policy,executorPubkey,rail,now=Date.now()) {
  need(['fiat','tnk','tap'].includes(rail),'invalid refund rail');
  need(Buffer.byteLength(JSON.stringify(work))<=16384,'refund work exceeds bound');
  shape(work,['refund_id','action','lease_token','lease_expires_at_ms','authorization','authorization_digest','invoice','payment_reference','payment_evidence','preparation','first_dispatch_at_ms',...(Object.hasOwn(work,'recovery')?['recovery']:[])]);
  need(opaque(work.refund_id)&&['prepare','reconcile'].includes(work.action)&&hex(work.lease_token)&&uint(work.lease_expires_at_ms,1),'invalid refund work');
  const a=work.authorization,b=a.body;
  shape(b,['schema_version','purpose','refund_id','invoice_id','payment_id','invoice_commitment','evidence_commitment','physical_key','policy_hash','authorizer_pubkey','reason',
    'amount_base_units','destination','authorization_method','return_evidence_hash','approved_at_ms','expires_at_ms']);
  need(b.schema_version===1&&b.purpose==='proxy_admission_refund'&&b.authorization_method==='operator_review'
    &&b.refund_id===work.refund_id&&opaque(b.invoice_id)&&opaque(b.payment_id)&&hex(b.invoice_commitment)&&hex(b.evidence_commitment)
    &&hex(b.policy_hash)&&hex(b.return_evidence_hash)&&amount(b.amount_base_units)&&BigInt(b.amount_base_units)>0n
    &&uint(b.approved_at_ms,1)&&uint(b.expires_at_ms,b.approved_at_ms+1),'invalid refund authorization');
  verified(AUTH,a,b.authorizer_pubkey);
  need(policy.enabled===true&&policy.policy_hash===b.policy_hash&&policy.authorizers.includes(b.authorizer_pubkey)&&policy.executors.includes(executorPubkey)
    &&!policy.authorizers.includes(executorPubkey)&&policy.reasons.includes(b.reason)&&policy.rails.includes(rail)
    &&uint(policy.max_authorization_ms,1)&&b.expires_at_ms-b.approved_at_ms<=policy.max_authorization_ms,'refund policy differs');
  need(await digest(AUTH,b)===work.authorization_digest,'refund authorization commitment differs');
  validateInvoice(work.invoice); const i=work.invoice;
  need(i.rail===rail&&await invoiceCommitment(b.invoice_id,i)===b.invoice_commitment&&i.invoice_commitment===b.invoice_commitment
    &&![i.issuer_pubkey,i.provider_pubkey].includes(b.authorizer_pubkey)&&![i.issuer_pubkey,i.provider_pubkey].includes(executorPubkey),'refund invoice/authority differs');
  await validateEvidence(work.payment_evidence,{invoice:i,payment_reference:work.payment_reference});
  const r=work.payment_evidence.receipt,d=b.destination;
  need(r.rail===rail&&work.payment_evidence.evidence_commitment===b.evidence_commitment&&r.physical_key===b.physical_key
    &&BigInt(r.amount_base_units)>=BigInt(b.amount_base_units)&&d.rail===rail,'original refund payment differs');
  if(rail==='fiat') {
    shape(d,['rail','stripe_account','livemode','payment_intent_id','currency']);
    need(d.stripe_account===r.stripe_account&&d.livemode===r.livemode&&d.payment_intent_id===r.payment_intent_id&&d.currency===r.currency,'original refund method differs');
  } else if(rail==='tnk') {
    shape(d,['rail','network','address']);
    const prefix=d.network==='mainnet'?'trac':'testtrac';
    need(['mainnet','testnet1'].includes(d.network)&&d.network===r.network&&isAddressValid(d.address,prefix)
      &&d.address!==r.to_address,'TNK refund destination differs');
  } else {
    shape(d,['rail','chain_id','token_contract','address']);
    need(d.chain_id===r.chain_id&&d.token_contract===r.token_contract&&typeof d.address==='string'
      &&/^0x[0-9a-f]{40}$/.test(d.address)&&d.address!==`0x${'0'.repeat(40)}`&&d.address!==r.to_address,'TAP refund destination differs');
  }
  need((work.action==='prepare'&&work.preparation===null&&work.first_dispatch_at_ms===null)
    ||(work.action==='reconcile'&&work.preparation!==null&&uint(work.first_dispatch_at_ms,1)),'refund dispatch state differs');
  let window=b;
  if(Object.hasOwn(work,'recovery')) {
    const recovery=work.recovery,r=recovery?.body;
    shape(r,['schema_version','purpose','refund_id','authorization_digest','policy_hash','authorizer_pubkey','revision','previous_recovery_digest',
      'preparation_digest','first_dispatch_at_ms','review_code','review_evidence_hash','approved_at_ms','expires_at_ms']);
    need(r.schema_version===1&&r.purpose==='proxy_admission_refund_recovery'&&r.refund_id===work.refund_id
      &&r.authorization_digest===work.authorization_digest&&r.policy_hash===policy.policy_hash&&hex(r.authorizer_pubkey)
      &&policy.authorizers.includes(r.authorizer_pubkey)&&![executorPubkey,i.issuer_pubkey,i.provider_pubkey].includes(r.authorizer_pubkey)
      &&uint(r.revision,1)&&r.revision<=2147483647&&(r.revision===1?r.previous_recovery_digest===null:hex(r.previous_recovery_digest))
      &&hex(r.review_evidence_hash)&&(r.review_code===null||typeof r.review_code==='string'&&/^[a-z0-9_]{1,100}$/.test(r.review_code))
      &&uint(r.approved_at_ms,1)&&uint(r.expires_at_ms,r.approved_at_ms+1)&&r.approved_at_ms<=now
      &&r.expires_at_ms-r.approved_at_ms<=policy.max_authorization_ms,'invalid refund recovery');
    verified(RECOVERY,recovery,r.authorizer_pubkey);
    if(r.first_dispatch_at_ms===null) need(r.preparation_digest===null,'refund recovery preparation differs');
    else need(uint(r.first_dispatch_at_ms,b.approved_at_ms)&&r.first_dispatch_at_ms<=r.approved_at_ms
      &&r.first_dispatch_at_ms===work.first_dispatch_at_ms&&work.preparation!==null&&hex(r.preparation_digest)
      &&await digest('refund-preparation',work.preparation)===r.preparation_digest,'refund recovery dispatch differs');
    window=r;
  }
  if(work.action==='prepare'&&(now<window.approved_at_ms||now>=window.expires_at_ms)) throw new ReviewWork('refund_authorization_expired');
  if(work.action==='reconcile') validateRefundDispatch(work,{schema_version:1,purpose:'proxy_admission_refund',refund_id:work.refund_id,
    action:'reconcile',first_dispatch_at_ms:work.first_dispatch_at_ms});
  return work;
}


// Called after validateRefundWork. A recovery of an already dispatched operation
// binds its exact preparation/time; it never grants a fresh financial identity.
export function validateRefundDispatch(work,grant) {
  shape(grant,['schema_version','purpose','refund_id','action','first_dispatch_at_ms']);
  const r=work.recovery?.body,b=work.authorization.body,window=r??b;
  const timestamp=grant.first_dispatch_at_ms;
  need(grant.schema_version===1&&grant.purpose==='proxy_admission_refund'&&grant.refund_id===work.refund_id
    &&['dispatch','reconcile'].includes(grant.action)&&uint(timestamp,1)
    &&(work.first_dispatch_at_ms===null||work.first_dispatch_at_ms===timestamp)
    &&(r?.first_dispatch_at_ms!=null ? timestamp===r.first_dispatch_at_ms
      : timestamp>=window.approved_at_ms&&timestamp<window.expires_at_ms),'invalid refund dispatch grant');
}

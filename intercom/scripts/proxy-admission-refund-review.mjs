#!/usr/bin/env node
// Explicit operator actions only. No custody, transfer client or automatic signing loop.
import fs from 'node:fs';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {createPrivateKey,createPublicKey,createHash} from 'node:crypto';
import {readCustodyFile} from './proxy-admission-custody.mjs';
import {fixedOrigin,boundedJson} from './proxy-admission-worker.mjs';
import {digest,hex,need,shape,uint} from './proxy-admission-wire.mjs';
import {RECOVERY,signed,verifyRefundSignature} from './proxy-admission-refund-common.mjs';
const envelope={schema_version:1,purpose:'proxy_admission_refund'};
const opaque=v=>typeof v==='string'&&/^[A-Za-z0-9_-]{1,128}$/.test(v);

export class RefundReviewApi {
 constructor({origin,credential,allowLoopbackHttp=false,fetcher=fetch}) {
  this.origin=fixedOrigin(origin,{allowLoopbackHttp});
  need(typeof credential==='string'&&/^[\x21-\x7e]{32,256}$/.test(credential),'review API credential required');
  this.credential=credential;this.fetcher=fetcher;
 }
 async post(action,body,signal=AbortSignal.timeout(15000)) {
  need(['review','recover'].includes(action),'review client cannot execute payments');
  const result=await boundedJson(`${this.origin}/internal/proxy-admission-refunds/${action}`,{body:{...body,...envelope},signal,
   headers:{authorization:`Bearer ${this.credential}`},fetcher:this.fetcher,maxBytes:16384});
  need(result?.schema_version===1&&result.purpose===envelope.purpose,'review API purpose differs');return result;
 }
}

export async function prepareRecovery(view,{key,policy,evidenceHash,approvalMs,now=Date.now()}) {
 shape(view,['refund_id','state','authorization','authorization_digest','preparation','preparation_digest','first_dispatch_at_ms',
  'review_code','previous_recovery_digest','next_revision','recovery']);
 need(Buffer.byteLength(JSON.stringify(view))<=16384&&opaque(view.refund_id)&&view.state==='review'
  &&key.asymmetricKeyType==='ed25519'&&hex(evidenceHash)&&uint(approvalMs,1)&&uint(now,1)
  &&Number.isSafeInteger(now+approvalMs),'invalid reviewed return');
 const authorizer=createPublicKey(key).export({format:'der',type:'spki'}).subarray(-32).toString('hex'),b=view.authorization?.body;
 need(b?.refund_id===view.refund_id&&hex(view.authorization_digest)&&hex(b.authorizer_pubkey),'review identity differs');
 verifyRefundSignature('mayhem/proxy/admission-refund-authorization/v1',view.authorization,b.authorizer_pubkey);
 need(await digest('mayhem/proxy/admission-refund-authorization/v1',b)===view.authorization_digest,'original authorization changed');
 need(policy.enabled===true&&policy.policy_hash===b.policy_hash&&policy.authorizers.includes(authorizer)
  &&policy.authorizers.includes(b.authorizer_pubkey)&&!policy.executors.includes(authorizer)
  &&policy.rails.includes(b.destination.rail)&&policy.reasons.includes(b.reason)&&uint(policy.max_authorization_ms,1)
  &&approvalMs<=policy.max_authorization_ms,'review policy differs');
 need(uint(view.next_revision,1)&&view.next_revision<=2147483647
  &&(view.next_revision===1?view.previous_recovery_digest===null:hex(view.previous_recovery_digest))
  &&(view.review_code===null||typeof view.review_code==='string'&&/^[a-z0-9_]{1,100}$/.test(view.review_code)),'review revision differs');
 if(view.first_dispatch_at_ms===null)need(view.preparation===null&&view.preparation_digest===null,'review dispatch differs');
 else need(uint(view.first_dispatch_at_ms,b.approved_at_ms)&&view.first_dispatch_at_ms<=now&&view.preparation!==null
  &&await digest('refund-preparation',view.preparation)===view.preparation_digest,'review preparation changed');
 if(view.next_revision===1)need(view.recovery===null,'review predecessor differs');
 else {
  const previous=view.recovery;
  verifyRefundSignature(RECOVERY,previous,previous?.body?.authorizer_pubkey);
  need(previous.body.refund_id===view.refund_id&&previous.body.authorization_digest===view.authorization_digest
   &&previous.body.revision===view.next_revision-1&&await digest(RECOVERY,previous.body)===view.previous_recovery_digest,'review predecessor differs');
 }
 return signed(RECOVERY,{schema_version:1,purpose:'proxy_admission_refund_recovery',refund_id:view.refund_id,
  authorization_digest:view.authorization_digest,policy_hash:policy.policy_hash,authorizer_pubkey:authorizer,
  revision:view.next_revision,previous_recovery_digest:view.previous_recovery_digest,preparation_digest:view.preparation_digest,
  first_dispatch_at_ms:view.first_dispatch_at_ms,review_code:view.review_code,review_evidence_hash:evidenceHash,
  approved_at_ms:now,expires_at_ms:now+approvalMs},key);
}

function json(file) {const raw=readCustodyFile(file,{max:16384});try{return JSON.parse(raw.toString('utf8'));}finally{raw.fill(0);}}
function protectedOutput(file,value) {
 need(path.isAbsolute(file)&&fs.realpathSync(path.dirname(file))===path.dirname(file),'canonical output parent required');
 const parent=fs.statSync(path.dirname(file));need(parent.uid===process.getuid()&&(parent.mode&0o077)===0,'owner-only output directory required');
 const fd=fs.openSync(file,fs.constants.O_WRONLY|fs.constants.O_CREAT|fs.constants.O_EXCL|fs.constants.O_NOFOLLOW,0o600);
 try{fs.writeFileSync(fd,JSON.stringify(value)+'\n');fs.fsyncSync(fd);}finally{fs.closeSync(fd);}
 const dir=fs.openSync(path.dirname(file),fs.constants.O_RDONLY);try{fs.fsyncSync(dir);}finally{fs.closeSync(dir);}
}
export async function main(args=process.argv.slice(2)) {
 const [action,configFile,subject,evidenceFile,outputFile,...extra]=args;
 need(['inspect','prepare','submit'].includes(action)&&configFile&&subject&&extra.length===0
  &&(action==='prepare'?evidenceFile&&outputFile:!evidenceFile&&!outputFile),'use inspect CONFIG REFUND_ID | prepare CONFIG REFUND_ID EVIDENCE_FILE OUTPUT | submit CONFIG APPROVAL_FILE');
 const c=json(path.resolve(configFile));
 shape(c,['api_origin','api_credential_file','policy_file','review_key_file','review_password_file','approval_ms','allow_loopback_http']);
 need(typeof c.allow_loopback_http==='boolean','explicit review transport required');
 const secret=readCustodyFile(c.api_credential_file,{max:4096});let api;
 try{api=new RefundReviewApi({origin:c.api_origin,credential:secret.toString('utf8').trim(),allowLoopbackHttp:c.allow_loopback_http});}finally{secret.fill(0);}
 if(action==='submit') {
  const recovery=json(path.resolve(subject)); // Submit the retained signed bytes; never re-sign on retry.
  verifyRefundSignature(RECOVERY,recovery,recovery?.body?.authorizer_pubkey);
  const result=await api.post('recover',{recovery});
  need(result.refund_id===recovery.body.refund_id&&['reserved','reconcile','review','leased','delivered'].includes(result.state)
   &&typeof result.replayed==='boolean','invalid recovery acknowledgement');
  return {refund_id:result.refund_id,state:result.state,replayed:result.replayed};
 }
 need(opaque(subject),'invalid refund ID');
 const {review:view}=await api.post('review',{refund_id:subject});need(view?.refund_id===subject,'review identity differs');
 if(action==='inspect')return view;
 const policy=json(c.policy_file),evidence=readCustodyFile(path.resolve(evidenceFile)),password=readCustodyFile(c.review_password_file,{max:4096});
 let pem;
 try {
  pem=readCustodyFile(c.review_key_file,{max:16384});
  need(pem.toString('ascii',0,40).startsWith('-----BEGIN ENCRYPTED PRIVATE KEY-----'),'encrypted reviewer key required');
  const key=createPrivateKey({key:pem,format:'pem',passphrase:password});
  const recovery=await prepareRecovery(view,{key,policy,evidenceHash:createHash('sha256').update(evidence).digest('hex'),approvalMs:c.approval_ms});
  protectedOutput(path.resolve(outputFile),recovery);
  return {refund_id:subject,state:'prepared_not_submitted',revision:recovery.body.revision,expires_at_ms:recovery.body.expires_at_ms};
 } finally{evidence.fill(0);password.fill(0);pem?.fill(0);}
}
if(process.argv[1]&&path.resolve(process.argv[1])===fileURLToPath(import.meta.url)) {
 main().then(result=>process.stdout.write(JSON.stringify(result)+'\n')).catch(()=>{
  process.stderr.write('Admission return review rejected; check protected configuration, current review state and approval expiry.\n');process.exitCode=1;
 });
}

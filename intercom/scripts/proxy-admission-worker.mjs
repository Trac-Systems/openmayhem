#!/usr/bin/env node
// Dedicated admission verification/signing. Never invokes deposit/bridge/credit
// or contract mutation code. SITE owns durable claims, immutable permit bodies
// and phase-specific leases; exact retries recompute only the original result.
import { randomBytes, createPrivateKey, createPublicKey, sign, verify } from 'node:crypto';
import fs from 'node:fs';
import { fileURLToPath } from 'node:url';
import { performance } from 'node:perf_hooks';
import { proxyAdmissionSigningBytes, proxyCanonicalSigningBytes, verifyProxyAdmissionPermit } from '../contract/proxy-protocol.js';
import { validateProxySnapshotProof } from '../features/mayhem/proxy-canonical-view.js';
import { PURPOSE, base, need, shape, hex, uint, validateNetwork, validateWork, validateEvidence, validateEvidenceSet, evidenceCommitment, validateReceipt, evidenceSeed, evidenceAppend, EVIDENCE_PROGRESS_DOMAIN, EVIDENCE_PAGE_SIZE, amount } from './proxy-admission-wire.mjs';
import { verifyTnkObservedTransfer } from './proxy-admission-tnk.mjs';
export { verifyTnkObservedTransfer } from './proxy-admission-tnk.mjs';
import { RetryWork, ReviewWork, verifyTapTransferReceipt, parseHexInt } from './retail-crypto-verification.mjs';

export function fixedOrigin(value, { allowLoopbackHttp = false } = {}) {
  const u = new URL(value);
  need(!u.username && !u.password && !u.search && !u.hash && u.pathname === '/', 'invalid fixed origin');
  const loopback = ['127.0.0.1','[::1]'].includes(u.hostname);
  need(u.protocol === 'https:' || (allowLoopbackHttp && loopback && u.protocol === 'http:'), 'HTTPS or explicit literal loopback required');
  return u.origin;
}
export async function boundedJson(url, { body, headers = {}, signal, fetcher = fetch, maxBytes = 16384, method = 'POST' } = {}) {
  const response = await fetcher(url, { method, headers: { 'content-type':'application/json', ...headers },
    ...(body === undefined ? {} : { body:JSON.stringify(body) }), signal, redirect:'error' });
  need(response.ok, `upstream HTTP ${response.status}`);
  const length = response.headers.get('content-length');
  need(length === null || (/^[0-9]+$/.test(length) && Number(length) <= maxBytes), 'response exceeds bound');
  need(response.body, 'missing upstream body');
  const chunks = []; let size = 0;
  const reader = response.body.getReader();
  try { for (;;) { const { done, value } = await reader.read(); if (done) break;
    size += value.byteLength; need(size <= maxBytes, 'response exceeds bound'); chunks.push(Buffer.from(value));
  } } finally { await reader.cancel().catch(() => {}); }
  return JSON.parse(Buffer.concat(chunks).toString('utf8'));
}
export function validatePolicy(v, network, nonce, recovery = null) {
  shape(v, ['ok','schema_version','lane','requester','request_nonce','context','proof','registry_enabled','fee_policy_hash','active_issuers','max_permit_epochs',
    ...(recovery?['provider_pubkey','recovery','recovery_state','enrollment']:[])]);
  shape(v.context, [...Object.keys(network),'epoch']);
  need(v.ok === true && v.schema_version === 1 && v.lane === 'proxy' && hex(v.requester)
    && v.request_nonce === nonce && uint(v.context.epoch,1) && Object.keys(network).every(k=>v.context[k]===network[k])
    && typeof v.registry_enabled === 'boolean' && hex(v.fee_policy_hash) && uint(v.max_permit_epochs,1)
    && Array.isArray(v.active_issuers) && v.active_issuers.length > 0 && v.active_issuers.length <= 16
    && v.active_issuers.every((x,i,a)=>hex(x)&&(i===0||a[i-1]<x)), 'canonical policy does not match');
  validateProxySnapshotProof(v.proof);
  if(recovery) {
    const {provider_pubkey,...bindings}=recovery;
    shape(v.recovery,Object.keys(bindings));
    need(v.provider_pubkey===provider_pubkey&&Object.keys(bindings).every(k=>v.recovery[k]===bindings[k]),'canonical recovery binding differs');
    shape(v.enrollment,['provider_pubkey','entitlement_id','provider_revoked','admission_revoked']);
    shape(v.recovery_state,['entitlement_used','invoice_used','evidence_used','admission_revoked','generation']);
    const e=v.enrollment,r=v.recovery_state;
    need(e.provider_pubkey===provider_pubkey&&(e.entitlement_id===null||hex(e.entitlement_id))
      &&typeof e.provider_revoked==='boolean'&&typeof e.admission_revoked==='boolean'&&typeof r.admission_revoked==='boolean','invalid enrollment recovery');
    for(const used of [r.entitlement_used,r.invoice_used,r.evidence_used]) if(used!==null){shape(used,['provider_pubkey','entitlement_id']);need(hex(used.provider_pubkey)&&hex(used.entitlement_id),'invalid recovery consumption');}
    if(r.generation!==null){shape(r.generation,['revision','permit_digest']);need(uint(r.generation.revision,1)&&hex(r.generation.permit_digest),'invalid recovery generation');}
    if(e.entitlement_id!==null||e.provider_revoked||e.admission_revoked||r.admission_revoked
      ||r.generation!==null||[r.entitlement_used,r.invoice_used,r.evidence_used].some(x=>x!==null)) throw new ReviewWork('reissue_requires_original_canonical_reconciliation');
  }
  return v;
}
export class AdmissionApi {
  constructor({ origin, credential, phase, allowLoopbackHttp = false, fetcher = fetch }) {
    this.origin = fixedOrigin(origin,{allowLoopbackHttp}); need(['verify','issue'].includes(phase) && typeof credential === 'string' && credential.length >= 16, 'worker custody configuration required');
    this.credential=credential; this.phase=phase; this.fetcher=fetcher;
  }
  async post(action, body, signal) {
    need(['pull','renew','retry','review','evidence','permit','reconcile','evidence-page','evidence-progress'].includes(action), 'unsupported worker action');
    need(body.phase === this.phase && !(this.phase==='verify'&&['permit','reconcile','evidence-page','evidence-progress'].includes(action)) && !(this.phase==='issue'&&action==='evidence'), 'worker role mismatch');
    return await boundedJson(`${this.origin}/internal/proxy-admission-worker/${action}`, {body, signal,
      headers:{authorization:`Bearer ${this.credential}`}, fetcher:this.fetcher});
  }
}
export class AdmissionWorker {
  constructor({ phase, api, coreOrigin, network, feePolicyHash, issuerPubkey, verifyReceipt = null, signPermit = null,
    allowLoopbackHttp = false, fetcher = fetch, timeoutMs = 15000, now = Date.now, rails }) {
    need(['verify','issue'].includes(phase) && api.phase === phase && hex(feePolicyHash) && hex(issuerPubkey)
      && uint(timeoutMs,100) && timeoutMs <= 15000, 'invalid worker configuration');
    need(phase==='verify' ? typeof verifyReceipt==='function'&&signPermit===null : typeof signPermit==='function'&&verifyReceipt===null, 'separate verifier and issuer custody required');
    need(Array.isArray(rails)&&rails.length>0&&rails.length<=3&&rails.every((r,i,a)=>['fiat','tap','tnk'].includes(r)&&(i===0||a[i-1]<r)),'explicit sorted worker rails required');
    validateNetwork(network); this.options={phase,api,network,feePolicyHash,issuerPubkey,verifyReceipt,signPermit,fetcher,timeoutMs,now,rails};
    this.coreOrigin=fixedOrigin(coreOrigin,{allowLoopbackHttp}); this.active=false;
  }
  async policy(signal, permit = null) {
    const o=this.options, nonce=randomBytes(32).toString('hex'), started=performance.now();
    const recovery=permit?{entitlement_id:permit.entitlement_id,invoice_commitment:permit.invoice_commitment,evidence_commitment:permit.evidence_commitment}:null;
    const result=await boundedJson(`${this.coreOrigin}/v1/proxy/admission-policy`,{body:{request_nonce:nonce,...(recovery?{provider_pubkey:permit.provider_pubkey,recovery}:{})},signal,fetcher:o.fetcher,maxBytes:8192});
    need(performance.now()-started <= o.timeoutMs, 'canonical policy expired');
    return validatePolicy(result,o.network,nonce,recovery?{provider_pubkey:permit.provider_pubkey,...recovery}:null);
  }
  async complete(work, signal) {
    const o=this.options; await validateWork(work,o.phase);
    const i=work.invoice;
    need(o.rails.includes(i.rail) && work.lease_expires_at_ms > o.now() && Object.keys(o.network).every(k=>i.network[k]===o.network[k])
      && i.fee_policy_hash===o.feePolicyHash && i.issuer_pubkey===o.issuerPubkey, 'invoice differs from configured custody/network');
    if(o.phase==='verify' && (work.reference_assigned_at_ms>i.quote_expires_at_ms || work.reference_assigned_at_ms>o.now())) throw new ReviewWork('late_reference_pending_policy');
    if(o.phase==='issue'&&work.evidence.format==='paged-v1') {
      const progress=await this.checkedEvidenceProgress(work);
      if(progress.count<work.evidence.receipt_count) return this.completeEvidencePage(work,progress,signal);
      need(progress.root===work.evidence.root&&progress.total===work.evidence.total_amount,'incomplete evidence progress');
    }
    const renewal=o.phase==='issue'&&work.permit.issuance_revision>1;
    if(renewal&&work.previous_permit===undefined) throw new ReviewWork('reissue_requires_original_canonical_reconciliation');
    const policy=await this.policy(signal,renewal?work.permit:null);
    if(!policy.registry_enabled || policy.fee_policy_hash!==i.fee_policy_hash || !policy.active_issuers.includes(i.issuer_pubkey)) throw new ReviewWork('canonical_policy_changed');
    if(o.phase==='verify') {
      const receipt=await o.verifyReceipt(work,signal);
      if(receipt.paid_at_ms!==undefined && (receipt.paid_at_ms>i.quote_expires_at_ms || receipt.paid_at_ms>o.now())) throw new ReviewWork('late_payment_pending_policy');
      const evidence={canonical_epoch:policy.context.epoch,evidence_commitment:await evidenceCommitment(work,receipt),receipt};
      await validateEvidence(evidence,work);
      return {action:'evidence',body:{...base(work),...evidence}};
    }
    const total=await validateEvidenceSet(work.evidence,work);
    if(total<BigInt(i.amount_base_units)) throw new ReviewWork('verified_payment_short');
    for(const item of work.evidence.receipts??[]) {
      if(item.reference_assigned_at_ms>i.quote_expires_at_ms || item.reference_assigned_at_ms>o.now() || (item.receipt.paid_at_ms!==undefined && (item.receipt.paid_at_ms>i.quote_expires_at_ms || item.receipt.paid_at_ms>o.now()))) throw new ReviewWork('late_payment_pending_policy');
    }
    const p=work.permit;
    // SITE stores this exact body before dispatch. The signer cannot choose a
    // new nonce/window/revision after a timeout, expiry, or uncertain append.
    if(!renewal&&p.issuance_revision!==1) throw new ReviewWork('reissue_requires_original_canonical_reconciliation');
    if(renewal) {
      const previous=work.previous_permit;
      const mutable=['nonce','issuance_revision','valid_from_epoch','expires_after_epoch'];
      need(previous.issuance_revision+1===p.issuance_revision&&previous.nonce!==p.nonce
        &&p.valid_from_epoch>previous.expires_after_epoch&&policy.context.epoch>previous.expires_after_epoch
        &&Object.keys(previous).filter(k=>!mutable.includes(k)).every(k=>previous[k]===p[k]),'successor does not preserve expired original permit');
    }
    for(const field of ['provider_pubkey','issuer_pubkey','entitlement_id','fee_policy_hash','invoice_commitment','initial_operation_digest','rail','accepted_value_au']) need(p[field]===i[field], 'permit invoice binding differs');
    need(p.accepted_amount===i.amount_base_units && p.evidence_commitment===work.evidence.evidence_commitment
      && p.valid_from_epoch===work.evidence.canonical_epoch, 'permit evidence binding differs');
    if(p.valid_from_epoch>policy.context.epoch || p.expires_after_epoch<policy.context.epoch
      || p.expires_after_epoch-p.valid_from_epoch+1>policy.max_permit_epochs) throw new ReviewWork('permit_window_requires_original_reconciliation');
    need(Object.keys(o.network).every(k=>p[k]===o.network[k]), 'permit network differs');
    if(signal.aborted) throw signal.reason;
    const signature=await o.signPermit(proxyAdmissionSigningBytes(p));
    const envelope={permit:p,issuer_signature:signature};
    await verifyProxyAdmissionPermit(envelope,{...o.network,provider_pubkey:i.provider_pubkey,
      initial_operation_digest:i.initial_operation_digest,fee_policy_hash:i.fee_policy_hash,epoch:policy.context.epoch,
      max_permit_epochs:policy.max_permit_epochs,active_issuers:policy.active_issuers},
      (sig,bytes,key)=>verify(null,bytes,createPublicKey({key:Buffer.concat([Buffer.from('302a300506032b6570032100','hex'),Buffer.from(key,'hex')]),format:'der',type:'spki'}),Buffer.from(sig,'hex')));
    return {action:'permit',body:{...base(work),...envelope}};
  }
  async checkedEvidenceProgress(work) {
    const v=work.evidence_progress;
    if(v===null) return {invoice_id:work.invoice_id,invoice_commitment:work.invoice.invoice_commitment,
      evidence_commitment:work.evidence.evidence_commitment,count:0,root:await evidenceSeed(work),total:'0',last_key:''};
    shape(v,['checkpoint','signature']);
    const c=v.checkpoint;shape(c,['invoice_id','invoice_commitment','evidence_commitment','count','root','total','last_key']);
    need(c.invoice_id===work.invoice_id&&c.invoice_commitment===work.invoice.invoice_commitment
      &&c.evidence_commitment===work.evidence.evidence_commitment&&uint(c.count,1)&&c.count<=work.evidence.receipt_count
      &&hex(c.root)&&amount(c.total)&&typeof c.last_key==='string'&&c.last_key.length>0&&c.last_key.length<=900
      &&typeof v.signature==='string'&&/^[0-9a-f]{128}$/.test(v.signature),'invalid evidence progress');
    const publicKey=createPublicKey({key:Buffer.concat([Buffer.from('302a300506032b6570032100','hex'),Buffer.from(work.invoice.issuer_pubkey,'hex')]),format:'der',type:'spki'});
    need(verify(null,proxyCanonicalSigningBytes(EVIDENCE_PROGRESS_DOMAIN,c),publicKey,Buffer.from(v.signature,'hex')),'unsigned evidence progress');
    return c;
  }
  async completeEvidencePage(work,progress,signal) {
    const o=this.options,i=work.invoice;
    const page=await o.api.post('evidence-page',base(work),signal);
    shape(page,['schema_version','purpose','phase','evidence_commitment','from_count','members']);
    need(page.schema_version===1&&page.purpose===PURPOSE&&page.phase==='issue'
      &&page.evidence_commitment===work.evidence.evidence_commitment&&page.from_count===progress.count
      &&Array.isArray(page.members)&&page.members.length===Math.min(EVIDENCE_PAGE_SIZE,work.evidence.receipt_count-progress.count),'invalid evidence page');
    const next={...progress};let total=BigInt(next.total);
    for(const entry of page.members) {
      shape(entry,['sequence','member']);const m=entry.member;shape(m,['payment_reference','reference_assigned_at_ms','receipt']);
      need(entry.sequence===next.count+1,'nonsequential evidence page');
      const r=validateReceipt(m.receipt,i,m.payment_reference);
      need(r.physical_key>next.last_key,'duplicate or unsorted evidence page');
      need(uint(m.reference_assigned_at_ms,i.created_at_ms),'invalid reference observation');
      if(m.reference_assigned_at_ms>i.quote_expires_at_ms||m.reference_assigned_at_ms>o.now()
        ||(r.paid_at_ms!==undefined&&(r.paid_at_ms>i.quote_expires_at_ms||r.paid_at_ms>o.now()))) throw new ReviewWork('late_payment_pending_policy');
      next.root=await evidenceAppend(work,next.root,++next.count,m);next.last_key=r.physical_key;
      total+=BigInt(r.amount_base_units);need(total<(1n<<128n),'evidence total overflow');
    }
    next.total=String(total);
    if(next.count===work.evidence.receipt_count) need(next.root===work.evidence.root&&next.total===work.evidence.total_amount,'paged evidence differs from retained manifest');
    need(!signal.aborted&&work.lease_expires_at_ms>o.now(),'lease expired before evidence progress');
    const signed={checkpoint:next,signature:await o.signPermit(proxyCanonicalSigningBytes(EVIDENCE_PROGRESS_DOMAIN,next))};
    // The checkpoint certifies immutable evidence only, not a permit or payment.
    // It remains useful across renewal; the final permit rechecks canonical state.
    await this.checkedEvidenceProgress({...work,evidence_progress:signed});
    return {action:'evidence-progress',body:{...base(work),progress:signed}};
  }
  async runOnce() {
    need(!this.active,'worker is busy'); this.active=true;
    const o=this.options, signal=AbortSignal.timeout(o.timeoutMs); let work;
    try {
      const response=await o.api.post('pull',{schema_version:1,purpose:PURPOSE,phase:o.phase,rails:o.rails},signal);
      shape(response,['schema_version','purpose','phase','work']);
      need(response.schema_version===1 && response.purpose===PURPOSE && response.phase===o.phase,'invalid pull response');
      if(response.work===null) return {status:'idle'};
      work=await validateWork(response.work,o.phase);
      // Refuse insufficient leases instead of allowing signing after ownership
      // may have changed. Renewal is authenticated and preserves original work.
      if(work.lease_expires_at_ms-o.now()<o.timeoutMs) {
        const renewed=await o.api.post('renew',base(work),signal);
        shape(renewed,['schema_version','purpose','phase','lease_expires_at_ms']);
        need(renewed.schema_version===1&&renewed.purpose===PURPOSE&&renewed.phase===o.phase
          && uint(renewed.lease_expires_at_ms,o.now()+o.timeoutMs),'lease renewal failed');
        work={...work,lease_expires_at_ms:renewed.lease_expires_at_ms};
      }
      const result=await this.complete(work,signal);
      need(!signal.aborted && work.lease_expires_at_ms>o.now(),'lease expired before completion');
      const ack=await o.api.post(result.action,result.body,signal);
      shape(ack,['schema_version','purpose','phase','accepted']);
      need(ack.schema_version===1&&ack.purpose===PURPOSE&&ack.phase===o.phase&&ack.accepted===true,'completion was not acknowledged');
      return {status:'accepted',phase:o.phase};
    } catch(error) {
      // An ambiguous completion is retried under SITE's original durable work.
      // Do not return a second signature/permit generation or dump upstream data.
      if(work && !signal.aborted) {
        if(o.phase==='issue'&&error instanceof ReviewWork&&error.reason==='permit_window_requires_original_reconciliation') {
          try {
            const ack=await o.api.post('reconcile',base(work),signal);
            shape(ack,['schema_version','purpose','phase','accepted']);
            need(ack.schema_version===1&&ack.purpose===PURPOSE&&ack.phase==='issue'&&ack.accepted===true,'recovery not acknowledged');
            return {status:'retry',phase:o.phase};
          } catch {
            await o.api.post('retry',{...base(work),code:'canonical_recovery_unavailable',delay_seconds:30},signal).catch(()=>{});
            return {status:'retry',phase:o.phase};
          }
        }
        const action=error instanceof ReviewWork?'review':'retry';
        const body=action==='review'?{...base(work),reason:error.reason}:{...base(work),code:error instanceof RetryWork?error.code:'verification_unavailable',delay_seconds:error instanceof RetryWork?error.delaySeconds:30};
        await o.api.post(action,body,signal).catch(()=>{});
      }
      return {status:error instanceof ReviewWork?'review':'retry',phase:o.phase};
    } finally { this.active=false; }
  }
}

// Independent exact-log TAP verification. No ambiguous same-amount selection,
// bridge, fallback-finality guess or body-selected RPC endpoint.
export function tapVerifier({origin,chainId,tokenContract,allowLoopbackHttp=false,fetcher=fetch,rpc=null}) {
  const rpcOrigin=rpc?null:fixedOrigin(origin,{allowLoopbackHttp}); need(uint(chainId,1)&&/^0x[0-9a-f]{40}$/.test(tokenContract),'invalid TAP asset configuration');
  let id=0;
  const call=rpc??(async(method,params,signal)=>{const requestId=++id;const result=await boundedJson(rpcOrigin,{body:{jsonrpc:'2.0',id:requestId,method,params},signal,fetcher,maxBytes:262144});need(result.jsonrpc==='2.0'&&result.id===requestId&&!result.error,'TAP RPC unavailable');return result.result;});
  return async(work,signal)=>{
    const i=work.invoice,p=work.payment_reference;
    need(i.rail==='tap'&&i.collection.chain_id===chainId&&i.collection.token_contract===tokenContract,'TAP asset differs');
    need(parseHexInt(await call('eth_chainId',[],signal),'chain')===BigInt(chainId),'TAP network differs');
    const receipt=await call('eth_getTransactionReceipt',[p.transaction_hash],signal);
    if(!receipt) throw new RetryWork('transfer_pending',15);
    const logs=receipt.logs.filter(log=>parseHexInt(log.logIndex,'log index')===BigInt(p.log_index));
    need(logs.length===1,'exact TAP log missing or duplicated');
    const finalized=await call('eth_getBlockByNumber',['finalized',false],signal);
    if(!finalized?.number) throw new RetryWork('finality_unavailable',30);
    const e=verifyTapTransferReceipt({...receipt,logs},{transactionHash:p.transaction_hash,token:tokenContract,destination:i.collection.destination,
      amountBaseUnits:String(parseHexInt(logs[0].data,'transfer amount')),latestBlock:parseHexInt(finalized.number,'finalized block'),finalizedBlock:parseHexInt(finalized.number,'finalized block')});
    const block=await call('eth_getBlockByNumber',[receipt.blockNumber,false],signal);
    need(block?.hash===receipt.blockHash && block.number===receipt.blockNumber,'TAP block changed');
    const paid=Number(parseHexInt(block.timestamp,'block timestamp'))*1000; need(uint(paid,1),'TAP block time invalid');
    return {rail:'tap',physical_key:`tap/${chainId}/${tokenContract}/${p.transaction_hash}/${p.log_index}`,amount_base_units:String(e.tokenAmountBaseUnits),finalized:true,
      transaction_hash:p.transaction_hash,log_index:p.log_index,chain_id:chainId,token_contract:tokenContract,from_address:e.fromAddress,to_address:e.toAddress,
      block_number:String(e.blockNumber),block_hash:e.blockHash,paid_at_ms:paid};
  };
}

export function stripeVerifier({account,livemode,currency,credential,fetcher=fetch}) {
  need(/^acct_[A-Za-z0-9]{1,100}$/.test(account)&&typeof livemode==='boolean'&&/^[a-z]{3}$/.test(currency)&&typeof credential==='string'&&credential.length>=16,'invalid Stripe verification configuration');
  return async(work,signal)=>{
    const i=work.invoice,p=work.payment_reference;
    need(i.rail==='fiat'&&i.collection.stripe_account===account&&i.collection.livemode===livemode&&i.collection.currency===currency,'Stripe account differs');
    const read=path=>boundedJson(`https://api.stripe.com/v1/${path}`,{method:'GET',signal,fetcher,maxBytes:262144,headers:{authorization:`Bearer ${credential}`,'stripe-account':account}});
    // Refetch the exact webhook event as well as the PaymentIntent; SITE's
    // verified-webhook reference is a correlation, never the payment evidence.
    const event=await read(`events/${p.verified_webhook_event_id}`), pi=await read(`payment_intents/${p.payment_intent_id}`);
    need(event.id===p.verified_webhook_event_id&&event.livemode===livemode&&event.type==='payment_intent.succeeded'
      &&event.data?.object?.id===p.payment_intent_id&&(!event.account||event.account===account),'Stripe event differs');
    if(pi.status!=='succeeded') throw new RetryWork('payment_pending',30);
    need(pi.id===p.payment_intent_id&&pi.livemode===livemode&&pi.currency===currency&&uint(pi.amount_received,1)
      &&pi.amount===pi.amount_received&&typeof pi.latest_charge==='string'&&/^ch_[A-Za-z0-9]{1,100}$/.test(pi.latest_charge),'Stripe payment differs');
    const m=pi.metadata??{};
    need(m.purpose===PURPOSE&&m.invoice_id===work.invoice_id&&m.invoice_commitment===i.invoice_commitment
      &&m.provider_pubkey===i.provider_pubkey&&m.initial_operation_digest===i.initial_operation_digest,'Stripe purpose binding differs');
    const charge=await read(`charges/${pi.latest_charge}`);
    need(charge.id===pi.latest_charge&&charge.payment_intent===pi.id&&charge.livemode===livemode&&charge.paid===true&&charge.captured===true
      &&charge.refunded===false&&charge.disputed===false&&charge.amount_refunded===0&&charge.currency===currency&&charge.amount===pi.amount_received
      &&uint(event.created,1),'Stripe charge needs review');
    return {rail:'fiat',physical_key:`fiat/${account}/${livemode?'live':'test'}/${pi.id}`,amount_base_units:String(pi.amount_received),finalized:true,
      stripe_account:account,livemode,payment_intent_id:pi.id,charge_id:charge.id,currency,paid_at_ms:event.created*1000};
  };
}

// TNK adapter receives the existing bounded signed-MSB receipt verifier. Its
// network/reader config is operator-owned; no buyer wallet or bridge is opened.
export function tnkVerifier({network,verifyTransfer}) {
  need(['mainnet','testnet1'].includes(network)&&typeof verifyTransfer==='function','invalid TNK verifier configuration');
  return async(work,signal)=>{
    need(work.invoice.rail==='tnk'&&work.invoice.collection.network===network,'TNK network differs');
    const e=await verifyTransfer({transaction_hash:work.payment_reference.transaction_hash,destination:work.invoice.collection.destination,
      token_amount_base_units:work.invoice.amount_base_units},signal);
    need(!signal.aborted&&e.finalized===true&&e.transactionHash===work.payment_reference.transaction_hash,'TNK observation incomplete');
    return {rail:'tnk',physical_key:`tnk/${network}/${e.transactionHash}`,amount_base_units:String(e.tokenAmountBaseUnits),finalized:true,
      network,transaction_hash:e.transactionHash,from_address:e.fromAddress,to_address:e.toAddress,confirmed_signed_length:Number(e.blockNumber)};
  };
}

export async function main(env=process.env) {
  // Disabled unless an operator explicitly selects a custody role/config.
  if(env.PROXY_ADMISSION_WORKER_ENABLED!=='1') throw new Error('Admission worker disabled');
  need(env.PROXY_ADMISSION_WORKER_CONFIG,'operator configuration required');
  const c=JSON.parse(fs.readFileSync(env.PROXY_ADMISSION_WORKER_CONFIG,'utf8'));
  const phase=c.phase; need(['verify','issue'].includes(phase),'explicit custody role required');
  need(phase==='issue'?!c.verification:!c.issuer_key_file,'credentials must be separated by custody role');
  const credential=fs.readFileSync(c.api_credential_file,'utf8').trim();
  const api=new AdmissionApi({origin:c.api_origin,credential,phase,allowLoopbackHttp:c.allow_loopback_http===true});
  let verifyReceipt=null,signPermit=null,closeReader=async()=>{},discovery=null;
  if(phase==='issue') {
    const key=createPrivateKey(fs.readFileSync(c.issuer_key_file)); need(key.asymmetricKeyType==='ed25519','issuer key must be Ed25519');
    const publicKey=createPublicKey(key).export({format:'der',type:'spki'}).subarray(-32).toString('hex');
    need(publicKey===c.issuer_pubkey,'issuer key differs'); signPermit=bytes=>sign(null,bytes,key).toString('hex');
  } else {
    need(c.verification&&['tap','tnk','fiat'].includes(c.verification.rail),'one explicitly configured verification rail required');
    const v=c.verification;
    if(v.rail==='tap') {
      const {admissionTapRpc,tapDiscovery,AdmissionDiscoveryWorker}=await import('./proxy-admission-discovery.mjs');
      const rpc=admissionTapRpc({urls:v.rpc_urls??[v.rpc_origin],chainId:v.chain_id,allowLoopbackHttp:c.allow_loopback_http===true});
      verifyReceipt=tapVerifier({rpc,chainId:v.chain_id,tokenContract:v.token_contract});
      if(c.discovery_enabled===true)discovery=new AdmissionDiscoveryWorker({origin:c.api_origin,credential,
        stream:{rail:'tap',chain_id:v.chain_id,token_contract:v.token_contract},scan:tapDiscovery({rpc,chainId:v.chain_id,tokenContract:v.token_contract}),allowLoopbackHttp:c.allow_loopback_http===true});
    }
    if(v.rail==='fiat') verifyReceipt=stripeVerifier({account:v.stripe_account,livemode:v.livemode,currency:v.currency,credential:fs.readFileSync(v.credential_file,'utf8').trim()});
    if(v.rail==='tnk') {
      need(['mainnet','testnet1'].includes(v.network)&&uint(v.lookback,1)&&v.lookback<=100000&&uint(v.finality,1)&&uint(v.reader_timeout_seconds,1)&&v.reader_timeout_seconds<=10&&v.state_dir,'explicit bounded TNK reader configuration required');
      need(hex(v.msb_bootstrap)&&v.msb_bootstrap===c.network.msb_bootstrap&&typeof v.channel==='string'&&v.channel.length>0
        &&Array.isArray(v.dht_bootstrap)&&v.dht_bootstrap.length<=16,'explicit TNK network transport binding required');
      const { MainSettlementBus }=await import('trac-msb/src/index.js');
      const { createLocalConfig }=await import('./msb-local-common.mjs');
      const config=createLocalConfig({network:v.network,stateDir:v.state_dir,storeName:'proxy-admission-reader',
        channel:v.channel,bootstrap:v.msb_bootstrap,dhtBootstrap:v.dht_bootstrap,enableWallet:false});
      need(String(config.networkId)===c.network.network_id,'TNK configured network identity differs');
      const msb=new MainSettlementBus(config); let ready=null;
      const origin=fixedOrigin(c.core_origin,{allowLoopbackHttp:c.allow_loopback_http===true});
      const canonicalFrontier=async(signal)=>{
        ready??=msb.ready();
        let abort;
        try { await Promise.race([ready,new Promise((_,reject)=>{abort=()=>reject(signal.reason);signal.addEventListener('abort',abort,{once:true});if(signal.aborted)abort();})]); }
        finally {if(abort)signal.removeEventListener('abort',abort);}
        const status=await boundedJson(`${origin}/status`,{method:'GET',signal,maxBytes:16384});
        const frontier=status?.msb?.signedLength; need(uint(frontier,1)&&String(status.msb.networkId)===c.network.network_id&&status.msb.bootstrapHex===c.network.msb_bootstrap,'TNK canonical frontier unavailable or foreign');
        return frontier;
      };
      verifyReceipt=tnkVerifier({network:v.network,verifyTransfer:async(intent,signal)=>{
        const frontier=await canonicalFrontier(signal);
        return await verifyTnkObservedTransfer(msb,intent,{frontier,finality:v.finality,timeoutSeconds:v.reader_timeout_seconds,signal,addressPrefix:config.addressPrefix});
      }});
      if(c.discovery_enabled===true){
        const {tnkDiscovery,AdmissionDiscoveryWorker}=await import('./proxy-admission-discovery.mjs');
        discovery=new AdmissionDiscoveryWorker({origin:c.api_origin,credential,stream:{rail:'tnk',network:v.network,msb_bootstrap:v.msb_bootstrap},
          scan:tnkDiscovery({msb,network:v.network,msbBootstrap:v.msb_bootstrap,frontier:canonicalFrontier}),allowLoopbackHttp:c.allow_loopback_http===true});
      }
      closeReader=async()=>{await msb.close();};
    }
  }
  const worker=new AdmissionWorker({phase,api,coreOrigin:c.core_origin,network:c.network,feePolicyHash:c.fee_policy_hash,
    issuerPubkey:c.issuer_pubkey,verifyReceipt,signPermit,rails:phase==='verify'?[c.verification.rail]:c.rails,allowLoopbackHttp:c.allow_loopback_http===true});
  try { do {
    if(discovery)try{const result=await discovery.runOnce();if(result.status!=='idle')process.stdout.write(JSON.stringify({event:'proxy_admission_discovery',...result})+'\n');}
      catch{process.stderr.write('{"event":"proxy_admission_discovery","status":"unavailable"}\n');}
    const result=await worker.runOnce(); process.stdout.write(JSON.stringify({event:'proxy_admission_worker',...result})+'\n');
    if(env.PROXY_ADMISSION_WORKER_ONCE==='1') break;
    await new Promise(resolve=>setTimeout(resolve,1000));
  } while(true); } finally { await closeReader(); }
}
if(process.argv[1]&&fileURLToPath(import.meta.url)===process.argv[1]) main().catch(()=>{process.stderr.write('Admission worker failed; inspect operator configuration.\n');process.exitCode=1;});

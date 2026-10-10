#!/usr/bin/env node
// Dedicated platform return worker. Disabled by default; does not start as an
// ordinary provider, receive custody from requests, or touch inference payouts.
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createPrivateKey } from 'node:crypto';
import { setTimeout as sleep } from 'node:timers/promises';
import { fixedOrigin,boundedJson } from './proxy-admission-worker.mjs';
import { readCustodyFile } from './proxy-admission-custody.mjs';
import { StripeAdmissionRefund } from './proxy-admission-refund-stripe.mjs';
import { need,shape,uint } from './proxy-admission-wire.mjs';
import { RetryWork,ReviewWork } from './retail-crypto-verification.mjs';
const envelope={schema_version:1,purpose:'proxy_admission_refund'};
export class RefundApi {
  constructor({origin,credential,allowLoopbackHttp=false,fetcher=fetch}) {
    this.origin=fixedOrigin(origin,{allowLoopbackHttp});need(typeof credential==='string'&&/^[\x21-\x7e]{32,256}$/.test(credential),'refund API credential required');
    this.credential=credential;this.fetcher=fetcher;
  }
  async post(action,body,signal) {
    need(['pull','dispatch','defer','delivered'].includes(action),'execution worker cannot authorize a return');
    const result=await boundedJson(`${this.origin}/internal/proxy-admission-refunds/${action}`,{body:{...body,...envelope},signal,
      headers:{authorization:`Bearer ${this.credential}`},fetcher:this.fetcher,maxBytes:32768});
    need(result?.schema_version===1&&result.purpose===envelope.purpose,'refund API purpose differs');return result;
  }
}
export class AdmissionRefundWorker {
  constructor({api,adapters,timeoutMs}) {
    need(api&&typeof api.post==='function'&&adapters&&Object.keys(adapters).length>0&&Object.keys(adapters).every(r=>['fiat','tnk','tap'].includes(r))
      &&uint(timeoutMs,100)&&timeoutMs<=15000,'explicit refund worker configuration required');
    this.api=api;this.adapters=adapters;this.timeoutMs=timeoutMs;this.active=false;
  }
  async once(signal=new AbortController().signal) {
    need(!this.active,'refund worker already active');this.active=true;let identity=null;
    try {
      const deadline=AbortSignal.any([signal,AbortSignal.timeout(this.timeoutMs)]);
      const result=await this.api.post('pull',{rails:Object.keys(this.adapters).sort()},deadline),work=result.work;
      if(work===null)return {outcome:'idle'};
      const adapter=this.adapters[work?.authorization?.body?.destination?.rail];need(adapter,'refund rail has no configured custody');
      // Adapter validates signatures and immutable evidence before local state
      // or any payment request. No generic arbitrary-URL execution is accepted.
      const preparation=await adapter.prepare(work);
      identity={refund_id:work.refund_id,lease_token:work.lease_token};
      const grant=await this.api.post('dispatch',{...identity,preparation},deadline);
      const delivery=await adapter.execute(work,grant,deadline);
      const completed=await this.api.post('delivered',{...identity,delivery},deadline);
      need(completed.accepted===true,'refund completion not accepted');
      return {outcome:'delivered',refund_id:work.refund_id};
    } catch(error) {
      if(!identity||signal.aborted)throw error;
      const review=error instanceof ReviewWork;
      const code=review?error.reason:error instanceof RetryWork?error.code:'refund_execution_unavailable';
      need(typeof code==='string'&&/^[a-z0-9_]{1,100}$/.test(code),'invalid refund recovery code');
      const delayMs=error instanceof RetryWork?Math.max(1000,Math.min(3600000,error.delaySeconds*1000)):30000;
      // A failed defer/expired lease leaves the retained operation for the next
      // owner. It never releases money, changes the request, or sends again here.
      await this.api.post('defer',{...identity,reason:code,delay_ms:delayMs,review},AbortSignal.any([signal,AbortSignal.timeout(5000)]));
      return {outcome:review?'review':'retry',refund_id:identity.refund_id,code};
    } finally {this.active=false;}
  }
}
function privateJson(file) { const b=readCustodyFile(file,{max:16384});try{return JSON.parse(b.toString('utf8'));}finally{b.fill(0);} }
function privateText(file) { const b=readCustodyFile(file,{max:4096});try{return b.toString('utf8').trim();}finally{b.fill(0);} }
export async function main(env=process.env) {
  need(env.PROXY_ADMISSION_REFUND_WORKER_ENABLED==='1'&&env.PROXY_ADMISSION_REFUND_WORKER_CONFIG,'refund worker disabled');
  const c=privateJson(env.PROXY_ADMISSION_REFUND_WORKER_CONFIG);
  shape(c,['api_origin','api_credential_file','policy_file','executor_key_file','executor_password_file','journal_root','stripe','timeout_ms','poll_ms','mode','allow_loopback_http']);
  shape(c.stripe,['account','livemode','currency','credential_file','retry_window_ms','max_lookup_pages']);
  need(['once','watch'].includes(c.mode)&&uint(c.poll_ms,1000)&&c.poll_ms<=60000&&typeof c.allow_loopback_http==='boolean','invalid refund worker mode');
  const policy=privateJson(c.policy_file);need(policy.enabled===true&&Array.isArray(policy.rails)&&policy.rails.includes('fiat'),'this entry point currently supports FIAT returns only');
  const password=readCustodyFile(c.executor_password_file,{max:4096}),pem=readCustodyFile(c.executor_key_file,{max:16384});let key;
  try {need(pem.toString('ascii',0,40).startsWith('-----BEGIN ENCRYPTED PRIVATE KEY-----'),'encrypted executor key required');key=createPrivateKey({key:pem,format:'pem',passphrase:password});}
  finally {password.fill(0);pem.fill(0);}
  const api=new RefundApi({origin:c.api_origin,credential:privateText(c.api_credential_file),allowLoopbackHttp:c.allow_loopback_http});
  const adapter=new StripeAdmissionRefund({account:c.stripe.account,livemode:c.stripe.livemode,currency:c.stripe.currency,credential:privateText(c.stripe.credential_file),
    policy,key,journalRoot:c.journal_root,retryWindowMs:c.stripe.retry_window_ms,maxLookupPages:c.stripe.max_lookup_pages});
  const worker=new AdmissionRefundWorker({api,adapters:{fiat:adapter},timeoutMs:c.timeout_ms}),stop=new AbortController();
  const shutdown=()=>stop.abort();process.once('SIGINT',shutdown);process.once('SIGTERM',shutdown);
  try {do {
    try { process.stdout.write(JSON.stringify(await worker.once(stop.signal))+'\n'); }
    catch {if(stop.signal.aborted)break;process.stdout.write(JSON.stringify({outcome:'retry',code:'refund_worker_unavailable'})+'\n');}
    if(c.mode==='once')break;
    await sleep(c.poll_ms,undefined,{signal:stop.signal}).catch(()=>{});
  } while(!stop.signal.aborted);}finally{process.removeListener('SIGINT',shutdown);process.removeListener('SIGTERM',shutdown);}
}
if(process.argv[1]&&path.resolve(process.argv[1])===fileURLToPath(import.meta.url)) {
  main().catch(()=>{process.stderr.write('Admission refund worker configuration rejected; inspect protected local configuration.\n');process.exitCode=1;});
}

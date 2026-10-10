// Separate, explicitly enabled TAP return custody. Admission discovery keeps its
// read-only RPC allowlist; only this worker may broadcast a retained transaction.
import {createPrivateKey} from 'node:crypto';
import {tapRefundSigner} from './proxy-admission-refund-tap-transaction.mjs';
import {readCustodyFile} from './proxy-admission-custody.mjs';
import {boundedJson} from './proxy-admission-worker.mjs';
import {need,shape,uint} from './proxy-admission-wire.mjs';
import {RetryWork,parseHexInt} from './retail-crypto-verification.mjs';
import {TapAdmissionRefund} from './proxy-admission-refund-tap.mjs';

export function readRefundTapSigner({key_file,password_file,receiver}) {
 const raw=readCustodyFile(key_file,{max:16384}),password=readCustodyFile(password_file,{max:4096});let secret;
 try {
  need(raw.toString('ascii',0,40).startsWith('-----BEGIN ENCRYPTED PRIVATE KEY-----'),'encrypted TAP PKCS8 required');
  const key=createPrivateKey({key:raw,format:'pem',passphrase:password});
  need(key.asymmetricKeyType==='ec'&&key.asymmetricKeyDetails?.namedCurve==='secp256k1','TAP custody key must be secp256k1');
  secret=Buffer.from(key.export({format:'jwk'}).d,'base64url');
  const signer=tapRefundSigner(secret);
  need(signer.address.toLowerCase()===receiver,'TAP configured receiver differs from custody key');
  return signer;
 } finally {raw.fill(0);password.fill(0);secret?.fill(0);}
}

export function refundTapRpc({urls,chainId,allowLoopbackHttp=false,fetcher=fetch}) {
 need(Array.isArray(urls)&&urls.length>0&&urls.length<=4&&uint(chainId,1),'TAP return RPC configuration required');
 const endpoints=urls.map(raw=>{need(typeof raw==='string'&&raw.length<=4096,'invalid operator RPC URL');const u=new URL(raw);
  need(!u.username&&!u.password&&!u.search&&!u.hash&&(u.protocol==='https:'||allowLoopbackHttp&&u.protocol==='http:'&&['127.0.0.1','[::1]'].includes(u.hostname)),
   'invalid operator RPC URL');return u.toString();});
 const methods=new Set(['eth_getBlockByNumber','eth_getTransactionReceipt','eth_getTransactionByHash','eth_getTransactionCount',
  'eth_call','eth_estimateGas','eth_getBalance','eth_sendRawTransaction']);let id=0;
 const call=async(url,method,params,signal)=>{
  const requestId=++id;
  const r=await boundedJson(url,{body:{jsonrpc:'2.0',id:requestId,method,params},signal,fetcher,maxBytes:262144});
  need(r?.jsonrpc==='2.0'&&r.id===requestId&&!r.error&&Object.hasOwn(r,'result'),'TAP return RPC unavailable');return r.result;
 };
 return async(method,params,signal)=>{
  need(methods.has(method)&&Array.isArray(params)&&Buffer.byteLength(JSON.stringify(params))<=8192,'TAP return RPC method/parameters rejected');
  if(method==='eth_sendRawTransaction')need(params.length===1&&typeof params[0]==='string'&&/^0x(?:[0-9a-f]{2}){1,2048}$/.test(params[0]),'signed TAP transaction required');
  signal.throwIfAborted();
  for(const url of endpoints){
   let sendAttempted=false;
   try {
    const attempt=AbortSignal.any([signal,AbortSignal.timeout(4000)]);
    need(parseHexInt(await call(url,'eth_chainId',[],attempt),'chain')===BigInt(chainId),'TAP chain differs');
    sendAttempted=method==='eth_sendRawTransaction';
    const result=await call(url,method,params,attempt);signal.throwIfAborted();return result;
   }catch{if(signal.aborted)throw signal.reason;}
   // A send may have succeeded despite a lost ACK. Return to exact-hash
   // reconciliation instead of blindly broadcasting through another endpoint.
   if(sendAttempted)break;
  }
  throw new RetryWork('refund_rpc_unavailable',15);
 };
}

export function openTapRefundRuntime(c,{policy,key,journalRoot,allowLoopbackHttp=false}) {
 shape(c,['chain_id','token_contract','receiver','key_file','password_file','rpc_urls_file','max_gas','max_fee_per_gas','priority_fee','max_fee']);
 const raw=readCustodyFile(c.rpc_urls_file,{max:16384});let urls;
 try {urls=JSON.parse(raw.toString('utf8'));}finally{raw.fill(0);}
 const rpc=refundTapRpc({urls,chainId:c.chain_id,allowLoopbackHttp});
 const signer=readRefundTapSigner(c);
 return {adapter:new TapAdmissionRefund({chainId:c.chain_id,token:c.token_contract,receiver:c.receiver,signer,rpc,policy,key,journalRoot,
  maxGas:c.max_gas,maxFeePerGas:c.max_fee_per_gas,priorityFee:c.priority_fee,maxFee:c.max_fee})};
}

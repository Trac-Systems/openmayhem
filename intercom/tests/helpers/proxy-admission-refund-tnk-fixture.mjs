// Actual Core worker to actual local SITE HTTP; actual signed Hyperbee receipt,
// synthetic validator transport only. No DHT, external wallet or credentials.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {createPrivateKey} from 'node:crypto';
import {TnkAdmissionRefund} from '../../scripts/proxy-admission-refund-tnk.mjs';
import {readRefundTnkWallet} from '../../scripts/proxy-admission-refund-tnk-runtime.mjs';
import {AdmissionRefundWorker,RefundApi} from '../../scripts/proxy-admission-refund-worker.mjs';
import {tnkRefundRuntime} from './proxy-admission-refund-tnk-runtime.mjs';
const chunks=[];let size=0;for await(const c of process.stdin){size+=c.length;if(size>32768)throw Error('fixture input too large');chunks.push(c);}
const input=JSON.parse(Buffer.concat(chunks).toString('utf8')),origin=new URL(input.origin);
if(origin.hostname!=='127.0.0.1'||origin.protocol!=='http:'||origin.username||origin.password||origin.pathname!=='/')throw Error('loopback fixture required');
const root=fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(),'refund-tnk-http-')));fs.chmodSync(root,0o700);let runtime,wallet;
try {
 const api=new RefundApi({origin:origin.origin,credential:input.credential,allowLoopbackHttp:true});
 const {work}=await api.post('pull',{rails:['tnk']},AbortSignal.timeout(5000));
 const keyFile=path.join(root,'wallet.pem'),passwordFile=path.join(root,'passphrase');
 const password=Buffer.from('public-ephemeral-test-passphrase');
 fs.writeFileSync(keyFile,createPrivateKey(input.custody).export({format:'pem',type:'pkcs8',cipher:'aes-256-cbc',passphrase:password}),{mode:0o600});
 fs.writeFileSync(passwordFile,password,{mode:0o600});
 wallet=await readRefundTnkWallet({format:'encrypted_pkcs8',key_file:keyFile,password_file:passwordFile,receiver:work.invoice.collection.destination,network_name:'testnet1'});
 runtime=await tnkRefundRuntime({wallet,network:work.invoice.network});
 const adapter=new TnkAdmissionRefund({msb:runtime.msb,network:work.invoice.network,networkName:'testnet1',canonicalFrontier:runtime.frontier,
  policy:input.policy,key:createPrivateKey(input.key),journalRoot:root,finality:2,timeoutSeconds:1});
 let proof,done,preparation,grant;const actualPost=api.post.bind(api);
 api.post=async(action,body,signal)=>{
  if(action==='pull')return {work};
  const result=await actualPost(action,body,signal);
  if(action==='dispatch'){preparation=body.preparation;grant=result;}
  if(action==='delivered'){proof=body.delivery;done=result;}return result;
 };
 const outcome=await new AdmissionRefundWorker({api,adapters:{tnk:adapter},timeoutMs:10000}).once();
 if(outcome.outcome!=='delivered')throw Error(`worker outcome ${outcome.outcome}`);
 await runtime.view.put('grew-after-completion',Buffer.from('metadata'));
 const replayProof=await adapter.execute({...work,action:'reconcile',preparation,first_dispatch_at_ms:grant.first_dispatch_at_ms},{...grant,action:'reconcile'},AbortSignal.timeout(5000));
 if(JSON.stringify(replayProof)!==JSON.stringify(proof))throw Error('completion changed after chain growth');
 const replay=await actualPost('delivered',{refund_id:work.refund_id,lease_token:work.lease_token,delivery:replayProof},AbortSignal.timeout(5000));
 console.log(JSON.stringify({broadcasts:runtime.state.broadcasts.length,scans:runtime.state.scans,transaction_hash:proof.body.receipt.transaction_hash,done,replay}));
}finally {await runtime?.close();wallet?.secretKey?.fill(0);fs.rmSync(root,{recursive:true,force:true});}

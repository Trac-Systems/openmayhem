// Local acceptance driver only. Real canonical service/signatures/publication;
// synthetic TAP RPC receipts. SITE queue/persistence lives in the calling test.
import fs from 'node:fs';
import http from 'node:http';
import { randomBytes, randomUUID } from 'node:crypto';
import { createInterface } from 'node:readline';
import { familyAdminFixture } from './proxy-family-admin-fixture.mjs';
import { createTnkDiscoveryFixture, testTnkAddress } from './proxy-admission-tnk-fixture.mjs';
import { createAdmissionMsbReader } from '../../features/mayhem/proxy-admission-msb.js';
import MayhemFeature from '../../features/mayhem/index.js';
import { requestProxyAdmissionPolicy } from '../../src/rpc.js';
import { proxyOperationDigest, proxyRegistryFeatureKey } from '../../contract/proxy-protocol.js';
import { AdmissionApi, AdmissionWorker, fixedOrigin, tapVerifier } from '../../scripts/proxy-admission-worker.mjs';
import { invoiceCommitment } from '../../scripts/proxy-admission-wire.mjs';
import { ERC20_TRANSFER_TOPIC, addressTopic } from '../../scripts/retail-crypto-verification.mjs';
const cleanup=[];
const f=await familyAdminFixture({after:fn=>cleanup.push(fn)});
const baseFixture=JSON.parse(fs.readFileSync(new URL('../fixtures/proxy-admission-worker-v1.json',import.meta.url))).cases[0];
const h=()=>randomBytes(32).toString('hex'),now=Date.now(),invoiceId=randomUUID();
const delayed=process.env.PROXY_ADMISSION_DELAYED_FIXTURE==='1';
const msbFixture=await createTnkDiscoveryFixture({hash:h(),destination:testTnkAddress(h()),
 networkId:f.network.network_id,msbBootstrap:f.network.msb_bootstrap});
cleanup.push(()=>msbFixture.close());
f.peer.proxyAdmissionMsbSnapshot=createAdmissionMsbReader(msbFixture.msb);
const envelope=await f.create();
const invoice={...baseFixture.verify_work.invoice,network:f.network,provider_pubkey:f.provider.publicKey,issuer_pubkey:f.issuer.publicKey,
 entitlement_id:h(),initial_operation_digest:await proxyOperationDigest(envelope.intent),fee_policy_hash:f.config.fee_policy_hash,
 created_at_ms:now-120000,quote_expires_at_ms:delayed?now-10000:now+120000,collection:{...baseFixture.verify_work.invoice.collection,allocation_id:h()}};
invoice.invoice_commitment=await invoiceCommitment(invoiceId,invoice);
const reference={...baseFixture.verify_work.payment_reference,transaction_hash:'0x'+h()};
const references=[reference,{...reference,log_index:reference.log_index+1}];
const observedAt=now-1000,paidAt=Math.floor((now-(delayed?20000:2000))/1000)*1000,blockHash='0x'+h();
let amounts=['400000000000000000','700000000000000000'];
const peer={...f.peer,wallet:{...f.peer.wallet,publicKey:f.issuer.publicKey,sign:bytes=>f.issuer.wallet.sign(bytes).toString('hex')},base:{writable:false,view:f.base.view}};
const client=new MayhemFeature(peer,{});cleanup.push(()=>client.stop());
let policyReads=0,tapReads=0;
client.requestService=async(service,body)=>{
 const authorization=f.feature._verifyServiceRequest(service,body,{admin:f.admin.publicKey,transport:f.issuer.publicKey});
 if(!authorization)throw new Error('Fixture canonical authentication failed');
 policyReads++;return { ...await f.feature._handleService(service,authorization.payload,authorization),
   relayed:true,request_id:'a'.repeat(64) };
};
peer.protocol={instance:{features:{mayhem:client}}};
const server=http.createServer(async(req,res)=>{
 const reply=(value,status=200)=>{res.writeHead(status,{'content-type':'application/json','cache-control':'no-store'});res.end(JSON.stringify(value));};
 try {
  if(req.method!=='POST')throw new Error('Only fixture POST supported');
  let raw='';for await(const chunk of req){raw+=chunk;if(Buffer.byteLength(raw)>16384)throw new Error('Fixture body bound');}
  const body=JSON.parse(raw);
  if(req.url==='/v1/proxy/admission-policy')return reply(await requestProxyAdmissionPolicy(peer,body));
  if(req.url!=='/')throw new Error('Unknown fixture route');
  tapReads++;let result;
  if(body.method==='eth_chainId')result='0x7a69';
  else if(body.method==='eth_getTransactionReceipt'){
   if(body.params[0]!==reference.transaction_hash)throw new Error('Wrong synthetic transfer');
   result={transactionHash:reference.transaction_hash,status:'0x1',blockNumber:'0x64',blockHash,
    logs:references.map((ref,i)=>({address:ref.token_contract,topics:[ERC20_TRANSFER_TOPIC,addressTopic(baseFixture.evidence_completion.receipt.from_address),addressTopic(invoice.collection.destination)],
     data:'0x'+BigInt(amounts[i]).toString(16),logIndex:'0x'+ref.log_index.toString(16)}))};
  }else if(body.method==='eth_getBlockByNumber')result=body.params[0]==='finalized'?{number:'0x70'}:{number:'0x64',hash:blockHash,timestamp:'0x'+(BigInt(paidAt)/1000n).toString(16)};
  else throw new Error('Unsupported synthetic RPC method');
  reply({jsonrpc:'2.0',id:body.id,result});
 }catch{reply({fixture_error:'rejected'},400);}
});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
const coreOrigin=`http://127.0.0.1:${server.address().port}`;
const nativeBalance=(await f.base.view.get('bal/existing-customer')).value,nativePayout=(await f.base.view.get('payout/epoch/542')).value;
let configured=null,lastCompletion=null;
async function run(command){
 if(command.action==='configure_many_topups'){
  if(policyReads!==0||tapReads!==0||f.calls!==0)throw new Error('Fixture already used');
  // 37 partial payments fund one original fee; the 38th is later excess.
  references.splice(0,references.length,...Array.from({length:38},(_,i)=>({...reference,log_index:100-i})));
  amounts=Array.from({length:38},(_,i)=>i<36?'10000000000000000':i===36?'640000000000000000':'100000000000000000');
  return {payment_references:references};
 }
 if(command.action==='advance_epoch'){
  const previous=(await f.base.view.get('epoch/apply/state')).value;
  const current=previous.updated_epoch??previous.epoch;
  if(!Number.isSafeInteger(command.epoch)||command.epoch<=current)throw new Error('Fixture epoch must advance');
  await f.base.append({type:'seed',entries:[['epoch/apply/state',{...previous,updated_epoch:command.epoch}]]});await f.base.update();
  return {epoch:command.epoch};
 }
 if(command.action==='inspect'){
  return {policy_reads:policyReads,canonical_appends:f.calls,pending:f.journal.list().length,model_calls:0,live_payments:0};
 }
 if(command.action==='configure'){
  if(configured)throw new Error('Fixture already configured');
  const siteOrigin=fixedOrigin(command.site_origin,{allowLoopbackHttp:true});
  if(!new URL(siteOrigin).hostname.match(/^(127\.0\.0\.1|\[::1\])$/))throw new Error('SITE fixture must be loopback');
  if(command.verifier_credential===command.issuer_credential)throw new Error('Separate fixture credentials required');
  configured={siteOrigin,verify:command.verifier_credential,issue:command.issuer_credential};return {configured:true};
 }
 if(command.action==='run'){
  if(!configured||!['verify','issue'].includes(command.phase))throw new Error('Invalid worker command');
  const phase=command.phase;let drop=command.drop_ack===true;lastCompletion=null;
  const fetcher=async(url,options)=>{
   const response=await fetch(url,options);
   if((url===`${configured.siteOrigin}/internal/proxy-admission-worker/${phase==='verify'?'evidence':'permit'}`
    ||phase==='issue'&&url===`${configured.siteOrigin}/internal/proxy-admission-worker/evidence-progress`)&&response.ok){
    lastCompletion=JSON.parse(options.body);
    if(drop){drop=false;await response.arrayBuffer();throw new Error('Synthetic lost completion ACK after real commit');}
   }
   return response;
  };
  const api=new AdmissionApi({origin:configured.siteOrigin,credential:configured[phase],phase,allowLoopbackHttp:true,fetcher});
  const worker=new AdmissionWorker({phase,api,coreOrigin,network:f.network,feePolicyHash:invoice.fee_policy_hash,issuerPubkey:invoice.issuer_pubkey,
   rails:['tap'],allowLoopbackHttp:true,fetcher,
   verifyReceipt:phase==='verify'?tapVerifier({origin:coreOrigin,chainId:31337,tokenContract:reference.token_contract,allowLoopbackHttp:true}):null,
   signPermit:phase==='issue'?bytes=>f.issuer.wallet.sign(bytes).toString('hex'):null});
  return {result:await worker.runOnce(),completion:lastCompletion,policy_reads:policyReads,tap_reads:tapReads};
 }
 if(command.action==='publish'){
  envelope.admission={permit:command.permit,issuer_signature:command.issuer_signature};
  const key=await proxyRegistryFeatureKey(envelope),result=await f.controller.submit(key,envelope);
  const balance=(await f.base.view.get('bal/existing-customer')).value,payout=(await f.base.view.get('payout/epoch/542')).value;
  if(JSON.stringify(balance)!==JSON.stringify(nativeBalance)||JSON.stringify(payout)!==JSON.stringify(nativePayout))throw new Error('Native fixture funds changed');
  return {result,canonical_appends:f.calls,pending:f.journal.list().length,native_balance:balance,native_payout:payout,policy_reads:policyReads,model_calls:0,live_payments:0};
 }
 throw new Error('Unknown fixture command');
}
const lines=createInterface({input:process.stdin,crlfDelay:Infinity});
process.stdout.write(JSON.stringify({ready:true,core_origin:coreOrigin,core_requester:f.issuer.publicKey,invoice_id:invoiceId,invoice,payment_references:references,reference_assigned_at_ms:observedAt})+'\n');
try {
 for await(const line of lines){
  if(Buffer.byteLength(line)>32768)throw new Error('Fixture command bound');
  const command=JSON.parse(line);if(command.action==='close')break;
  try{process.stdout.write(JSON.stringify({id:command.id,ok:true,value:await run(command)})+'\n');}
  catch(error){process.stdout.write(JSON.stringify({id:command.id,ok:false,error:String(error.message)})+'\n');}
 }
}finally{
 server.closeAllConnections();await new Promise(resolve=>server.close(resolve));
 for(const stop of cleanup.reverse())await stop();
}

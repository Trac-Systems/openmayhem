import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {randomBytes,generateKeyPairSync,verify} from 'node:crypto';
import {TapAdmissionRefund} from '../scripts/proxy-admission-refund-tap.mjs';
import {tapRefundSigner,inspectTapRefundTransaction,tapTransferData} from '../scripts/proxy-admission-refund-tap-transaction.mjs';
import {readRefundTapSigner,refundTapRpc,openTapRefundRuntime} from '../scripts/proxy-admission-refund-tap-runtime.mjs';
import {RefundJournal} from '../scripts/proxy-admission-refund-common.mjs';
import {AdmissionRefundWorker,main} from '../scripts/proxy-admission-refund-worker.mjs';
import {admissionTapRpc} from '../scripts/proxy-admission-discovery.mjs';
import {ERC20_TRANSFER_TOPIC,addressTopic} from '../scripts/retail-crypto-verification.mjs';
import {proxyCanonicalSigningBytes} from '../contract/proxy-protocol.js';
import {tapRefundWork} from './helpers/proxy-admission-refund-tap-work.mjs';
const signal=()=>AbortSignal.timeout(5000),h=()=>`0x${randomBytes(32).toString('hex')}`;
const q=n=>`0x${BigInt(n).toString(16)}`,word=n=>`0x${BigInt(n).toString(16).padStart(64,'0')}`;
async function fixture(t,shared={}){
 const signer=shared.signer??tapRefundSigner(randomBytes(32)),receiver=signer.address.toLowerCase();
 const f=await tapRefundWork(t,{receiver,...shared});
 const state={calls:[],sends:[],receipt:null,pendingTx:null,nonce:0,pendingNonce:0,finalized:100,balance:100n,eth:10n**18n,baseFee:100n,
  gas:50000n,dropBefore:false,dropAfter:false,mine:true,blockHash:h()};
 const o={chainId:31337,token:f.work.invoice.collection.token_contract,receiver,signer,policy:f.policy,key:f.executor.privateKey,journalRoot:f.root,
  maxGas:'100000',maxFeePerGas:'10000',priorityFee:'10',maxFee:'1000000000'};
 const rpc=async(method,p,abort)=>{
  abort.throwIfAborted();state.calls.push(method);
  if(method==='eth_getTransactionCount')return q(p[1]==='latest'?state.nonce:state.pendingNonce);
  if(method==='eth_getBlockByNumber')return {number:q(p[0]==='finalized'?state.finalized:p[0]==='latest'?100:BigInt(p[0])),hash:state.blockHash,baseFeePerGas:q(state.baseFee)};
  if(method==='eth_estimateGas')return q(state.gas);
  if(method==='eth_call')return word(state.balance);
  if(method==='eth_getBalance')return q(state.eth);
  if(method==='eth_getTransactionReceipt')return structuredClone(state.receipt);
  if(method==='eth_getTransactionByHash')return structuredClone(state.pendingTx);
  assert.equal(method,'eth_sendRawTransaction');state.sends.push(p[0]);
  if(state.dropBefore){state.dropBefore=false;throw Error('simulated loss before acceptance');}
  const tx=inspectTapRefundTransaction(p[0],{...o,from:receiver,to:f.work.authorization.body.destination.address,amount:'10'});
  state.pendingTx={hash:tx.hash,from:receiver,to:o.token,input:tx.data,nonce:q(tx.nonce),value:'0x0'};state.pendingNonce=tx.nonce+1;
  if(state.mine){state.nonce=tx.nonce+1;state.balance-=10n;
   state.receipt={transactionHash:tx.hash,from:receiver,to:o.token,blockNumber:'0x64',blockHash:state.blockHash,status:'0x1',logs:[{
    address:o.token,transactionHash:tx.hash,blockHash:state.blockHash,blockNumber:'0x64',logIndex:'0x0',removed:false,
    topics:[ERC20_TRANSFER_TOPIC,addressTopic(receiver),addressTopic(f.work.authorization.body.destination.address)],data:word(10)}]};}
  if(state.dropAfter){state.dropAfter=false;throw Error('simulated loss after acceptance');}return tx.hash;
 };
 o.rpc=rpc;
 return {...f,options:o,state,signer,adapter:()=>new TapAdmissionRefund(o)};
}
test('signed TAP transaction is durable before dispatch; finalized proof and completion replay never pay twice',async t=>{
 const f=await fixture(t),a=f.adapter(),p=await a.prepare(f.work,signal()),j=new RefundJournal(f.root,f.work.authorization_digest);
 assert.equal(f.state.sends.length,0);assert.equal(j.get('request').transaction_hash,p.body.reference.transaction_hash);
 assert.equal(fs.statSync(j.file('request')).mode&0o777,0o600);
 const delivered=await a.execute(f.work,f.grant,signal());
 assert.equal(delivered.body.receipt.amount_base_units,'10');assert.equal(delivered.body.receipt.from_address,f.options.receiver);
 assert(verify(null,proxyCanonicalSigningBytes('mayhem/proxy/admission-refund-delivery/v1',delivered.body),f.executor.publicKey,Buffer.from(delivered.signature,'hex')));
 f.state.finalized+=100;
 assert.deepEqual(await f.adapter().execute(f.resumed(p),{...f.grant,action:'reconcile'},signal()),delivered);
 assert.equal(f.state.sends.length,1);assert.equal(f.state.balance,90n);
 assert(!f.state.calls.some(m=>m==='eth_getLogs'),'no history scan');
});
test('lost broadcast response before/after acceptance recovers the exact signed transaction',async t=>{
 for(const fault of ['dropBefore','dropAfter']){
  const f=await fixture(t),p=await f.adapter().prepare(f.work,signal());f.state[fault]=true;
  await assert.rejects(f.adapter().execute(f.work,f.grant,signal()),/simulated loss/);
  const done=await f.adapter().execute(f.resumed(p),{...f.grant,action:'reconcile'},signal());
  assert.equal(done.body.receipt.transaction_hash,p.body.reference.transaction_hash);
  assert.equal(f.state.sends.length,fault==='dropBefore'?2:1);assert.equal(new Set(f.state.sends).size,1);assert.equal(f.state.balance,90n);
 }
});
test('mempool and unfinalized receipts defer without resending; mined revert requires review',async t=>{
 for(const mode of ['pending','unfinalized','reverted']){
  const f=await fixture(t),p=await f.adapter().prepare(f.work,signal());
  if(mode==='pending')f.state.mine=false;else f.state.finalized=99;
  await assert.rejects(f.adapter().execute(f.work,f.grant,signal()),e=>['refund_transfer_pending','refund_awaiting_finality'].includes(e.code));
  if(mode==='reverted'){f.state.finalized=100;f.state.receipt.status='0x0';}
  await assert.rejects(f.adapter().execute(f.resumed(p),{...f.grant,action:'reconcile'},signal()),e=>mode==='reverted'?e.reason==='refund_transfer_reverted':['refund_transfer_pending','refund_awaiting_finality'].includes(e.code));
  assert.equal(f.state.sends.length,1);
 }
});
test('different refunds sharing custody cannot claim the same nonce, even across restart',async t=>{
 const f=await fixture(t);await f.adapter().prepare(f.work,signal());
 const g=await fixture(t,{signer:f.signer,root:f.root,policy:f.policy,executor:f.executor,review:f.review});
 await assert.rejects(g.adapter().prepare(g.work,signal()),e=>e.code==='refund_nonce_reserved');
 assert.equal(g.state.sends.length,0);assert.equal(new RefundJournal(g.root,g.work.authorization_digest).get('request'),null);
 await f.adapter().execute(f.work,f.grant,signal());g.state.nonce=1;g.state.pendingNonce=1;
 const p=await g.adapter().prepare(g.work,signal());assert.notEqual(p.body.reference.transaction_hash,new RefundJournal(f.root,f.work.authorization_digest).get('request').transaction_hash);
});
test('concurrent signing race has one durable nonce owner and no replacement transfer',async t=>{
 const f=await fixture(t),g=await fixture(t,{signer:f.signer,root:f.root,policy:f.policy,executor:f.executor,review:f.review});
 const results=await Promise.allSettled([f.adapter().prepare(f.work,signal()),g.adapter().prepare(g.work,signal())]);
 assert.equal(results.filter(r=>r.status==='fulfilled').length,1);assert.equal(f.state.sends.length+g.state.sends.length,0);
 const loser=results[0].status==='rejected'?f:g;
 await assert.rejects(loser.adapter().prepare(loser.work,signal()),e=>e.reason==='refund_nonce_claim_conflict');
});
test('nonce consumed elsewhere, missing journal, wrong sender, ambiguous log, and changed canonical receipt cannot complete',async t=>{
 for(const mode of ['nonce','missing','sender','duplicate','fork','race','blockRace']){
  const f=await fixture(t),p=await f.adapter().prepare(f.work,signal());
  if(mode==='nonce'){f.state.nonce=1;f.state.pendingNonce=1;}
  else if(mode==='missing')fs.unlinkSync(new RefundJournal(f.root,f.work.authorization_digest).file('request'));
  else {
   f.state.finalized=99;await assert.rejects(f.adapter().execute(f.work,f.grant,signal()));f.state.finalized=100;
   if(mode==='sender')f.state.receipt.logs[0].topics[1]=addressTopic('0x'+'3'.repeat(40));
   if(mode==='duplicate')f.state.receipt.logs.push(structuredClone(f.state.receipt.logs[0]));
   if(mode==='fork')f.state.blockHash=h();
   if(mode==='race'){const base=f.options.rpc;let reads=0;f.options.rpc=async(m,p,s)=>{const r=await base(m,p,s);if(m==='eth_getTransactionReceipt'&&++reads===2)r.blockHash=h();return r;};}
   if(mode==='blockRace'){const base=f.options.rpc;let reads=0;f.options.rpc=async(m,p,s)=>{const r=await base(m,p,s);if(m==='eth_getBlockByNumber'&&p[0]==='0x64'&&++reads===2)r.hash=h();return r;};}
  }
  const sends=f.state.sends.length;
  await assert.rejects(f.adapter().execute(f.resumed(p),{...f.grant,action:'reconcile'},signal()));
  assert.equal(f.state.sends.length,sends);assert.equal(new RefundJournal(f.root,f.work.authorization_digest).get('refund'),null);
 }
});
test('custody shortages and fee bounds defer before dispatch; expired lease never broadcasts',async t=>{
 for(const mode of ['token','gas','price','budget','lease']){
  const f=await fixture(t);
  if(mode==='token')f.state.balance=0n;if(mode==='gas')f.state.eth=0n;if(mode==='price')f.state.baseFee=10000n;if(mode==='budget')f.state.gas=1000000n;
  if(mode==='lease')f.work.lease_expires_at_ms=Date.now()-1;
  const actions=[],api={post:async(action,body)=>{actions.push({action,body});if(action==='pull')return {work:f.work};if(action==='defer')return {accepted:true};assert.fail('dispatch must not happen');}};
  const result=await new AdmissionRefundWorker({api,adapters:{tap:f.adapter()},timeoutMs:5000}).once();
  assert.equal(result.outcome,mode==='budget'?'review':'retry');assert.deepEqual(actions.map(x=>x.action),['pull','defer']);assert.equal(f.state.sends.length,0);
 }
});
test('immutable signed transaction rejects altered destination, amount, chain, fee and calldata',async t=>{
 const f=await fixture(t),p=await f.adapter().prepare(f.work,signal()),j=new RefundJournal(f.root,f.work.authorization_digest),stored=j.get('request');
 const expected={...f.options,from:f.options.receiver,to:f.work.authorization.body.destination.address,amount:'10'};
 for(const change of [{to:'0x'+'3'.repeat(40)},{amount:'11'},{chainId:1},{token:'0x'+'4'.repeat(40)},{maxGas:'1'},{maxFee:'1'}])assert.throws(()=>inspectTapRefundTransaction(stored.raw_transaction,{...expected,...change}));
 const raw=await f.signer.signTransaction({type:2,chainId:31337,nonce:0,to:f.options.token,value:0n,data:tapTransferData(expected.to,'11'),gasLimit:60000,maxFeePerGas:210,maxPriorityFeePerGas:10});
 fs.writeFileSync(j.file('request'),JSON.stringify({...stored,raw_transaction:raw}));
 await assert.rejects(f.adapter().execute(f.resumed(p),{...f.grant,action:'reconcile'},signal()));assert.equal(f.state.sends.length,0);
});
test('protected encrypted secp256k1 custody loads exact passphrase; wrong key/mode/receiver rejected',async t=>{
 const f=await fixture(t),k=generateKeyPairSync('ec',{namedCurve:'secp256k1'}),password=randomBytes(32);
 const keyFile=path.join(f.root,'key.pem'),passwordFile=path.join(f.root,'password'),rpcFile=path.join(f.root,'rpc.json');
 fs.writeFileSync(keyFile,k.privateKey.export({format:'pem',type:'pkcs8',cipher:'aes-256-cbc',passphrase:password}),{mode:0o600});
 fs.writeFileSync(passwordFile,password,{mode:0o600});fs.writeFileSync(rpcFile,JSON.stringify(['https://rpc.invalid/operator-credential']),{mode:0o600});
 const expected=tapRefundSigner(Buffer.from(k.privateKey.export({format:'jwk'}).d,'base64url')).address.toLowerCase();
 const c={chain_id:31337,token_contract:f.options.token,receiver:expected,key_file:keyFile,password_file:passwordFile,rpc_urls_file:rpcFile,
  max_gas:'100000',max_fee_per_gas:'10000',priority_fee:'10',max_fee:'1000000000'};
 assert.equal(readRefundTapSigner(c).address.toLowerCase(),expected);
 assert(openTapRefundRuntime(c,{policy:f.policy,key:f.executor.privateKey,journalRoot:f.root}).adapter);
 assert.throws(()=>readRefundTapSigner({...c,receiver:f.options.receiver}));
 fs.chmodSync(keyFile,0o644);assert.throws(()=>readRefundTapSigner(c));fs.chmodSync(keyFile,0o600);
 fs.writeFileSync(passwordFile,Buffer.from('wrong passphrase'));assert.throws(()=>readRefundTapSigner(c));
 await assert.rejects(main({}),/disabled/);
});
test('return RPC validates chains, bounds responses and keeps discovery read-only; send ACK loss never triggers blind fallback',async()=>{
 const calls=[];let mode='fallback';
 const fetcher=async(url,o)=>{
  assert.equal(o.redirect,'error');const b=JSON.parse(o.body);calls.push({url,method:b.method});
  if(mode==='large')return new Response('x'.repeat(262145));
  if(b.method==='eth_chainId')return Response.json({jsonrpc:'2.0',id:b.id,result:url.includes('wrong')?'0x1':'0x7a69'});
  if(mode==='lost')throw Error('secret-bearing upstream error');
  return Response.json({jsonrpc:'2.0',id:b.id,result:null});
 };
 const urls=['https://wrong.invalid/key','https://good.invalid/key'],rpc=refundTapRpc({urls,chainId:31337,fetcher});
 assert.equal(await rpc('eth_getTransactionReceipt',[h()],signal()),null);
 assert.deepEqual(calls.map(c=>c.method),['eth_chainId','eth_chainId','eth_getTransactionReceipt']);
 await assert.rejects(rpc('eth_sendTransaction',[{}],signal()));
 const read=admissionTapRpc({urls,chainId:31337,fetcher});await assert.rejects(read('eth_sendRawTransaction',['0x00'],signal()),/read-only/);
 mode='lost';calls.length=0;
 const sending=refundTapRpc({urls:['https://good.invalid/key','https://fallback.invalid/key'],chainId:31337,fetcher});
 await assert.rejects(sending('eth_sendRawTransaction',['0x00'],signal()),e=>e.code==='refund_rpc_unavailable'&&!e.message.includes('secret'));
 assert.equal(calls.length,2);mode='large';await assert.rejects(sending('eth_getTransactionReceipt',[h()],signal()));
 mode='fallback';calls.length=0;await rpc('eth_sendRawTransaction',['0x00'],signal());
 assert.deepEqual(calls.map(c=>c.method),['eth_chainId','eth_chainId','eth_sendRawTransaction']);
 const abort=new AbortController();abort.abort();calls.length=0;await assert.rejects(sending('eth_getTransactionReceipt',[h()],abort.signal));assert.equal(calls.length,0);
});

import test from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import { AdmissionDiscoveryWorker, admissionTapRpc, tapDiscovery, tnkDiscovery } from '../scripts/proxy-admission-discovery.mjs';
import { ERC20_TRANSFER_TOPIC } from '../scripts/retail-crypto-verification.mjs';

const h='a'.repeat(64),hash='0x'+h,token='0x'+'b'.repeat(40),destination='0x'+'c'.repeat(40);
const stream={rail:'tap',chain_id:31337,token_contract:token};
const work=(extra={})=>({schema_version:1,purpose:'proxy_admission_fee',phase:'verify',stream_id:h,stream,
  from_cursor:'10',from_offset:0,max_positions:16,lease_token:'d'.repeat(64),lease_expires_at_ms:Date.now()+60000,...extra});
const block={number:'0xa',hash,parentHash:'0x'+'9'.repeat(64),timestamp:'0x64'};
const logs=Array.from({length:21},(_,i)=>({address:token,blockHash:hash,blockNumber:'0xa',removed:false,
  topics:[ERC20_TRANSFER_TOPIC,'0x'+'0'.repeat(64),'0x'+'0'.repeat(24)+destination.slice(2)],transactionHash:'0x'+i.toString(16).padStart(64,'0'),logIndex:'0x'+i.toString(16)}));

test('TAP uses operator credential paths, explicit fallback and finalized block-bound dense pages',async()=>{
  const requests=[];
  const server=http.createServer(async(req,res)=>{let text='';for await(const chunk of req)text+=chunk;const v=JSON.parse(text);requests.push({path:req.url,method:v.method});
    if(req.url==='/primary'){res.writeHead(503);res.end();return;}
    res.setHeader('content-type','application/json');res.end(JSON.stringify({jsonrpc:'2.0',id:v.id,result:v.method==='eth_chainId'?'0x7a69':v.method==='eth_getLogs'?logs:block}));});
  await new Promise(r=>server.listen(0,'127.0.0.1',r));
  try {
    const origin=`http://127.0.0.1:${server.address().port}`;
    const rpc=admissionTapRpc({urls:[origin+'/primary',origin+'/operator-credential-path/'],chainId:31337,allowLoopbackHttp:true});
    const scan=tapDiscovery({rpc,chainId:31337,tokenContract:token});
    const first=await scan(work(),AbortSignal.timeout(3000));assert.equal(first.next_cursor,'10');assert.equal(first.next_offset,16);assert.equal(first.observations.length,16);
    const second=await scan(work({from_offset:16}),AbortSignal.timeout(3000));assert.equal(second.next_cursor,'11');assert.equal(second.next_offset,0);assert.equal(second.observations.length,5);
    assert.equal(new Set([...first.observations,...second.observations].map(x=>x.transaction_hash)).size,21);
    assert.equal(first.coverage.block_hash,hash);assert.equal(first.coverage.log_count,21);
    assert.equal(second.coverage.block_time_ms,100000);assert.equal(second.coverage.block_number,'10');
    assert.equal(first.observations[0].observed_at_ms,100000);assert.equal(first.observations[0].destination,destination);
    assert(requests.some(r=>r.path==='/operator-credential-path/'));await assert.rejects(rpc('eth_sendRawTransaction',[],AbortSignal.timeout(1000)));
  }finally{await new Promise(r=>server.close(r));}
});
test('TAP rejects wrong chain, non-canonical logs and missing finality without advancing',async()=>{
  const bad=admissionTapRpc({urls:['https://fixture.invalid/secret/'],chainId:31337,fetcher:async(_u,o)=>new Response(JSON.stringify({jsonrpc:'2.0',id:JSON.parse(o.body).id,result:'0x1'}))});
  await assert.rejects(bad('eth_getLogs',[],AbortSignal.timeout(1000)));
  for(const data of [null,{...block,hash:'invalid'}]){
    const scan=tapDiscovery({rpc:async()=>data,chainId:31337,tokenContract:token});await assert.rejects(scan(work(),AbortSignal.timeout(1000)));
  }
  const scan=tapDiscovery({rpc:async(method)=>method==='eth_getLogs'?[{...logs[0],removed:true}]:block,chainId:31337,tokenContract:token});
  await assert.rejects(scan(work(),AbortSignal.timeout(1000)));
  const missingParent=tapDiscovery({rpc:async()=>({...block,parentHash:undefined}),chainId:31337,tokenContract:token});
  await assert.rejects(missingParent(work(),AbortSignal.timeout(1000)));
});
test('TAP empty blocks carry finalized coverage; being ahead of finality cannot fabricate coverage',async()=>{
  let logReads=0;
  const scan=tapDiscovery({rpc:async(method)=>{if(method==='eth_getLogs'){logReads++;return [];}return block;},chainId:31337,tokenContract:token});
  const empty=await scan(work(),AbortSignal.timeout(1000));
  assert.equal(empty.coverage.log_count,0);assert.equal(empty.coverage.parent_hash,block.parentHash);
  assert.equal(empty.next_cursor,'11');assert.equal(empty.next_offset,0);
  const ahead=await scan(work({from_cursor:'11'}),AbortSignal.timeout(1000));
  assert.deepEqual(ahead,{next_cursor:'11',next_offset:0,observations:[]});assert.equal(logReads,1);
});
test('TNK shares one bounded signed page, requires canonical catchup and rejects missing transaction records',async()=>{
  const calls=[],s={rail:'tnk',network:'testnet1',msb_bootstrap:h};
  const msb={state:{getSignedLength:()=>1000},getTxHashes:async(start,end)=>{calls.push([start,end]);return {hashes:[{hash:h,confirmed_length:12}]};},
    getTxDetails:async()=>({address:'testtrac1sender',tro:{to:'testtrac1receiver',am:'1000000000000000000'}})};
  const scan=tnkDiscovery({msb,network:s.network,msbBootstrap:h,frontier:async()=>1000,now:()=>100000});
  const page=await scan(work({stream:s}),AbortSignal.timeout(1000));assert.deepEqual(calls,[[10,26]]);assert.equal(page.next_cursor,'26');assert.equal(page.observations.length,1);assert.equal(page.observations[0].position,'12');
  msb.state.getSignedLength=()=>9;await assert.rejects(scan(work({stream:s}),AbortSignal.timeout(1000)));assert.equal(calls.length,1);
  msb.state.getSignedLength=()=>1000;msb.getTxDetails=async()=>null;await assert.rejects(scan(work({stream:s}),AbortSignal.timeout(1000)));
});
test('discovery worker idle makes no chain read and a lost completion is never replaced by a guessed cursor',async()=>{
  let calls=0,scanCalls=0,posted;
  const w=new AdmissionDiscoveryWorker({origin:'https://fixture.invalid',credential:'public-fixture-only',stream,
    scan:async()=>{scanCalls++;return {next_cursor:'11',next_offset:0,observations:[]};},
    fetcher:async(url,o)=>{calls++;if(url.endsWith('/pull'))return new Response(JSON.stringify({schema_version:1,purpose:'proxy_admission_fee',phase:'verify',work:calls===1?null:work()}));posted=JSON.parse(o.body);throw Error('lost ACK');}});
  assert.equal((await w.runOnce()).status,'idle');assert.equal(scanCalls,0);
  await assert.rejects(w.runOnce());assert.equal(posted.from_cursor,'10');assert.equal(posted.next_cursor,'11');assert.equal(calls,3);
});
test('minimum lease remains usable after network transit and expired leases cannot scan',async()=>{
  let scans=0,remaining=14500;
  const w=new AdmissionDiscoveryWorker({origin:'https://fixture.invalid',credential:'public-fixture-only',stream,
    scan:async(_w,signal)=>{assert(!signal.aborted);scans++;return {next_cursor:'11',next_offset:0,observations:[]};},
    fetcher:async(url)=>new Response(JSON.stringify(url.endsWith('/pull')
      ?{schema_version:1,purpose:'proxy_admission_fee',phase:'verify',work:work({lease_expires_at_ms:Date.now()+remaining})}
      :{accepted:true,purpose:'proxy_admission_fee',phase:'verify'}))});
  assert.equal((await w.runOnce()).status,'observed');assert.equal(scans,1);
  remaining=-1;await assert.rejects(w.runOnce(),/lease too short/);assert.equal(scans,1);
});

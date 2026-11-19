import test from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import { AdmissionDiscoveryWorker, admissionTapRpc, tapDiscovery, tnkDiscovery } from '../scripts/proxy-admission-discovery.mjs';
import { ERC20_TRANSFER_TOPIC } from '../scripts/retail-crypto-verification.mjs';
import { createTnkDiscoveryFixture, testTnkAddress } from './helpers/proxy-admission-tnk-fixture.mjs';

const h='a'.repeat(64),hash='0x'+h,token='0x'+'b'.repeat(40),destination='0x'+'c'.repeat(40);
const stream={rail:'tap',chain_id:31337,token_contract:token};
const work=(extra={})=>({schema_version:1,purpose:'proxy_admission_fee',phase:'verify',stream_id:h,stream,
  from_cursor:'10',from_offset:0,max_positions:16,lease_token:'d'.repeat(64),lease_expires_at_ms:Date.now()+60000,...extra});
const block={number:'0xa',hash,parentHash:'0x'+'9'.repeat(64),timestamp:'0x64'};
const logs=Array.from({length:21},(_,i)=>({address:token,blockHash:hash,blockNumber:'0xa',removed:false,
  topics:[ERC20_TRANSFER_TOPIC,'0x'+'0'.repeat(64),'0x'+'0'.repeat(24)+destination.slice(2)],transactionHash:'0x'+i.toString(16).padStart(64,'0'),logIndex:'0x'+i.toString(16)}));

// Drive the runner's one-second ticks against a SITE-owned cursor and count
// actual JSON-RPC request bodies, including the adapter's identity checks.
function tapPollingFixture(from, finalized) {
  const state = {now: 100000, from: String(from), offset: 0, finalized, calls: [], pages: []};
  const blockHash = n => '0x' + n.toString(16).padStart(64, '0');
  const rpc = admissionTapRpc({urls: ['https://rpc.invalid/'], chainId: stream.chain_id,
    fetcher: async (_url, {body}) => {
      const {id, method, params} = JSON.parse(body);
      state.calls.push(method);
      let result;
      if (method === 'eth_chainId') result = '0x7a69';
      else if (method === 'eth_getBlockByNumber') {
        const n = params[0] === 'finalized' ? state.finalized : Number(params[0]);
        assert(n <= state.finalized);
        result = {...block, number: '0x' + n.toString(16), hash: blockHash(n), parentHash: blockHash(n-1)};
      } else {
        assert.equal(method, 'eth_getLogs');
        assert.deepEqual(params, [{blockHash: blockHash(Number(state.from)), address: token, topics: [ERC20_TRANSFER_TOPIC]}]);
        result = state.from === '10' ? logs.map(log => ({...log, blockHash: blockHash(10)})) : [];
      }
      return Response.json({jsonrpc: '2.0', id, result});
    }});
  const worker = new AdmissionDiscoveryWorker({origin: 'https://site.invalid', credential: 'public-fixture-only', stream,
    now: () => state.now, scan: tapDiscovery({rpc, chainId: stream.chain_id, tokenContract: token}),
    fetcher: async (url, {body}) => {
      const envelope = {schema_version: 1, purpose: 'proxy_admission_fee', phase: 'verify'};
      if (url.endsWith('/pull')) return Response.json({...envelope, work: work({from_cursor: state.from,
        from_offset: state.offset, lease_expires_at_ms: state.now + 15000})});
      assert(url.endsWith('/complete'));
      const page = JSON.parse(body);
      assert.equal(page.from_cursor, state.from);
      assert.equal(page.from_offset, state.offset);
      state.pages.push(page);
      state.from = page.next_cursor;
      state.offset = page.next_offset;
      return Response.json({...envelope, accepted: true});
    }});
  state.tick = async () => { await worker.runOnce(); state.now += 1000; };
  return state;
}

test('TAP polling cost over sixty caught-up ticks and subsequent finality advance', async () => {
  const f = tapPollingFixture(11, 10);
  for (let tick = 0; tick < 60; tick++) await f.tick();
  assert.deepEqual(f.calls, ['eth_chainId', 'eth_getBlockByNumber', 'eth_chainId', 'eth_getBlockByNumber']);
  assert(f.pages.every(p => !p.coverage && p.next_cursor === '11' && p.next_offset === 0 && !p.observations.length));
  assert.equal(f.pages.length, 2);
  f.finalized = 11;
  await f.tick();
  assert.equal(f.from, '12');
  assert.equal(f.pages.at(-1).coverage.finalized_number, '11');
  // SITE's next lease after processing a block is due in one second. Its
  // first empty page starts the server's idle interval as well as our own.
  const afterTip = f.calls.length;
  await f.tick();
  assert.equal(f.pages.at(-1).coverage, undefined);
  assert.equal(f.pages.at(-1).next_cursor, '12');
  assert.deepEqual(f.calls.slice(afterTip), ['eth_chainId', 'eth_getBlockByNumber']);
  await f.tick();
  assert.equal(f.calls.length, afterTip + 2);
});

test('TAP polling cost for empty and dense blocks during bounded catch-up', async () => {
  const f = tapPollingFixture(9, 11);
  const costs = [];
  for (let tick = 0; tick < 4; tick++) {
    const before = f.calls.length;
    await f.tick();
    costs.push(f.calls.length - before);
  }
  assert.deepEqual(f.pages.map(p => [p.next_cursor, p.next_offset, p.observations.length]),
    [['10', 0, 0], ['10', 16, 16], ['11', 0, 5], ['12', 0, 0]]);
  assert.deepEqual(costs, [4, 4, 4, 3]);
  assert.equal(f.calls.filter(m => m === 'eth_chainId').length, 4);
});

test('TAP failover checks each URL and rebuilds the page; the next lease checks identity again', async () => {
  const urls = ['https://rpc.invalid/primary/', 'https://rpc.invalid/other-chain/', 'https://rpc.invalid/fallback/'];
  const fallbackHash = '0x' + 'f'.repeat(64);
  const requests = [];
  let chainChanged = false;
  const rpc = admissionTapRpc({urls, chainId: stream.chain_id, fetcher: async (url, {body}) => {
    const {id, method, params} = JSON.parse(body);
    const endpoint = urls.indexOf(url);
    requests.push([endpoint, method, params]);
    let result;
    if (method === 'eth_chainId') result = endpoint === 1 || chainChanged ? '0x1' : '0x7a69';
    else if (method === 'eth_getBlockByNumber') result = {...block, hash: endpoint === 0 ? hash : fallbackHash};
    else if (endpoint === 0) return new Response(null, {status: 503});
    else result = [];
    return Response.json({jsonrpc: '2.0', id, result});
  }});
  const scan = tapDiscovery({rpc, chainId: stream.chain_id, tokenContract: token});
  const signal = AbortSignal.timeout(3000);
  const page = await scan(work(), signal);
  assert.deepEqual(requests.map(([endpoint, method]) => [endpoint, method]), [
    [0, 'eth_chainId'], [0, 'eth_getBlockByNumber'], [0, 'eth_getLogs'], [1, 'eth_chainId'],
    [2, 'eth_chainId'], [2, 'eth_getBlockByNumber'], [2, 'eth_getLogs'],
  ]);
  assert.equal(requests.at(-1)[2][0].blockHash, fallbackHash);
  assert.equal(page.coverage.finalized_hash, fallbackHash);
  assert.equal(page.coverage.block_hash, fallbackHash);

  chainChanged = true;
  requests.length = 0;
  await assert.rejects(scan(work(), signal));
  assert.deepEqual(requests, urls.map((_, i) => [i, 'eth_chainId', []]));
});

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
test('TNK observes payload and position from one real signed checkout and requires canonical catchup',async()=>{
  const s={rail:'tnk',network:'testnet1',msb_bootstrap:h},destination=testTnkAddress('07'.repeat(32));
  const f=await createTnkDiscoveryFixture({hash:h,destination});
  try {
    const scan=tnkDiscovery({msb:f.msb,network:s.network,msbBootstrap:h,frontier:f.frontier,now:()=>100000});
    const page=await scan(work({stream:s}),AbortSignal.timeout(1000));
    assert.equal(page.next_cursor,'11');assert.deepEqual(page.observations,[{destination,position:'10',transaction_hash:h,observed_at_ms:100000}]);
    f.msb.state.getSignedLength=()=>9;await assert.rejects(scan(work({stream:s}),AbortSignal.timeout(1000)));
  } finally {await f.close();}
});
test('TNK discovery waits for cold-reader catchup without moving its authoritative boundary',async()=>{
  const s={rail:'tnk',network:'testnet1',msb_bootstrap:h},destination=testTnkAddress('07'.repeat(32));
  const f=await createTnkDiscoveryFixture({hash:h,destination});
  try {
    const proof=await f.frontier(),full=f.msb.state.getSignedLength;
    let caught=false,reads=0;
    f.msb.state.getSignedLength=()=>{reads++;return caught?full():0;};
    const timer=setTimeout(()=>{caught=true;},20);
    try {
      const scan=tnkDiscovery({msb:f.msb,network:s.network,msbBootstrap:h,frontier:async()=>proof});
      const page=await scan(work({stream:s}),AbortSignal.timeout(3000));
      assert.equal(page.next_cursor,String(proof.signed_length));assert.equal(page.observations.length,1);assert(reads>1);
      caught=false;
      await assert.rejects(scan(work({stream:s}),AbortSignal.timeout(20)),error=>error.name==='AbortError'||error.name==='TimeoutError');
    } finally {clearTimeout(timer);}
  } finally {await f.close();}
});
test('discovery worker idle makes no chain read and a lost completion is never replaced by a guessed cursor',async()=>{
  let calls=0,scanCalls=0,posted;
  const scan=tapDiscovery({rpc:async method=>method==='eth_getLogs'?[]:block,chainId:stream.chain_id,tokenContract:token});
  const w=new AdmissionDiscoveryWorker({origin:'https://fixture.invalid',credential:'public-fixture-only',stream,
    scan:async(...args)=>{scanCalls++;return scan(...args);},
    fetcher:async(url,o)=>{calls++;if(url.endsWith('/pull'))return new Response(JSON.stringify({schema_version:1,purpose:'proxy_admission_fee',phase:'verify',work:calls===1?null:work()}));posted=JSON.parse(o.body);throw Error('lost ACK');}});
  assert.equal((await w.runOnce()).status,'idle');assert.equal(scanCalls,0);
  await assert.rejects(w.runOnce());assert.equal(posted.from_cursor,'10');assert.equal(posted.next_cursor,'11');assert.equal(calls,3);
  await assert.rejects(w.runOnce(),/lost ACK/);assert.equal(scanCalls,2);assert.equal(posted.from_cursor,'10');assert.equal(calls,5);
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

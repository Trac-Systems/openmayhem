// Purpose-isolated transfer observation. Discovery only queues references; the
// existing independent verifier and issuer retain all financial authority.
import { boundedJson, fixedOrigin } from './proxy-admission-worker.mjs';
import { need, shape, hex, uint, PURPOSE } from './proxy-admission-wire.mjs';
import { ERC20_TRANSFER_TOPIC, parseHexInt } from './retail-crypto-verification.mjs';
import { scanTnkSignedPage } from './proxy-admission-tnk.mjs';

const cursor = value => typeof value === 'string' && /^(0|[1-9][0-9]{0,15})$/.test(value) && BigInt(value) <= BigInt(Number.MAX_SAFE_INTEGER);
const ethHash = value => typeof value === 'string' && /^0x[0-9a-f]{64}$/.test(value);
const ethAddress = value => typeof value === 'string' && /^0x[0-9a-f]{40}$/.test(value);
export function validateDiscoveryWork(w, stream) {
  shape(w,['schema_version','purpose','phase','stream_id','stream','from_cursor','from_offset','max_positions','lease_token','lease_expires_at_ms']);
  need(w.schema_version===1 && w.purpose===PURPOSE && w.phase==='verify' && hex(w.stream_id) && hex(w.lease_token)
    && cursor(w.from_cursor) && uint(w.from_offset) && w.from_offset<=0x7fffffff && w.max_positions===16 && uint(w.lease_expires_at_ms,1),'invalid discovery work');
  shape(w.stream,Object.keys(stream));
  need(Object.keys(stream).every(key=>w.stream[key]===stream[key]),'discovery stream differs'); return w;
}
export class AdmissionDiscoveryWorker {
  constructor({origin,credential,stream,scan,fetcher=fetch,allowLoopbackHttp=false,now=Date.now,timeoutMs=15000}) {
    need(typeof credential==='string'&&credential.length>=16&&typeof scan==='function'&&uint(timeoutMs,100)&&timeoutMs<=15000,'discovery configuration required');
    this.origin=fixedOrigin(origin,{allowLoopbackHttp});this.o={credential,stream,scan,fetcher,now,timeoutMs};this.active=false;
  }
  async post(action,body,signal) {
    return await boundedJson(`${this.origin}/internal/proxy-admission-worker/discovery/${action}`,{body,signal,
      fetcher:this.o.fetcher,headers:{authorization:`Bearer ${this.o.credential}`}});
  }
  async runOnce() {
    need(!this.active,'discovery worker is busy');this.active=true;
    const o=this.o,signal=AbortSignal.timeout(o.timeoutMs),envelope={schema_version:1,purpose:PURPOSE,phase:'verify'};
    try {
      const response=await this.post('pull',{...envelope,rails:[o.stream.rail]},signal);
      shape(response,['schema_version','purpose','phase','work']);need(response.schema_version===1&&response.purpose===PURPOSE&&response.phase==='verify','discovery response differs');
      if(response.work===null)return {status:'idle'};
      const work=validateDiscoveryWork(response.work,o.stream);
      const remaining=work.lease_expires_at_ms-o.now();
      need(remaining>1000,'discovery lease too short');
      // Network transit consumes some lease time. The minimum supported 15s
      // lease must remain usable, with all work bounded by the time left.
      const leasedSignal=AbortSignal.any([signal,AbortSignal.timeout(remaining)]);
      const page=await o.scan(work,leasedSignal);
      shape(page,['next_cursor','next_offset','observations',...(Object.hasOwn(page,'coverage')?['coverage']:[])]);
      need(cursor(page.next_cursor)&&uint(page.next_offset)&&Array.isArray(page.observations)&&page.observations.length<=16,'invalid discovery page');
      const completed=await this.post('complete',{...envelope,stream_id:work.stream_id,lease_token:work.lease_token,
        from_cursor:work.from_cursor,from_offset:work.from_offset,...page},leasedSignal);
      need(completed.accepted===true&&completed.purpose===PURPOSE&&completed.phase==='verify','discovery completion rejected');
      return {status:'observed',observations:page.observations.length};
    } finally {this.active=false;}
  }
}

/** Operator-owned RPC URLs may contain the credential path used by QuickNode.
 * No caller-selected endpoint, redirect or transaction-submission method. */
export function admissionTapRpc({urls,chainId,allowLoopbackHttp=false,fetcher=fetch}) {
  need(Array.isArray(urls)&&urls.length>0&&urls.length<=4&&uint(chainId,1),'TAP RPC configuration required');
  const endpoints=urls.map(raw=>{const u=new URL(raw);need(!u.username&&!u.password&&!u.search&&!u.hash
    &&(u.protocol==='https:'||allowLoopbackHttp&&u.protocol==='http:'&&['127.0.0.1','[::1]'].includes(u.hostname)),'invalid operator RPC URL');return u.toString();});
  let id=0;
  const read=async(url,method,params,signal)=>{const rid=++id;
    const r=await boundedJson(url,{body:{jsonrpc:'2.0',id:rid,method,params},signal,fetcher,maxBytes:262144});
    need(r.jsonrpc==='2.0'&&r.id===rid&&!r.error,'TAP RPC unavailable');return r.result;};
  return async(method,params,signal)=>{
    need(['eth_chainId','eth_getBlockByNumber','eth_getLogs','eth_getTransactionReceipt'].includes(method),'read-only RPC required');
    for(const url of endpoints){
      try {
        const attempt=AbortSignal.any([signal,AbortSignal.timeout(4000)]);
        const chain=await read(url,'eth_chainId',[],attempt);need(parseHexInt(chain,'chain')===BigInt(chainId),'TAP chain differs');
        return method==='eth_chainId'?chain:await read(url,method,params,attempt);
      }catch{if(signal.aborted)throw signal.reason;}
    }
    throw new Error('Configured TAP RPCs unavailable');
  };
}
export function tapDiscovery({rpc,chainId,tokenContract}) {
  need(typeof rpc==='function'&&uint(chainId,1)&&ethAddress(tokenContract),'TAP discovery configuration required');
  return async(work,signal)=>{
    validateDiscoveryWork(work,{rail:'tap',chain_id:chainId,token_contract:tokenContract});
    const from=BigInt(work.from_cursor),offset=work.from_offset;
    const finalized=await rpc('eth_getBlockByNumber',['finalized',false],signal);
    need(finalized&&ethHash(finalized.hash),'TAP finalized frontier unavailable');
    const tip=parseHexInt(finalized.number,'finalized block');
    if(from>tip)return {next_cursor:String(from),next_offset:offset,observations:[]};
    // One block per page bounds RPC work. Persisted log offset handles dense
    // blocks without discarding later events or a fixed total recipient cap.
    const number=`0x${from.toString(16)}`;
    const block=from===tip?finalized:await rpc('eth_getBlockByNumber',[number,false],signal);
    need(block?.number===number&&ethHash(block.hash)&&ethHash(block.parentHash),'TAP canonical block differs');
    const observed=Number(parseHexInt(block.timestamp,'block time'))*1000;need(uint(observed,1),'TAP block time invalid');
    const logs=await rpc('eth_getLogs',[{blockHash:block.hash,address:tokenContract,topics:[ERC20_TRANSFER_TOPIC]}],signal);
    need(Array.isArray(logs),'TAP transfer logs missing');
    let previous=-1;
    for(const log of logs){
      need(log.address===tokenContract&&log.blockHash===block.hash&&log.blockNumber===number&&log.removed!==true
        &&Array.isArray(log.topics)&&log.topics.length===3&&log.topics[0]===ERC20_TRANSFER_TOPIC&&ethHash(log.topics[1])&&ethHash(log.topics[2])
        &&log.topics[2].slice(2,26)==='0'.repeat(24)&&ethHash(log.transactionHash),'TAP log identity differs');
      const index=Number(parseHexInt(log.logIndex,'log index'));need(uint(index)&&index<=0x7fffffff&&index>previous,'TAP log order differs');previous=index;
    }
    need(offset<=logs.length,'TAP retained offset differs');
    const selected=logs.slice(offset,offset+16),nextOffset=offset+selected.length;
    return {next_cursor:nextOffset===logs.length?String(from+1n):String(from),next_offset:nextOffset===logs.length?0:nextOffset,
      coverage:{kind:'tap_finalized_block',block_number:String(from),block_hash:block.hash,parent_hash:block.parentHash,
        block_time_ms:observed,finalized_number:String(tip),finalized_hash:finalized.hash,log_count:logs.length},
      observations:selected.map(log=>({destination:`0x${log.topics[2].slice(26)}`,position:String(from),observed_at_ms:observed,
        transaction_hash:log.transactionHash,log_index:Number(parseHexInt(log.logIndex,'log index'))}))};
  };
}

export function tnkDiscovery({msb,network,msbBootstrap,frontier,now=Date.now,addressPrefix=network==='mainnet'?'trac':'testtrac'}) {
  need(['mainnet','testnet1'].includes(network)&&hex(msbBootstrap)&&typeof frontier==='function','TNK discovery configuration required');
  return async(work,signal)=>{
    validateDiscoveryWork(work,{rail:'tnk',network,msb_bootstrap:msbBootstrap});need(work.from_offset===0,'TNK cursor offset differs');
    const current=await frontier(signal);need(uint(current,1)&&msb.state.getSignedLength()>=current,'TNK canonical reader behind');
    const page=await scanTnkSignedPage(msb,{from:Number(work.from_cursor),frontier:current,signal,addressPrefix});
    // Observer time remains explicit. It must not become an invented ledger
    // timestamp or authorize unpaid quote renewal; that needs canonical barriers.
    return {next_cursor:page.next_cursor,next_offset:0,
      observations:page.transfers.map(transfer=>({...transfer,observed_at_ms:now()}))};
  };
}

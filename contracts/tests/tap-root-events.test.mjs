import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { tapRootEventReader } from '../scripts/tap-root-events.mjs';

const root='0x'+'a'.repeat(64), tx='0x'+'b'.repeat(64);
const address='0x'+'1'.repeat(40), epoch=17, cumulativeSpentWei=10n;
function fixture(t,{head=26000000,pending=true,posted=false,block=head-300}={}) {
  const dir=fs.mkdtempSync(path.join(os.tmpdir(),'tap-events-'));
  t.after(()=>fs.rmSync(dir,{recursive:true,force:true}));
  const file=path.join(dir,'events.json'), calls=[], blockReads=[];
  const events=[{blockNumber:block,transactionHash:tx,args:{merkleRoot:root,cumulativeSpent:10n,nonce:0n}}];
  const provider={getNetwork:async()=>({chainId:1n}),getBlock:async n=>{
    const number=n==='latest'?head:n;blockReads.push(number);return {number,timestamp:1000+number*12};
  }};
  const pool={runner:{provider},getAddress:async()=>address,epoch:async()=>posted?BigInt(epoch):0n,
    pendingRoot:async()=>pending?{newEpoch:epoch,executeAfter:1000+block*12+3600}:[0,0,0,0,0,0,0],
    governanceDelay:async()=>3600n,filters:{RootPosted:e=>({name:'RootPosted',epoch:e}),RootProposed:e=>({name:'RootProposed',epoch:e})},
    queryFilter:async(filter,from,to)=>{assert.ok(to-from<10000,'QuickNode block limit');calls.push({filter,from,to});return events.filter(e=>e.blockNumber>=from&&e.blockNumber<=to);}};
  const args={pool,epoch,rateTimestamp:1000,file};
  const request={name:'RootProposed',root,cumulativeSpentWei,nonce:0n,throughBlock:head};
  return {args,request,file,calls,blockReads,events,head,pool};
}

test('pending proposal recovery anchors by time and restarts with exact-block reads',async t=>{
  const f=fixture(t),read=await tapRootEventReader(f.args);
  assert.ok(f.blockReads.length<=27,'logarithmic one-time anchor lookup');
  assert.equal((await read(f.request)).transactionHash,tx);
  assert.equal(f.calls.length,1);assert.ok(f.calls[0].from>f.head-400);
  f.blockReads.length=0;f.calls.length=0;
  const resumed=await tapRootEventReader(f.args);
  assert.equal((await resumed(f.request)).transactionHash,tx);
  assert.equal(f.blockReads.length,0,'restart reuses durable anchor');
  assert.equal(f.calls.length,1);assert.equal(f.calls[0].from,f.calls[0].to);
  await assert.rejects(resumed({...f.request,root:'0x'+'c'.repeat(64)}),/conflicts/);
  await assert.rejects(tapRootEventReader({...f.args,epoch:epoch+1}),/identity/);
});

test('legacy executed-root search has bounded attempts and persists forward progress',async t=>{
  const f=fixture(t,{head:100000,pending:false,posted:true,block:65000});
  let read=await tapRootEventReader(f.args);
  await assert.rejects(read(f.request),{code:'TAP_EVENT_LOOKUP_PENDING'});
  assert.equal(f.calls.length,4);assert.equal(f.calls[3].to,39999);
  f.calls.length=0;read=await tapRootEventReader(f.args);
  assert.equal((await read(f.request)).blockNumber,65000);
  assert.equal(f.calls[0].from,40000,'no rescanning completed ranges');
  assert.ok(f.calls.length<=4);
});

test('new roots persist an anchor before broadcast and retry an RPC failure without skipping',async t=>{
  const f=fixture(t,{pending:false,block:26000001});
  const read=await tapRootEventReader(f.args),state=JSON.parse(fs.readFileSync(f.file));
  assert.equal(state.start_block,f.head-64);assert.deepEqual(state.events,{});
  const original=f.pool.queryFilter;let fail=true;
  f.pool.queryFilter=async(...args)=>{if(fail){fail=false;throw Error('temporary RPC failure');}return original(...args);};
  await assert.rejects(read({...f.request,throughBlock:f.head+1}),/temporary RPC failure/);
  const resumed=await tapRootEventReader(f.args);
  assert.equal((await resumed({...f.request,throughBlock:f.head+1})).blockNumber,f.head+1);
});

test('cached events are re-read after a pre-finality reorg',async t=>{
  const f=fixture(t),read=await tapRootEventReader(f.args);
  await read(f.request);
  f.events[0]={...f.events[0],blockNumber:f.events[0].blockNumber+1,transactionHash:'0x'+'d'.repeat(64)};
  const resumed=await tapRootEventReader(f.args),event=await resumed(f.request);
  assert.equal(event.transactionHash,f.events[0].transactionHash);
  assert.equal(JSON.parse(fs.readFileSync(f.file)).events.RootProposed.tx,event.transactionHash);
});

test('corrupt cursors and oversized state fail closed',async t=>{
  const f=fixture(t);await tapRootEventReader(f.args);
  const state=JSON.parse(fs.readFileSync(f.file));state.events.RootPosted={next_block:-1};fs.writeFileSync(f.file,JSON.stringify(state));
  await assert.rejects(tapRootEventReader(f.args),/position/);
  fs.writeFileSync(f.file,'x'.repeat(17000));await assert.rejects(tapRootEventReader(f.args),/too large/);
});

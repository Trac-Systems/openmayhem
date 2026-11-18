import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { advanceSweep, prepareSweep } from '../scripts/prepare-tap-sweep-bundle.mjs';

const anchor = 'a'.repeat(64);
const mid = 'payout/liability/tap/' + 'b'.repeat(64) + '/' + 'c'.repeat(64);
const checksum = file => createHash('sha256').update(fs.readFileSync(file)).digest('hex');
const read = file => JSON.parse(fs.readFileSync(file,'utf8'));
const write = (file,obj) => fs.writeFileSync(file,JSON.stringify(obj));

function fixture(t, epoch=7) {
  const spool = fs.mkdtempSync(path.join(os.tmpdir(), 'tap-sweep-'));
  t.after(() => fs.rmSync(spool,{recursive:true,force:true}));
  fs.mkdirSync(path.join(spool,'processed'));
  const dir=path.join(spool,'work'); fs.mkdirSync(dir);
  const file=path.join(dir,'bundle.tmp');
  const records = {
    'epoch/apply/state': {updated_epoch:epoch,last_apply_hash:anchor,pending_epoch:null,last_settlement_unix:1234},
    ['epoch/apply-anchor/'+epoch]: {type:'epoch_apply_anchor',epoch,apply_hash:anchor,settlement_unix:1234},
    ['epoch/seal/'+epoch]: {type:'epoch_empty_seal',epoch,seal_hash:anchor,totals:{debited_au:'0',earned_au:'0',fee_au:'0',burn_au:'0'}},
  };
  const requests=[];
  const fetchImpl=async url=>{
    const key=url.searchParams.get('key'); requests.push(key);
    assert.equal(url.searchParams.get('confirmed'),'true');
    assert.equal(url.searchParams.get('signed_length'),key==='epoch/apply/state'?null:'321');
    return {ok:true,json:async()=>({confirmed:true,signed_length:321,key,value:records[key]??null})};
  };
  return { args:{epoch,anchor,spool,dir,file,peer:'http://localhost/v1',empty:true,fetchImpl},records,requests };
}

test('a signed empty epoch schedules a liability page without a new receipt; restart reuses its exact bytes', async t => {
  const {args,requests} = fixture(t);
  await prepareSweep(args);
  const first=fs.readFileSync(args.file); const bundle=JSON.parse(first);
  assert.deepEqual(bundle.receipts,[]);assert.deepEqual(bundle.tap_liability_sweep,{schema_version:1,signed_length:321,after:null});
  assert.equal(bundle.at,1234);assert.equal(typeof bundle.params.payout_min_au,'string');
  assert(requests.includes('epoch/seal/7'));
  await prepareSweep({...args,fetchImpl:()=>{throw Error('unexpected network read')}});
  assert.deepEqual(fs.readFileSync(args.file),first);
});

test('an incomplete epoch, missing seal, or inconsistent policy cannot create an intent',async t=>{
  const {args,records}=fixture(t);
  records['epoch/apply/state'].pending_epoch=8;
  await assert.rejects(prepareSweep(args),/fully applied/);
  records['epoch/apply/state'].pending_epoch=null;records['epoch/seal/7']=null;
  await assert.rejects(prepareSweep(args),/empty TAP epoch seal/);
  write(args.file,{epoch:7,rail:'tap',epoch_apply_hash:anchor,receipts:[],params:{fee_bps:678}});
  await assert.rejects(prepareSweep({...args,empty:false}),/policy mismatch/);
  assert.equal(fs.existsSync(path.join(args.dir,'tap-sweep-intent.json')),false);
});

test('the cursor advances only from the exact verified report and replays idempotently after a crash',async t=>{
  const {args}=fixture(t);
  await prepareSweep(args);
  const bundle=read(args.file);
  const prefix=path.join(args.spool,'processed','epoch-7-'+anchor);
  const published=prefix+'.receipts.json';const report=prefix+'.settlement.json';
  fs.copyFileSync(args.file,published);
  const pointer=bundle.tap_liability_sweep;
  write(report,{tap_liability_sweep:{...pointer,at:1234,next_after:mid,eligible_count:0}});
  write(path.join(args.dir,'tap.produced'),{epoch:7,epoch_apply_hash:anchor,evidence_sha256:checksum(published)});
  const marker=path.join(args.dir,'tap.complete.tmp');
  const proof={rail:'tap',status:'carry',epoch:7,epoch_apply_hash:anchor,evidence:report,evidence_sha256:checksum(report)};
  write(marker,{...proof,evidence_sha256:'0'.repeat(64)});
  const commit=()=>advanceSweep({...args,marker});
  assert.throws(commit,/evidence changed/);
  const cursor=path.join(args.spool,'processed/liability-sweep-cursor.json');
  assert.equal(fs.existsSync(cursor),false);
  write(marker,proof);commit();
  const first=fs.readFileSync(cursor); assert.equal(JSON.parse(first).next_after,mid);
  commit();assert.deepEqual(fs.readFileSync(cursor),first);
  const next=fixture(t,8);await prepareSweep({...next.args,spool:args.spool});
  assert.equal(read(next.args.file).tap_liability_sweep.after,mid);
  write(report,{tap_liability_sweep:{...pointer,at:1234,next_after:null,eligible_count:0}});
  write(marker,{...proof,evidence_sha256:checksum(report)});
  assert.throws(commit,/Conflicting TAP page/);
  assert.deepEqual(fs.readFileSync(cursor),first);
});

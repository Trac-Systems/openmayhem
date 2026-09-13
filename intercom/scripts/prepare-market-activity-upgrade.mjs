#!/usr/bin/env node
// Offline only: read canonical state exports, validate coverage, emit unsigned admin commands.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {fileURLToPath} from 'node:url';
import assert from 'node:assert/strict';
import MayhemContract from '../contract/contract.js';

const compare = (a,b) => a<b?-1:a>b?1:0;
export function catalogActivityInventory(catalog) {
  return catalog.models.map((row) => ({
    model_id: row.model_id, model_class: row.model_class,
    rate_units: (row.price_ref_au?.rate_map?.map((r)=>r.unit) ??
      (row.model_class==='text-generation'?['input_token','output_token']:[])).sort(compare),
    mode: row.activity_calibration ? 'calibrated_work_v1' : 'relative_dimension_vector_v1',
    calibration: row.activity_calibration ?? null,
  })).sort((a,b)=>compare(a.model_id,b.model_id));
}
export function prepareMarketActivityUpgrade(snapshot, overrides = {}) {
  assert(Number.isSafeInteger(snapshot.at) && snapshot.at>=0,'canonical snapshot at is required');
  assert(snapshot.epoch_apply_state && snapshot.epoch_apply_state.pending_epoch == null,
    'finish the prior-version paged epoch before preparing an upgrade');
  assert(Array.isArray(snapshot.pending_price_commits) && snapshot.pending_price_commits.length===0,
    'resolve all prior-version nonempty price-root commitments before upgrade; legacy AU proofs must not be reinterpreted');
  assert(snapshot.modelrefs && snapshot.enclaves && Array.isArray(snapshot.prices),
    'canonical modelrefs/enclaves maps and complete prices schedule array are required');
  const c=new MayhemContract({},{}); const markets=[];const seen=new Set();
  const inventory=[];
  for(const schedule of snapshot.prices) {
    const current=schedule.pending?.effective_at<=snapshot.at?schedule.pending:schedule.current;
    if(!current)continue;
    const enclave=snapshot.enclaves[current.enclave_id];
    if(enclave?.status!=='active')continue;
    assert.equal(current.model_id,enclave.model_id,'price/enclave model mismatch');
    assert.equal((current.seed??current).set_by_role,'admin','active price lacks admin provenance');
    const ref=snapshot.modelrefs[enclave.model_id];assert(ref,'active market modelref missing');
    const key=current.enclave_id+'/'+(current.ctx_bracket??'base');assert(!seen.has(key),'duplicate active schedule');seen.add(key);
    markets.push({enclave_id:current.enclave_id,...(current.ctx_bracket?{
      ctx_bracket:current.ctx_bracket,ctx_bracket_table_ver:current.ctx_bracket_table_ver}:{} )});
  }
  markets.sort((a,b)=>compare(a.enclave_id,b.enclave_id)||compare(a.ctx_bracket??'',b.ctx_bracket??''));
  assert(markets.length<=5000,'active market migration exceeds canonical index bound');
  const modelrefCommands=[];
  for(const [modelId,ref] of Object.entries(snapshot.modelrefs).sort(([a],[b])=>compare(a,b))) {
    const calibration=overrides[modelId]??ref.activity_calibration;
    if(calibration) {
      const error=c.validateActivityCalibration(calibration,ref.model_class,ref.rate_map);assert(!error,error?.message);
      if(overrides[modelId])modelrefCommands.push({op:'set_model_ref',model_id:modelId,model_class:ref.model_class,
        rate_map:ref.rate_map,...(ref.source_hash?{source_hash:ref.source_hash}:{}),activity_calibration:calibration});
    }
    inventory.push({model_id:modelId,model_class:ref.model_class,rate_units:ref.rate_map.map(r=>r.unit).sort(compare),
      activity_basis:calibration?'calibrated_work_v1':'relative_dimension_vector_v1',
      calibration_source_hash:calibration?.source_hash??null});
  }
  for(const modelId of Object.keys(overrides))assert(snapshot.modelrefs[modelId],'calibration override has no canonical modelref');
  const commands=[];
  for(let i=0;i<markets.length||i===0;i+=128)commands.push({op:'migrate_market_pricing',at:snapshot.at,markets:markets.slice(i,i+128)});
  return {schema_version:1,contract_version:25,snapshot_sha256:crypto.createHash('sha256').update(JSON.stringify(snapshot)).digest('hex'),
    active_market_count:markets.length,modelref_inventory:inventory,commands:[...commands,...modelrefCommands],
    required_final_index:markets,hard_min_bps:2500,hard_max_bps:40000,
    note:'Unsigned offline plan. Compare required_final_index with canonical market/activity/index after all signed batches. Every market bootstraps one settled epoch; no provider-count gate or dollar target.'};
}
if(process.argv[1]&&path.resolve(process.argv[1])===fileURLToPath(import.meta.url)) {
  const [snapshotPath,outputPath,overridesPath]=process.argv.slice(2);
  assert(snapshotPath&&outputPath,'usage: prepare-market-activity-upgrade.mjs canonical-snapshot.json unsigned-plan.json [calibrations.json]');
  const result=prepareMarketActivityUpgrade(JSON.parse(fs.readFileSync(snapshotPath,'utf8')),
    overridesPath?JSON.parse(fs.readFileSync(overridesPath,'utf8')):{});
  fs.writeFileSync(outputPath,JSON.stringify(result,null,2)+'\n',{mode:0o600,flag:'wx'});
  console.log(JSON.stringify({ok:true,active_markets:result.active_market_count,commands:result.commands.length}));
}

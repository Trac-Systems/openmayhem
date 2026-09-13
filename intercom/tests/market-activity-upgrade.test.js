import assert from 'node:assert/strict';
import test from 'node:test';
import fs from 'node:fs';
import {prepareMarketActivityUpgrade,catalogActivityInventory} from '../scripts/prepare-market-activity-upgrade.mjs';

test('offline upgrade generator includes every active base and context price, never changes reference prices',()=>{
 const ref={model_class:'workflow',rate_map:[{unit:'pixel_frame',per_unit_au:'12',granularity:1}]};
 const snapshot={at:3600,epoch_apply_state:{updated_epoch:1,pending_epoch:null},pending_price_commits:[],
   modelrefs:{media:ref,text:{model_class:'text-generation',rate_map:[{unit:'input_token',per_unit_au:'1',granularity:1},{unit:'output_token',per_unit_au:'2',granularity:1}]}},
   enclaves:{a:{model_id:'media',status:'active'},b:{model_id:'text',status:'active'},c:{model_id:'media',status:'retired'}},
   prices:[{current:{enclave_id:'a',model_id:'media',set_by_role:'admin'}},{current:{enclave_id:'b',model_id:'text',set_by_role:'admin',ctx_bracket:'le8k',ctx_bracket_table_ver:1}},{current:{enclave_id:'c',model_id:'media',set_by_role:'admin'}}]};
 const before=JSON.stringify(snapshot);const plan=prepareMarketActivityUpgrade(snapshot);
 assert.equal(plan.active_market_count,2);assert.equal(plan.commands[0].markets.length,2);
 assert.ok(plan.modelref_inventory.every(r=>r.activity_basis==='relative_dimension_vector_v1'));
 assert.equal(JSON.stringify(snapshot),before);
 const pending=structuredClone(snapshot);pending.pending_price_commits=[{epoch:1}];
 assert.throws(()=>prepareMarketActivityUpgrade(pending),/resolve all prior-version/);
});
test('signed catalog inventory covers all current model rows and billing units',()=>{
 const catalog=JSON.parse(fs.readFileSync(new URL('../../catalog/models.json',import.meta.url),'utf8'));
 const inventory=catalogActivityInventory(catalog);
 assert.equal(inventory.length,catalog.models.length);assert.ok(inventory.every(row=>row.rate_units.length>0));
});

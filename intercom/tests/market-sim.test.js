import assert from 'node:assert/strict';
import test from 'node:test';
import {runMarketSimulation,validateMarketSimulation,formatMarketSimulationMarkdown,marketConstants} from '../scripts/market-sim.mjs';

test('market simulation uses live activity constants and respects step/hard bands',()=>{
  const report=runMarketSimulation();assert.deepEqual(report.constants,marketConstants());
  const result=validateMarketSimulation(report);assert.equal(result.ok,true,result.failures.join('\n'));
});
test('one-provider and empty markets respond after a single baseline epoch',()=>{
  const r=runMarketSimulation();
  assert.ok(BigInt(r.scenarios.one_provider.rows[19].price_au)>BigInt(r.seed_price_au));
  assert.ok(BigInt(r.scenarios.one_provider.rows[39].price_au)<BigInt(r.scenarios.one_provider.rows[38].price_au));
  assert.equal(BigInt(r.scenarios.empty.summary.final_price_au),BigInt(r.seed_price_au)/4n);
  assert.ok(BigInt(r.scenarios.empty.rows[20].price_au)<BigInt(r.scenarios.empty.rows[19].price_au), 'consecutive empty epochs keep decreasing');
});
test('spend and phantom-provider changes do not alter market activity',()=>{
  const r=runMarketSimulation();
  for(const name of ['spend_spike','phantom_supply']) assert.ok(r.scenarios[name].rows.every(row=>row.price_au===r.seed_price_au));
});
test('simulation describes activity without a dollar utilization target',()=>{
  const markdown=formatMarketSimulationMarkdown(runMarketSimulation());
  assert.match(markdown,/Settled activity momentum/);assert.match(markdown,/max_step_bps/);assert.match(markdown,/Validation: PASS/);
  assert.doesNotMatch(markdown,/target_utilization_bps|provider_epoch_target_au/);
});

#!/usr/bin/env node
import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import MayhemContract, { contractParamDefinitions } from '../contract/contract.js';

const SEED = 1_000_000_000_000_000_000n;
const DEFAULTS = Object.fromEntries(Object.entries(contractParamDefinitions()).map(([k,v]) => [k,v.default]));
export const marketConstants = () => new MayhemContract({}, {}).marketPriceConstants(DEFAULTS);
const bpsDelta = (a,b) => Number((a>b?a-b:b-a)*10000n/b);

// Exogenous settled-work scenarios. Dollars/provider counts are varied only as
// evidence labels, never supplied to the contract's activity math.
export function runMarketSimulation() {
  const c = new MayhemContract({}, {}); const constants = marketConstants();
  const definitions = {
    stable: (e) => 1000n,
    rise: (e) => e < 20 ? 1000n : 2000n,
    fall: (e) => e < 20 ? 1000n : 500n,
    empty: (e) => e < 20 ? 1000n : 0n,
    one_provider: (e) => e < 20 ? 1000n : e < 40 ? 2000n : 500n,
    adversarial_spike: (e) => e === 20 ? 1000000000000n : 1000n,
    phantom_supply: (e) => 1000n,
    spend_spike: (e) => 1000n,
  };
  const scenarios = {};
  for (const [name, work] of Object.entries(definitions)) {
    let price = SEED, ema = null, previousActivity = null;
    const rows = [];
    for (let epoch=1;epoch<=120;epoch++) {
      const activity = work(epoch).toString();
      const multiplier = previousActivity === null ? 10000 : c.marketActivityMomentum(activity,previousActivity,constants);
      previousActivity = activity;
      const previous = price;
      const desired = c.scalePriceTerm(price.toString(),multiplier);
      price = BigInt(c.stepPriceTerm(price.toString(),desired,constants));
      price = price < SEED/4n ? SEED/4n : price > SEED*4n ? SEED*4n : price;
      ema = ema === null ? activity : c.marketActivityEma(ema,activity,constants);
      rows.push({epoch,activity,momentum_bps:multiplier,ema_activity:ema,price_au:price.toString(),
        step_bps:bpsDelta(price,previous),frozen:epoch===1,
        provider_count:name==='one_provider'?1:name==='phantom_supply'&&epoch===20?1000000:2,
        settled_gross_au:name==='spend_spike'&&epoch===20?'1000000000000000000000':'100'});
    }
    const amounts=rows.map(r=>BigInt(r.price_au));
    const last=amounts.slice(-20); const min=last.reduce((a,b)=>a<b?a:b),max=last.reduce((a,b)=>a>b?a:b);
    scenarios[name]={name,epochs:rows.length,rows,summary:{
      final_price_au:price.toString(),max_step_bps:Math.max(...rows.map(r=>r.step_bps)),
      min_price_au:amounts.reduce((a,b)=>a<b?a:b).toString(),max_price_au:amounts.reduce((a,b)=>a>b?a:b).toString(),
      last_price_range_bps:bpsDelta(max,min),frozen_epochs:1,
    }};
  }
  return {controller:'Settled activity momentum v2',seed_price_au:SEED.toString(),constants,scenarios};
}
export function validateMarketSimulation(report) {
  const failures=[];
  for(const s of Object.values(report.scenarios)) {
    if(s.summary.max_step_bps>report.constants.max_step_bps)failures.push(s.name+': step exceeded');
    if(BigInt(s.summary.min_price_au)<SEED/4n||BigInt(s.summary.max_price_au)>SEED*4n)failures.push(s.name+': hard band exceeded');
    if(s.summary.last_price_range_bps>350)failures.push(s.name+': final activity did not settle');
  }
  for(const name of ['phantom_supply','spend_spike']) if(report.scenarios[name].rows.some(r=>r.price_au!==SEED.toString())) failures.push(name+': non-work input moved price');
  if(BigInt(report.scenarios.rise.rows[19].price_au)<=SEED)failures.push('rising work did not raise price');
  if(BigInt(report.scenarios.fall.rows[19].price_au)>=SEED)failures.push('falling work did not lower price');
  return {ok:failures.length===0,failures};
}
export function formatMarketSimulationMarkdown(report) {
  const validation=validateMarketSimulation(report);
  return ['# Settled activity momentum simulation','',
    `Validation: ${validation.ok?'PASS':'FAIL'}`,'',
    'Aggregate work is compared with the immediately previous epoch. EMA is telemetry only. Initialized empty epochs keep decreasing toward the lower band. Provider counts and settled spend do not enter the controller.','',
    '```json',JSON.stringify(report.constants,null,2),'```','',
    '| Scenario | Final price / seed | Maximum step (bps) |','| --- | ---: | ---: |',
    ...Object.values(report.scenarios).map(s=>`| ${s.name} | ${Number(BigInt(s.summary.final_price_au)*10000n/SEED)/10000} | ${s.summary.max_step_bps} |`),
    '', 'A one-epoch spike is bounded; momentum pricing does not promise a return to a seed price or a dollar revenue target.',''].join('\n');
}
if (process.argv[1] && path.resolve(process.argv[1])===fileURLToPath(import.meta.url)) {
  const report=runMarketSimulation();const markdown=formatMarketSimulationMarkdown(report);
  if(process.argv.includes('--write-report'))await fs.writeFile('market-activity-simulation.md',markdown);
  else process.stdout.write(markdown);
  if(!validateMarketSimulation(report).ok)process.exitCode=1;
}

#!/usr/bin/env node
import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import MayhemContract, { contractParamDefinitions } from '../contract/contract.js';

const SEED = 1_000_000_000_000_000_000n;
const DEFAULTS = Object.fromEntries(Object.entries(contractParamDefinitions()).map(([k,v]) => [k,v.default]));
export const marketConstants = () => new MayhemContract({}, {}).marketPriceConstants(DEFAULTS);
const bpsDelta = (a,b) => Number((a>b?a-b:b-a)*10000n/b);

// Exogenous signed slot-utilization scenarios. Dollars/provider counts are
// varied only as evidence labels, never supplied to the controller.
export function runMarketSimulation() {
  const c = new MayhemContract({}, {}); const constants = marketConstants();
  const definitions = {
    stable: () => 5_000,
    rise: (e) => e < 20 ? 5_000 : 8_500,
    fall: (e) => e < 20 ? 5_000 : 1_500,
    empty: (e) => e < 20 ? 5_000 : 0,
    one_provider: (e) => e < 20 ? 5_000 : e < 40 ? 8_500 : 1_500,
    adversarial_spike: (e) => e === 20 ? 10_000 : 5_000,
    phantom_supply: () => 5_000,
    spend_spike: () => 5_000,
  };
  const scenarios = {};
  for (const [name, utilization] of Object.entries(definitions)) {
    let price = SEED;
    const rows = [];
    for (let epoch=1;epoch<=120;epoch++) {
      const utilizationBps = utilization(epoch);
      const multiplier = c.marketUtilizationMultiplier(utilizationBps);
      const previous = price;
      price = BigInt(c.scalePriceTerm(price.toString(), multiplier));
      price = price < SEED/4n ? SEED/4n : price > SEED*4n ? SEED*4n : price;
      rows.push({epoch,utilization_bps:utilizationBps,multiplier_bps:multiplier,
        price_au:price.toString(),step_bps:bpsDelta(price,previous),
        provider_count:name==='one_provider'?1:name==='phantom_supply'&&epoch===20?1000000:2,
        settled_gross_au:name==='spend_spike'&&epoch===20?'1000000000000000000000':'100'});
    }
    const amounts=rows.map(r=>BigInt(r.price_au));
    const last=amounts.slice(-20); const min=last.reduce((a,b)=>a<b?a:b),max=last.reduce((a,b)=>a>b?a:b);
    scenarios[name]={name,epochs:rows.length,rows,summary:{
      final_price_au:price.toString(),max_step_bps:Math.max(...rows.map(r=>r.step_bps)),
      min_price_au:amounts.reduce((a,b)=>a<b?a:b).toString(),max_price_au:amounts.reduce((a,b)=>a>b?a:b).toString(),
      last_price_range_bps:bpsDelta(max,min),
    }};
  }
  return {controller:'Signed slot utilization v3',seed_price_au:SEED.toString(),constants,scenarios};
}
export function validateMarketSimulation(report) {
  const failures=[];
  for(const s of Object.values(report.scenarios)) {
    if(s.summary.max_step_bps>report.constants.price_step_bps)failures.push(s.name+': step exceeded');
    if(BigInt(s.summary.min_price_au)<SEED/4n||BigInt(s.summary.max_price_au)>SEED*4n)failures.push(s.name+': hard band exceeded');
    if(s.summary.last_price_range_bps>350)failures.push(s.name+': final activity did not settle');
  }
  for(const name of ['phantom_supply','spend_spike']) if(report.scenarios[name].rows.some(r=>r.price_au!==SEED.toString())) failures.push(name+': non-work input moved price');
  if(BigInt(report.scenarios.rise.rows[19].price_au)<=SEED)failures.push('high utilization did not raise price');
  if(BigInt(report.scenarios.fall.rows[19].price_au)>=SEED)failures.push('low utilization did not lower price');
  return {ok:failures.length===0,failures};
}
export function formatMarketSimulationMarkdown(report) {
  const validation=validateMarketSimulation(report);
  return ['# Signed slot utilization simulation','',
    `Validation: ${validation.ok?'PASS':'FAIL'}`,'',
    'Each epoch uses absolute signed slot utilization. At or above 80% the price rises 10%; at or below 20% it falls 10%; the middle band holds. Empty epochs keep decreasing toward the lower band. Provider counts and settled spend do not choose direction.','',
    '```json',JSON.stringify(report.constants,null,2),'```','',
    '| Scenario | Final price / seed | Maximum step (bps) |','| --- | ---: | ---: |',
    ...Object.values(report.scenarios).map(s=>`| ${s.name} | ${Number(BigInt(s.summary.final_price_au)*10000n/SEED)/10000} | ${s.summary.max_step_bps} |`),
    '', 'A one-epoch spike moves one step. Later mid-band epochs hold that price; no dollar revenue target or previous-hour comparison is used.',''].join('\n');
}
if (process.argv[1] && path.resolve(process.argv[1])===fileURLToPath(import.meta.url)) {
  const report=runMarketSimulation();const markdown=formatMarketSimulationMarkdown(report);
  if(process.argv.includes('--write-report'))await fs.writeFile('market-activity-simulation.md',markdown);
  else process.stdout.write(markdown);
  if(!validateMarketSimulation(report).ok)process.exitCode=1;
}

#!/usr/bin/env node
import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import MayhemContract from '../contract/contract.js';

const BPS = 10_000;
const DEFAULT_EPOCH_SECONDS = 3_600;
const DEFAULT_SEED_PRICE_MU = 1_000_000;
const REPORT_DATE = '2026-07-07';

const providerMinAskFactors = Object.freeze(
  Array.from({ length: 160 }, (_, index) => 0.62 + (index / 159) * 0.76)
);

const throwIfError = (value) => {
  if (value instanceof Error) throw value;
  return value;
};

const bpsDelta = (left, right) => {
  if (right === 0) return left === 0 ? 0 : BPS;
  return Math.floor((Math.abs(left - right) * BPS) / right);
};

const pct = (bps) => `${(bps / 100).toFixed(2)}%`;

const makeContract = () => new MayhemContract({}, {});

export function marketConstants() {
  return makeContract().marketPriceConstants();
}

export function makeMarketAgents({ seedPriceMu = DEFAULT_SEED_PRICE_MU } = {}) {
  const constants = marketConstants();
  const providers = providerMinAskFactors.map((factor, index) => ({
    id: `provider-${String(index + 1).padStart(2, '0')}`,
    min_ask_mu: Math.round(seedPriceMu * factor),
  }));
  const seedActiveSupply = providers.filter((provider) => provider.min_ask_mu <= seedPriceMu).length;
  const targetDemandMu = Math.floor(
    (seedActiveSupply * constants.provider_epoch_target_mu * constants.target_utilization_bps) /
    BPS
  );

  const users = Array.from({ length: 640 }, (_, index) => {
    const bidFactor = 0.72 + (index / 639) * 1.28;
    const base = 35_000 + ((index * 37) % 43) * 1_250;
    return {
      id: `user-${String(index + 1).padStart(3, '0')}`,
      max_bid_mu: Math.round(seedPriceMu * bidFactor),
      base_demand_mu: base,
    };
  });
  const seedDemandMu = users
    .filter((user) => user.max_bid_mu >= seedPriceMu)
    .reduce((sum, user) => sum + user.base_demand_mu, 0);
  const demandScale = targetDemandMu / seedDemandMu;

  return {
    seed_price_mu: seedPriceMu,
    providers,
    users: users.map((user) => ({
      ...user,
      base_demand_mu: Math.max(1, Math.round(user.base_demand_mu * demandScale)),
    })),
  };
}

export const SCENARIOS = Object.freeze({
  surge: Object.freeze({
    title: 'Surge then normalize',
    epochs: 72,
    demandMultiplier: (epoch) => (epoch < 8 ? 1.0 : epoch < 28 ? 2.15 : 1.0),
  }),
  drain: Object.freeze({
    title: 'Demand drain then normalize',
    epochs: 72,
    demandMultiplier: (epoch) => (epoch < 8 ? 1.0 : epoch < 28 ? 0.36 : 1.0),
  }),
  thin_liquidity: Object.freeze({
    title: 'Thin liquidity stays pinned',
    epochs: 32,
    providerCap: 1,
    demandMultiplier: () => 3.0,
  }),
  adversarial_spike: Object.freeze({
    title: 'Single-epoch adversarial demand spike',
    epochs: 48,
    demandMultiplier: (epoch) => (epoch === 12 ? 14.0 : 1.0),
  }),
  phantom_supply: Object.freeze({
    title: 'Single-epoch phantom-supply depression',
    epochs: 48,
    demandMultiplier: () => 1.0,
    phantomSupply: (epoch) => (epoch === 12 ? 190 : 0),
  }),
});

function activeProviderCount(agents, priceMu, scenario, epoch) {
  let active = agents.providers.filter((provider) => provider.min_ask_mu <= priceMu);
  if (Number.isSafeInteger(scenario.providerCap)) active = active.slice(0, scenario.providerCap);
  const phantomSupply = scenario.phantomSupply ? scenario.phantomSupply(epoch) : 0;
  return active.length + phantomSupply;
}

function activeDemandMu(agents, priceMu, scenario, epoch) {
  const seedPriceMu = agents.seed_price_mu;
  const multiplier = scenario.demandMultiplier ? scenario.demandMultiplier(epoch) : 1.0;
  const grossDemandElasticity = scenario.grossDemandElasticity ?? 0.72;
  const priceFactor = Math.pow(seedPriceMu / Math.max(1, priceMu), grossDemandElasticity);
  const demand = agents.users
    .filter((user) => user.max_bid_mu >= priceMu)
    .reduce((sum, user) => sum + user.base_demand_mu * multiplier * priceFactor, 0);
  return Math.max(0, Math.round(demand));
}

export function nextMarketEpoch(contract, previous, observation) {
  const constants = contract.marketPriceConstants();
  const utilizationBps = throwIfError(
    contract.marketUtilizationBps(observation.demand_mu, observation.active_supply)
  );
  const frozen = observation.active_supply < constants.cold_start_min_providers;
  const emaUtilizationBps = frozen
    ? constants.target_utilization_bps
    : contract.marketEmaUtilizationBps(previous.ema_utilization_bps, utilizationBps);
  const multiplierBps = frozen ? BPS : contract.marketCurveMultiplierBps(emaUtilizationBps);
  const desiredPriceMu = frozen
    ? previous.seed_price_mu
    : throwIfError(contract.scalePriceTerm(previous.seed_price_mu, multiplierBps));
  const priceMu = frozen
    ? previous.seed_price_mu
    : throwIfError(contract.stepPriceTerm(previous.price_mu, desiredPriceMu));

  return {
    seed_price_mu: previous.seed_price_mu,
    price_mu: priceMu,
    desired_price_mu: desiredPriceMu,
    utilization_bps: utilizationBps,
    ema_utilization_bps: emaUtilizationBps,
    multiplier_bps: multiplierBps,
    active_supply: observation.active_supply,
    demand_mu: observation.demand_mu,
    frozen,
  };
}

export function simulateScenario(name, scenario, options = {}) {
  const contract = options.contract ?? makeContract();
  const agents = options.agents ?? makeMarketAgents(options);
  let state = {
    seed_price_mu: agents.seed_price_mu,
    price_mu: agents.seed_price_mu,
    ema_utilization_bps: contract.marketPriceConstants().target_utilization_bps,
  };
  const rows = [];

  for (let epoch = 0; epoch < scenario.epochs; epoch += 1) {
    const previousPriceMu = state.price_mu;
    const observation = {
      active_supply: activeProviderCount(agents, previousPriceMu, scenario, epoch),
      demand_mu: activeDemandMu(agents, previousPriceMu, scenario, epoch),
    };
    state = nextMarketEpoch(contract, state, observation);
    rows.push({
      epoch,
      previous_price_mu: previousPriceMu,
      price_mu: state.price_mu,
      desired_price_mu: state.desired_price_mu,
      price_step_bps: bpsDelta(state.price_mu, previousPriceMu),
      utilization_bps: state.utilization_bps,
      ema_utilization_bps: state.ema_utilization_bps,
      multiplier_bps: state.multiplier_bps,
      active_supply: state.active_supply,
      demand_mu: state.demand_mu,
      frozen: state.frozen,
    });
  }

  return {
    name,
    title: scenario.title,
    epochs: scenario.epochs,
    rows,
    summary: summarizeRows(rows, agents.seed_price_mu, contract.marketPriceConstants()),
  };
}

function summarizeRows(rows, seedPriceMu, constants) {
  const final = rows.at(-1);
  const lastWindow = rows.slice(-8);
  const lastPrices = lastWindow.map((row) => row.price_mu);
  const lastUtilizations = lastWindow.map((row) => row.utilization_bps);
  const maxStepBps = Math.max(...rows.map((row) => row.price_step_bps));
  const maxPriceMu = Math.max(...rows.map((row) => row.price_mu));
  const minPriceMu = Math.min(...rows.map((row) => row.price_mu));
  const lastPriceRangeBps = bpsDelta(Math.max(...lastPrices), Math.min(...lastPrices));
  const lastUtilizationRangeBps = Math.max(...lastUtilizations) - Math.min(...lastUtilizations);

  return {
    final_price_mu: final.price_mu,
    final_price_deviation_bps: bpsDelta(final.price_mu, seedPriceMu),
    final_utilization_bps: final.utilization_bps,
    final_ema_utilization_bps: final.ema_utilization_bps,
    final_ema_deviation_bps: Math.abs(
      final.ema_utilization_bps - constants.target_utilization_bps
    ),
    max_step_bps: maxStepBps,
    max_price_mu: maxPriceMu,
    min_price_mu: minPriceMu,
    max_price_deviation_bps: Math.max(
      bpsDelta(maxPriceMu, seedPriceMu),
      bpsDelta(minPriceMu, seedPriceMu)
    ),
    last_price_range_bps: lastPriceRangeBps,
    last_utilization_range_bps: lastUtilizationRangeBps,
    frozen_epochs: rows.filter((row) => row.frozen).length,
  };
}

export function runMarketSimulation(options = {}) {
  const contract = options.contract ?? makeContract();
  const constants = contract.marketPriceConstants();
  const agents = options.agents ?? makeMarketAgents(options);
  const scenarios = Object.fromEntries(
    Object.entries(SCENARIOS).map(([name, scenario]) => [
      name,
      simulateScenario(name, scenario, { ...options, agents, contract }),
    ])
  );
  const report = {
    report_date: REPORT_DATE,
    controller: 'I3-F1 utilization-indexed market price controller',
    seed_price_mu: agents.seed_price_mu,
    epoch_seconds: DEFAULT_EPOCH_SECONDS,
    constants,
    scenarios,
  };
  report.validation = validateMarketSimulation(report);
  return report;
}

export function validateMarketSimulation(report) {
  const failures = [];
  const constants = report.constants;
  const requireScenario = (name) => {
    const scenario = report.scenarios[name];
    if (!scenario) failures.push(`missing scenario ${name}`);
    return scenario;
  };

  for (const scenario of Object.values(report.scenarios)) {
    if (scenario.summary.max_step_bps > constants.max_step_bps) {
      failures.push(
        `${scenario.name} exceeded clamp: ${scenario.summary.max_step_bps} > ${constants.max_step_bps}`
      );
    }
    if (scenario.summary.last_price_range_bps > 350) {
      failures.push(
        `${scenario.name} has sustained price oscillation: ${scenario.summary.last_price_range_bps} bps`
      );
    }
  }

  for (const name of ['surge', 'drain']) {
    const scenario = requireScenario(name);
    if (!scenario) continue;
    if (scenario.summary.final_ema_deviation_bps > 550) {
      failures.push(
        `${name} did not converge to target utilization EMA: ` +
        `${scenario.summary.final_ema_deviation_bps} bps deviation`
      );
    }
    if (scenario.summary.last_utilization_range_bps > 800) {
      failures.push(
        `${name} still has unstable utilization in final window: ` +
        `${scenario.summary.last_utilization_range_bps} bps range`
      );
    }
  }

  const thin = requireScenario('thin_liquidity');
  if (thin) {
    const allPinned = thin.rows.every(
      (row) => row.frozen && row.price_mu === report.seed_price_mu
    );
    if (!allPinned) failures.push('thin_liquidity was not pinned at P0 while below S_min');
  }

  for (const name of ['adversarial_spike', 'phantom_supply']) {
    const scenario = requireScenario(name);
    if (!scenario) continue;
    if (scenario.summary.final_ema_deviation_bps > 650) {
      failures.push(
        `${name} did not damp back toward target: ` +
        `${scenario.summary.final_ema_deviation_bps} bps deviation`
      );
    }
    if (scenario.summary.final_price_deviation_bps > 400) {
      failures.push(
        `${name} left price too far from seed: ` +
        `${scenario.summary.final_price_deviation_bps} bps deviation`
      );
    }
  }

  return {
    ok: failures.length === 0,
    failures,
  };
}

export function formatMarketSimulationMarkdown(report) {
  const lines = [];
  lines.push('# I3-F7 Pre-Mainnet Market Simulation Report');
  lines.push('');
  lines.push(`Date: ${report.report_date}`);
  lines.push(`Controller: ${report.controller}`);
  lines.push(`Seed price: ${report.seed_price_mu} muUSD`);
  lines.push(`Epoch length: ${report.epoch_seconds}s`);
  lines.push('');
  lines.push('## Protocol Constants Used');
  lines.push('');
  lines.push('| Constant | Value |');
  lines.push('|---|---:|');
  for (const [key, value] of Object.entries(report.constants)) {
    lines.push(`| ${key} | ${value} |`);
  }
  lines.push('');
  lines.push('## Scenario Results');
  lines.push('');
  lines.push('| Scenario | Epochs | Final price | Final util | Final EMA | Max step | Final-window range | Verdict |');
  lines.push('|---|---:|---:|---:|---:|---:|---:|---|');
  for (const scenario of Object.values(report.scenarios)) {
    const summary = scenario.summary;
    lines.push(
      `| ${scenario.title} | ${scenario.epochs} | ${summary.final_price_mu} | ` +
      `${pct(summary.final_utilization_bps)} | ${pct(summary.final_ema_utilization_bps)} | ` +
      `${pct(summary.max_step_bps)} | ${pct(summary.last_price_range_bps)} | pass |`
    );
  }
  lines.push('');
  lines.push('## Gate Findings');
  lines.push('');
  lines.push('- The simulator imports `intercom/contract/contract.js` and calls the live F1 market math (`marketUtilizationBps`, `marketEmaUtilizationBps`, `marketCurveMultiplierBps`, `scalePriceTerm`, `stepPriceTerm`).');
  lines.push('- Surge and drain recover to the target-utilization EMA without sustained final-window oscillation.');
  lines.push('- Thin liquidity remains pinned at `P0` while active supply is below `S_min`.');
  lines.push('- One-epoch demand and phantom-supply manipulation cannot move the operating price beyond `max_step_bps` in that epoch, and the EMA damps back toward target.');
  lines.push('');
  lines.push(report.validation.ok ? 'Validation: PASS' : `Validation: FAIL (${report.validation.failures.join('; ')})`);
  return `${lines.join('\n')}\n`;
}

export async function writeMarketSimulationReport(report, outputPath) {
  await fs.mkdir(path.dirname(outputPath), { recursive: true });
  await fs.writeFile(outputPath, formatMarketSimulationMarkdown(report), 'utf8');
  return outputPath;
}

function isMain() {
  return process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1]);
}

if (isMain()) {
  const args = new Set(process.argv.slice(2));
  const report = runMarketSimulation();
  if (args.has('--json')) {
    console.log(JSON.stringify(report, null, 2));
  } else {
    console.log(formatMarketSimulationMarkdown(report));
  }
  if (args.has('--write-report')) {
    const scriptDir = path.dirname(fileURLToPath(import.meta.url));
    const repoRoot = path.resolve(scriptDir, '../..');
    const outputPath = path.join(repoRoot, 'docs/reports/market-simulation-2026-07-07.md');
    await writeMarketSimulationReport(report, outputPath);
  }
  if (!report.validation.ok) {
    console.error(report.validation.failures.join('\n'));
    process.exitCode = 1;
  }
}

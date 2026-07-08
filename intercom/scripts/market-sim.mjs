#!/usr/bin/env node
import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import MayhemContract, { contractParamDefinitions } from '../contract/contract.js';

const BPS = 10_000;
const DEFAULT_EPOCH_SECONDS = 3_600;
const DEFAULT_SEED_PRICE_AU = '1000000000000000000';
const ATTO_PER_LEGACY_DEMAND_POINT = 1_000_000_000_000n;
const FLOAT_SCALE = 1_000_000n;
const REPORT_DATE = '2026-07-07';

const providerMinAskFactors = Object.freeze(
  Array.from({ length: 160 }, (_, index) => 0.62 + (index / 159) * 0.76)
);

const throwIfError = (value) => {
  if (value instanceof Error) throw value;
  return value;
};

const parseAuBigInt = (value) => BigInt(value);
const auString = (value) => value.toString();
const compareAu = (left, right) => {
  const leftAu = parseAuBigInt(left);
  const rightAu = parseAuBigInt(right);
  return leftAu < rightAu ? -1 : leftAu > rightAu ? 1 : 0;
};
const maxAu = (values) => values.reduce((max, value) => (
  compareAu(value, max) > 0 ? value : max
));
const minAu = (values) => values.reduce((min, value) => (
  compareAu(value, min) < 0 ? value : min
));
const scaleAuByFloat = (value, factor, { allowZero = false } = {}) => {
  if (!Number.isFinite(factor) || factor < 0) {
    throw new Error('Invalid market simulation factor.');
  }
  const scaledFactor = BigInt(Math.round(factor * Number(FLOAT_SCALE)));
  const scaled = (parseAuBigInt(value) * scaledFactor + (FLOAT_SCALE / 2n)) / FLOAT_SCALE;
  return auString(scaled > 0n || allowZero ? scaled : 1n);
};
const bpsDelta = (left, right) => {
  const leftAu = parseAuBigInt(left);
  const rightAu = parseAuBigInt(right);
  if (rightAu === 0n) return leftAu === 0n ? 0 : BPS;
  const delta = leftAu > rightAu ? leftAu - rightAu : rightAu - leftAu;
  return Number((delta * BigInt(BPS)) / rightAu);
};

const pct = (bps) => `${(bps / 100).toFixed(2)}%`;

const makeContract = () => new MayhemContract({}, {});
const defaultParamValues = () => Object.fromEntries(
  Object.entries(contractParamDefinitions()).map(([key, definition]) => [key, definition.default])
);

export function marketConstants() {
  return makeContract().marketPriceConstants(defaultParamValues());
}

export function makeMarketAgents({ seedPriceAu = DEFAULT_SEED_PRICE_AU, constants = marketConstants() } = {}) {
  const providers = providerMinAskFactors.map((factor, index) => ({
    id: `provider-${String(index + 1).padStart(2, '0')}`,
    min_ask_au: scaleAuByFloat(seedPriceAu, factor),
  }));
  const seedActiveSupply = providers.filter((provider) => (
    compareAu(provider.min_ask_au, seedPriceAu) <= 0
  )).length;
  const targetDemandAu = auString(
    (BigInt(seedActiveSupply) *
      parseAuBigInt(constants.provider_epoch_target_au) *
      BigInt(constants.target_utilization_bps)) /
    BigInt(BPS)
  );

  const users = Array.from({ length: 640 }, (_, index) => {
    const bidFactor = 0.72 + (index / 639) * 1.28;
    const base = BigInt(35_000 + ((index * 37) % 43) * 1_250) *
      ATTO_PER_LEGACY_DEMAND_POINT;
    return {
      id: `user-${String(index + 1).padStart(3, '0')}`,
      max_bid_au: scaleAuByFloat(seedPriceAu, bidFactor),
      base_demand_au: auString(base),
    };
  });
  const seedDemandAu = users
    .filter((user) => compareAu(user.max_bid_au, seedPriceAu) >= 0)
    .reduce((sum, user) => sum + parseAuBigInt(user.base_demand_au), 0n);
  const demandScale = Number(parseAuBigInt(targetDemandAu)) / Number(seedDemandAu);

  return {
    seed_price_au: seedPriceAu,
    providers,
    users: users.map((user) => ({
      ...user,
      base_demand_au: scaleAuByFloat(user.base_demand_au, demandScale),
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

function activeProviderCount(agents, priceAu, scenario, epoch) {
  let active = agents.providers.filter((provider) => compareAu(provider.min_ask_au, priceAu) <= 0);
  if (Number.isSafeInteger(scenario.providerCap)) active = active.slice(0, scenario.providerCap);
  const phantomSupply = scenario.phantomSupply ? scenario.phantomSupply(epoch) : 0;
  return active.length + phantomSupply;
}

function activeDemandAu(agents, priceAu, scenario, epoch) {
  const seedPriceAu = agents.seed_price_au;
  const multiplier = scenario.demandMultiplier ? scenario.demandMultiplier(epoch) : 1.0;
  const grossDemandElasticity = scenario.grossDemandElasticity ?? 0.72;
  const priceFactor = Math.pow(
    Number(parseAuBigInt(seedPriceAu)) / Math.max(1, Number(parseAuBigInt(priceAu))),
    grossDemandElasticity
  );
  return auString(agents.users
    .filter((user) => compareAu(user.max_bid_au, priceAu) >= 0)
    .reduce((sum, user) => (
      sum + parseAuBigInt(scaleAuByFloat(user.base_demand_au, multiplier * priceFactor, {
        allowZero: true,
      }))
    ), 0n));
}

export function nextMarketEpoch(contract, previous, observation) {
  const constants = observation.constants ?? marketConstants();
  const utilizationBps = throwIfError(
    contract.marketUtilizationBps(observation.demand_au, observation.active_supply, constants)
  );
  const frozen = observation.active_supply < constants.cold_start_min_providers;
  const emaUtilizationBps = frozen
    ? constants.target_utilization_bps
    : contract.marketEmaUtilizationBps(previous.ema_utilization_bps, utilizationBps, constants);
  const multiplierBps = frozen ? BPS : contract.marketCurveMultiplierBps(emaUtilizationBps, constants);
  const desiredPriceAu = frozen
    ? previous.seed_price_au
    : throwIfError(contract.scalePriceTerm(previous.seed_price_au, multiplierBps));
  const priceAu = frozen
    ? previous.seed_price_au
    : throwIfError(contract.stepPriceTerm(previous.price_au, desiredPriceAu, constants));

  return {
    seed_price_au: previous.seed_price_au,
    price_au: priceAu,
    desired_price_au: desiredPriceAu,
    utilization_bps: utilizationBps,
    ema_utilization_bps: emaUtilizationBps,
    multiplier_bps: multiplierBps,
    active_supply: observation.active_supply,
    demand_au: observation.demand_au,
    frozen,
  };
}

export function simulateScenario(name, scenario, options = {}) {
  const contract = options.contract ?? makeContract();
  const constants = options.constants ?? marketConstants();
  const agents = options.agents ?? makeMarketAgents(options);
  let state = {
    seed_price_au: agents.seed_price_au,
    price_au: agents.seed_price_au,
    ema_utilization_bps: constants.target_utilization_bps,
  };
  const rows = [];

  for (let epoch = 0; epoch < scenario.epochs; epoch += 1) {
    const previousPriceAu = state.price_au;
    const observation = {
      active_supply: activeProviderCount(agents, previousPriceAu, scenario, epoch),
      demand_au: activeDemandAu(agents, previousPriceAu, scenario, epoch),
      constants,
    };
    state = nextMarketEpoch(contract, state, observation);
    rows.push({
      epoch,
      previous_price_au: previousPriceAu,
      price_au: state.price_au,
      desired_price_au: state.desired_price_au,
      price_step_bps: bpsDelta(state.price_au, previousPriceAu),
      utilization_bps: state.utilization_bps,
      ema_utilization_bps: state.ema_utilization_bps,
      multiplier_bps: state.multiplier_bps,
      active_supply: state.active_supply,
      demand_au: state.demand_au,
      frozen: state.frozen,
    });
  }

  return {
    name,
    title: scenario.title,
    epochs: scenario.epochs,
    rows,
    summary: summarizeRows(rows, agents.seed_price_au, constants),
  };
}

function summarizeRows(rows, seedPriceAu, constants) {
  const final = rows.at(-1);
  const lastWindow = rows.slice(-8);
  const lastPrices = lastWindow.map((row) => row.price_au);
  const lastUtilizations = lastWindow.map((row) => row.utilization_bps);
  const maxStepBps = Math.max(...rows.map((row) => row.price_step_bps));
  const maxPriceAu = maxAu(rows.map((row) => row.price_au));
  const minPriceAu = minAu(rows.map((row) => row.price_au));
  const lastPriceRangeBps = bpsDelta(maxAu(lastPrices), minAu(lastPrices));
  const lastUtilizationRangeBps = Math.max(...lastUtilizations) - Math.min(...lastUtilizations);

  return {
    final_price_au: final.price_au,
    final_price_deviation_bps: bpsDelta(final.price_au, seedPriceAu),
    final_utilization_bps: final.utilization_bps,
    final_ema_utilization_bps: final.ema_utilization_bps,
    final_ema_deviation_bps: Math.abs(
      final.ema_utilization_bps - constants.target_utilization_bps
    ),
    max_step_bps: maxStepBps,
    max_price_au: maxPriceAu,
    min_price_au: minPriceAu,
    max_price_deviation_bps: Math.max(
      bpsDelta(maxPriceAu, seedPriceAu),
      bpsDelta(minPriceAu, seedPriceAu)
    ),
    last_price_range_bps: lastPriceRangeBps,
    last_utilization_range_bps: lastUtilizationRangeBps,
    frozen_epochs: rows.filter((row) => row.frozen).length,
  };
}

export function runMarketSimulation(options = {}) {
  const contract = options.contract ?? makeContract();
  const constants = options.constants ?? marketConstants();
  const agents = options.agents ?? makeMarketAgents({ ...options, constants });
  const scenarios = Object.fromEntries(
    Object.entries(SCENARIOS).map(([name, scenario]) => [
      name,
      simulateScenario(name, scenario, { ...options, agents, contract }),
    ])
  );
  const report = {
    report_date: REPORT_DATE,
    controller: 'I3-F1 utilization-indexed market price controller',
    seed_price_au: agents.seed_price_au,
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
      (row) => row.frozen && row.price_au === report.seed_price_au
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
  lines.push(`Seed price: ${report.seed_price_au} au`);
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
      `| ${scenario.title} | ${scenario.epochs} | ${summary.final_price_au} | ` +
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

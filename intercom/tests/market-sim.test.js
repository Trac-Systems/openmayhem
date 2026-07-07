import assert from 'node:assert/strict';
import test from 'node:test';

import MayhemContract from '../contract/contract.js';
import {
  runMarketSimulation,
  validateMarketSimulation,
  formatMarketSimulationMarkdown,
} from '../scripts/market-sim.mjs';

test('market simulation uses the live contract controller constants', () => {
  const report = runMarketSimulation();
  const contract = new MayhemContract({}, {});

  assert.deepEqual(report.constants, contract.marketPriceConstants());
});

test('market simulation scenarios satisfy the F7 launch-gate invariants', () => {
  const report = runMarketSimulation();
  const validation = validateMarketSimulation(report);

  assert.equal(validation.ok, true, validation.failures.join('\n'));
  for (const scenario of Object.values(report.scenarios)) {
    assert.ok(
      scenario.summary.max_step_bps <= report.constants.max_step_bps,
      `${scenario.name} exceeded per-epoch price clamp`
    );
    assert.ok(
      scenario.summary.last_price_range_bps <= 350,
      `${scenario.name} kept oscillating in the final window`
    );
  }
});

test('thin-liquidity simulation remains pinned at P0 below S_min', () => {
  const report = runMarketSimulation();
  const thin = report.scenarios.thin_liquidity;

  assert.equal(thin.summary.frozen_epochs, thin.epochs);
  assert.equal(thin.summary.max_price_mu, report.seed_price_mu);
  assert.equal(thin.summary.min_price_mu, report.seed_price_mu);
  assert.ok(thin.rows.every((row) => row.frozen && row.price_mu === report.seed_price_mu));
});

test('one-epoch demand and phantom-supply attacks are bounded and recover', () => {
  const report = runMarketSimulation();

  for (const name of ['adversarial_spike', 'phantom_supply']) {
    const scenario = report.scenarios[name];
    assert.ok(scenario.summary.max_step_bps <= report.constants.max_step_bps);
    assert.ok(scenario.summary.final_price_deviation_bps <= 400);
    assert.ok(scenario.summary.final_ema_deviation_bps <= 650);
  }
});

test('market simulation report renders the selected constants and pass verdict', () => {
  const report = runMarketSimulation();
  const markdown = formatMarketSimulationMarkdown(report);

  assert.match(markdown, /target_utilization_bps/);
  assert.match(markdown, /max_step_bps/);
  assert.match(markdown, /Validation: PASS/);
});

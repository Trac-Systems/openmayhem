#!/usr/bin/env node
// Offline preparation only. This file never connects, signs, or submits.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {fileURLToPath} from 'node:url';
import assert from 'node:assert/strict';
import {advanceDemand, demandObservation, normalizedDemand} from '../contract/reference-demand.js';

export function prepareReferenceDemandUpgrade(snapshot) {
  assert(Number.isSafeInteger(snapshot.at) && snapshot.at >= 0);
  const through = snapshot.epoch_apply_state?.updated_epoch;
  assert(Number.isSafeInteger(through) && through > 0 && snapshot.epoch_apply_state.pending_epoch == null,
    'A completed canonical epoch boundary is required.');
  assert(Array.isArray(snapshot.pending_price_commits) && snapshot.pending_price_commits.length === 0,
    'Resolve prior nonempty price commitments before changing deterministic pricing.');
  assert(snapshot.complete_from_epoch === 1 && snapshot.complete_through_epoch === through,
    'Use the complete bounded export, not a selected model/date subset.');
  assert(Array.isArray(snapshot.rows) && Array.isArray(snapshot.prices) && snapshot.enclaves);
  const sourceHash = crypto.createHash('sha256').update(JSON.stringify(snapshot)).digest('hex');
  const keyFor = (r) => `${r.enclave_id}/${r.ctx_bracket ?? ''}`;
  const groups = new Map();
  for (const row of snapshot.rows) {
    assert(Number.isSafeInteger(row.epoch) && row.epoch > 0 && row.epoch <= through);
    const key = keyFor(row);
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(row);
  }
  for (const rows of groups.values()) {
    rows.sort((a, b) => a.epoch - b.epoch);
    assert(rows.every((r, i) => i === 0 || r.epoch > rows[i - 1].epoch), 'Duplicate market epoch.');
  }
  const commands = [], inventory = [], seen = new Set();
  for (const schedule of snapshot.prices) {
    const price = schedule.pending?.effective_at <= snapshot.at ? schedule.pending : schedule.current;
    if (!price) continue;
    const enclave = snapshot.enclaves[price.enclave_id];
    if (enclave?.status !== 'active') continue;
    assert(price.model_id === enclave.model_id && (price.seed ?? price).set_by_role === 'admin');
    const key = keyFor(price);
    assert(!seen.has(key), 'Duplicate price market.'); seen.add(key);
    let state = null;
    const referenceEpochs = [], afterReference = [];
    for (const row of groups.get(key) ?? []) {
      assert(row.model_id === price.model_id, 'Canonical model identity changed within a market.');
      const observation = demandObservation(row);
      if (!state?.reference) {
        const result = advanceDemand(state, observation);
        if (result.state.warmup.length > (state?.warmup.length ?? 0) || result.state.reference) referenceEpochs.push(row.epoch);
        state = result.state;
      } else if (normalizedDemand(observation.units, state.reference) !== null) {
        afterReference.push(row.epoch);
      }
    }
    inventory.push({model_id: price.model_id, enclave_id: price.enclave_id, ctx_bracket: price.ctx_bracket ?? null,
      status: state?.reference ? (state.reference.thin ? 'ready_thin_reference' : 'ready') : referenceEpochs.length ? 'partial_reference' : 'no_paid_evidence',
      reference_observations: referenceEpochs.length, history_observations: Math.min(72, afterReference.length)});
    if (!referenceEpochs.length) continue;
    commands.push({op: 'bootstrap_market_demand', at: snapshot.at, enclave_id: price.enclave_id,
      ...(price.ctx_bracket ? {ctx_bracket: price.ctx_bracket, ctx_bracket_table_ver: price.ctx_bracket_table_ver} : {}),
      through_epoch: through, expected_price_ver: price.ver, source_hash: sourceHash,
      reference_epochs: referenceEpochs, history_epochs: afterReference.slice(-72)});
  }
  return {schema_version: 1, contract_version: 29, source_hash: sourceHash,
    through_epoch: through, inventory, commands,
    note: 'Unsigned one-time bounded initialization. No model filters; references use first usable evidence and latest known demand. Review before submitting at the captured frozen boundary.'};
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [input, output] = process.argv.slice(2);
  assert(input && output, 'usage: prepare-reference-demand-upgrade.mjs canonical-snapshot.json unsigned-plan.json');
  fs.writeFileSync(output, JSON.stringify(prepareReferenceDemandUpgrade(JSON.parse(fs.readFileSync(input, 'utf8'))), null, 2) + '\n', {mode: 0o600});
}

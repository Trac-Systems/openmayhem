#!/usr/bin/env node
// Offline only: read a canonical state export and emit unsigned v27 migration commands.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';

const compare = (a, b) => a < b ? -1 : a > b ? 1 : 0;

export function catalogActivityInventory(catalog) {
  return catalog.models.map((row) => ({
    model_id: row.model_id,
    model_class: row.model_class,
    rate_units: (row.price_ref_au?.rate_map?.map((rate) => rate.unit) ??
      (row.model_class === 'text-generation'
        ? ['input_token', 'output_token']
        : (row.model_class === 'embedding' ? ['input_token'] : [])))
      .sort(compare),
    activity_basis: 'signed_slot_time_v1',
  })).sort((left, right) => compare(left.model_id, right.model_id));
}

export function prepareMarketActivityUpgrade(snapshot) {
  assert(Number.isSafeInteger(snapshot.at) && snapshot.at >= 0,
    'canonical snapshot at is required');
  assert(snapshot.epoch_apply_state && snapshot.epoch_apply_state.pending_epoch == null,
    'finish the prior-version paged epoch before preparing an upgrade');
  assert(Array.isArray(snapshot.pending_price_commits) && snapshot.pending_price_commits.length === 0,
    'resolve all prior-version nonempty price-root commitments before upgrade; old price proofs must not be reinterpreted');
  assert(snapshot.modelrefs && snapshot.enclaves && Array.isArray(snapshot.prices),
    'canonical modelrefs/enclaves maps and complete prices schedule array are required');

  const markets = [];
  const seen = new Set();
  const inventory = Object.entries(snapshot.modelrefs)
    .sort(([left], [right]) => compare(left, right))
    .map(([modelId, ref]) => ({
      model_id: modelId,
      model_class: ref.model_class,
      rate_units: ref.rate_map.map((rate) => rate.unit).sort(compare),
      activity_basis: 'signed_slot_time_v1',
    }));

  for (const schedule of snapshot.prices) {
    const current = schedule.pending?.effective_at <= snapshot.at
      ? schedule.pending
      : schedule.current;
    if (!current) continue;
    const enclave = snapshot.enclaves[current.enclave_id];
    if (enclave?.status !== 'active') continue;
    assert.equal(current.model_id, enclave.model_id, 'price/enclave model mismatch');
    assert.equal((current.seed ?? current).set_by_role, 'admin',
      'active price lacks admin provenance');
    assert(snapshot.modelrefs[enclave.model_id], 'active market modelref missing');
    const key = `${current.enclave_id}/${current.ctx_bracket ?? 'base'}`;
    assert(!seen.has(key), 'duplicate active schedule');
    seen.add(key);
    markets.push({
      enclave_id: current.enclave_id,
      ...(current.ctx_bracket ? {
        ctx_bracket: current.ctx_bracket,
        ctx_bracket_table_ver: current.ctx_bracket_table_ver,
      } : {}),
    });
  }
  markets.sort((left, right) => compare(left.enclave_id, right.enclave_id) ||
    compare(left.ctx_bracket ?? '', right.ctx_bracket ?? ''));
  assert(markets.length <= 5_000, 'active market migration exceeds canonical index bound');

  const commands = [];
  for (let index = 0; index < markets.length || index === 0; index += 128) {
    commands.push({
      op: 'migrate_market_pricing',
      at: snapshot.at,
      markets: markets.slice(index, index + 128),
    });
  }
  return {
    schema_version: 2,
    contract_version: 27,
    snapshot_sha256: crypto.createHash('sha256')
      .update(JSON.stringify(snapshot)).digest('hex'),
    active_market_count: markets.length,
    modelref_inventory: inventory,
    commands,
    required_final_index: markets,
    low_utilization_bps: 2_000,
    high_utilization_bps: 8_000,
    price_step_bps: 1_000,
    hard_min_bps: 2_500,
    hard_max_bps: 40_000,
    note: 'Unsigned offline plan. Verify the final canonical activity index exactly, then resume settlement. Signed receipt slot time drives every market; model recalibration is not required.',
  };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [snapshotPath, outputPath] = process.argv.slice(2);
  assert(snapshotPath && outputPath,
    'usage: prepare-market-activity-upgrade.mjs canonical-snapshot.json unsigned-plan.json');
  const result = prepareMarketActivityUpgrade(JSON.parse(fs.readFileSync(snapshotPath, 'utf8')));
  fs.writeFileSync(outputPath, `${JSON.stringify(result, null, 2)}\n`, { mode: 0o600, flag: 'wx' });
  console.log(JSON.stringify({
    ok: true,
    active_markets: result.active_market_count,
    commands: result.commands.length,
  }));
}

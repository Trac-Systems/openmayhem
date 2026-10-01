import test from 'node:test';
import assert from 'node:assert/strict';
import {prepareReferenceDemandUpgrade} from '../scripts/prepare-reference-demand-upgrade.mjs';
import {advanceDemand, demandObservation} from '../contract/reference-demand.js';

function fixture() {
  const rows = Array.from({length: 400}, (_, i) => ({type: 'price_derivation',
    enclave_id: 'test', model_id: 'test/media', epoch: i + 1, epoch_seconds: 3600,
    usage: {session_count: i % 3 ? 1 : 0, settled_usage: {pixel_frame: String(i % 3 ? 100 + i % 17 : 0)}},
  }));
  return {at: 1440000, epoch_apply_state: {updated_epoch: 400, pending_epoch: null}, pending_price_commits: [],
    complete_from_epoch: 1, complete_through_epoch: 400, rows,
    enclaves: {test: {model_id: 'test/media', status: 'active'}},
    prices: [{current: {enclave_id: 'test', model_id: 'test/media', set_by_role: 'admin', ver: 400}}]};
}

test('bounded bootstrap reproduces the full historical controller without transferring arbitrary prices', () => {
  const snapshot = fixture(), before = JSON.stringify(snapshot);
  const plan = prepareReferenceDemandUpgrade(snapshot);
  assert.equal(JSON.stringify(snapshot), before);
  assert.equal(plan.commands.length, 1);
  const command = plan.commands[0];
  assert.equal(command.reference_epochs.length, 72);
  assert.equal(command.reference_epochs[0], 2);
  assert.equal(command.history_epochs.length, 72);
  assert.equal(command.history_epochs.at(-1), 400);
  assert.ok(!('reference' in command) && !('multiplier' in command));
  let full, bounded;
  for (const row of snapshot.rows) full = advanceDemand(full, demandObservation(row)).state;
  for (const epoch of [...command.reference_epochs, ...command.history_epochs]) {
    bounded = advanceDemand(bounded, demandObservation(snapshot.rows[epoch - 1])).state;
  }
  assert.deepEqual(bounded.reference, full.reference);
  assert.deepEqual(bounded.history, full.history);
  assert.deepEqual(bounded.last_target, full.last_target);
});

test('partial and absent evidence is reported without inventing a busy rate', () => {
  const snapshot = fixture();
  snapshot.rows = snapshot.rows.slice(0, 20);
  const plan = prepareReferenceDemandUpgrade(snapshot);
  assert.equal(plan.inventory[0].status, 'partial_reference');
  assert.equal(plan.commands[0].reference_epochs.length, 19);
  assert.deepEqual(plan.commands[0].history_epochs, []);
  snapshot.rows = [];
  const empty = prepareReferenceDemandUpgrade(snapshot);
  assert.equal(empty.inventory[0].status, 'no_paid_evidence');
  assert.equal(empty.commands.length, 0);
});

test('new axes cannot displace the last known demand history, and incomplete exports are rejected', () => {
  const snapshot = fixture();
  for (const row of snapshot.rows.slice(-100)) row.usage.settled_usage = {new_axis: '100'};
  const plan = prepareReferenceDemandUpgrade(snapshot);
  assert.equal(plan.commands[0].history_epochs.at(-1), 300);
  assert.throws(() => prepareReferenceDemandUpgrade({...snapshot, complete_from_epoch: 2}), /complete bounded export/);
  assert.throws(() => prepareReferenceDemandUpgrade({...snapshot, pending_price_commits: [{}]}), /Resolve prior/);
  assert.throws(() => prepareReferenceDemandUpgrade({...snapshot, rows: [...snapshot.rows, snapshot.rows[0]]}), /Duplicate/);
});

// Financial transports remain synthetic. Immutable outputs are derived only
// from the real contract-produced fixture liabilities. This models the existing
// CLI's externally confirmed reports; it is not Stripe/MSB or live CLI proof.
import fs from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { CONTRACT_VERSION } from '../../../intercom/contract/contract.js';
const hash = value => createHash('sha256').update(JSON.stringify(value)).digest('hex');
export async function installPayoutDouble(ctx, source, rail) {
  const rows = [...source.f.storage.values].filter(([key]) => key.startsWith(`payout/liability/${rail}/`)).map(([, value]) => value);
  const outputs = rows.map((row, output_index) => {
    const payable = BigInt(row.total_au) - BigInt(row.held_au) - BigInt(row.paid_cum_au);
    if (payable <= 0n) throw new Error('fixture must retain actual matured payable liability');
    return { role: 'provider', provider: row.provider, payout_revision: row.revision, to: row.target,
      liability_au: String(payable), paid_au: String(payable), output_index,
      economic_op_id: hash({ rail, epoch: source.epoch, apply: source.applyHash, provider: row.provider, revision: row.revision }),
      ...(rail === 'fiat' ? { source_currency: 'usd', source_amount_minor: String(payable / 10000000000000000n),
        destination_currency: row.currency, destination_amount_min_minor: '1', destination_amount_max_minor: '1000000' } : {}) };
  });
  const identity = { rail, epoch: source.epoch, epoch_apply_hash: source.applyHash };
  const plan = { op: 'prepare_targeted_payout_epoch', contract_version: CONTRACT_VERSION, ...identity,
    at: source.epoch * 3600, snapshot_signed_length: 44, outcome: 'payouts', outputs, carry: [],
    outputs_root: hash(outputs), carry_root: hash([]), plan_root: hash({ identity, outputs }),
    admin: source.f.admin.publicKey, admin_sig: '7'.repeat(128) };
  const outputRecords = outputs.map(output => ({ type: `targeted_${rail}_output_settlement`, rail, epoch: source.epoch,
    economic_op_id: output.economic_op_id, value: { op: `settle_targeted_${rail}_output`, ...identity,
      plan_root: plan.plan_root, economic_op_id: output.economic_op_id, output_index: output.output_index,
      ...(rail === 'fiat' ? { attempt_id: hash(output) } : {}) } }));
  const close = { type: 'targeted_payout_epoch_close', rail, epoch: source.epoch, plan_root: plan.plan_root,
    outcome: 'payouts', output_count: outputs.length, carry_count: 0, outputs_root: plan.outputs_root, carry_root: plan.carry_root,
    value: { op: 'close_targeted_payout_epoch', ...identity, plan_root: plan.plan_root } };
  const result = { plan: { type: 'targeted_payout_epoch_plan', rail, epoch: source.epoch, plan_root: plan.plan_root, value: plan }, outputs: outputRecords, close };
  const report = { ok: true, submitted: true, already_settled: null, no_work: false, carry_forward: false,
    epoch: source.epoch, settlement: plan, feature: result.plan, feature_result: result, settlement_state: close,
    payout_preparations: [], skipped_providers: [], planned_liabilities: outputs,
    ...(rail === 'fiat' ? { stripe_transfers: [] } : { msb_outputs: outputs, msb_transfers: [] }) };
  const reportFile = path.join(ctx.root, 'synthetic-financial-report.json');
  fs.writeFileSync(reportFile, JSON.stringify(report), { mode: 0o600 });
  const effects = path.join(ctx.root, 'effects'); fs.mkdirSync(effects);
  const records = JSON.parse(fs.readFileSync(ctx.records));
  records[`payout/epoch-plan/tnk/${source.epoch}`] = { key: `payout/epoch-plan/tnk/${source.epoch}`, confirmed: true, signed_length: 44, value: null };
  fs.writeFileSync(ctx.records, JSON.stringify(records));
  const executable = path.join(ctx.root, 'bin/synthetic-financial-cli');
  fs.writeFileSync(executable, `#!${process.execPath}
const fs = require('node:fs'), path = require('node:path');
const report = JSON.parse(fs.readFileSync(process.env.FIXTURE_REPORT));
if (process.argv[2] !== 'admin' || process.argv[3] !== report.settlement.rail + '-settlement') process.exit(92);
const effects = process.env.FIXTURE_EFFECTS;
// Durable retained plan is read back before any external effect. It is not
// regenerated after a lost ACK. Each effect is keyed by original operation.
const prepared = path.join(effects, 'prepared.json');
if (!fs.existsSync(prepared)) fs.writeFileSync(prepared, JSON.stringify(report.settlement), { flag: 'wx', mode: 0o600 });
if (fs.readFileSync(prepared, 'utf8') !== JSON.stringify(report.settlement)) process.exit(93);
const records = JSON.parse(fs.readFileSync(process.env.FIXTURE_RECORDS));
const key = 'payout/epoch-plan/' + report.settlement.rail + '/' + report.epoch;
records[key] = { key, confirmed: true, signed_length: 44, value: report.feature_result.plan };
fs.writeFileSync(process.env.FIXTURE_RECORDS, JSON.stringify(records));
for (const output of report.settlement.outputs) {
  const effect = path.join(effects, output.economic_op_id + '.effect');
  if (!fs.existsSync(effect)) {
    fs.writeFileSync(effect, JSON.stringify(output), { flag: 'wx', mode: 0o600 });
    fs.appendFileSync(path.join(effects, 'sent.log'), output.economic_op_id + '\\n');
    const crash = path.join(effects, 'crashed');
    if (process.env.FIXTURE_CRASH === '1' && !fs.existsSync(crash)) {
      fs.writeFileSync(crash, 'lost ACK', { flag: 'wx', mode: 0o600 }); process.exit(75);
    }
  } else if (fs.readFileSync(effect, 'utf8') !== JSON.stringify(output)) process.exit(94);
}
process.stdout.write(JSON.stringify(report));
`, { mode: 0o700 });
  Object.assign(ctx.env, { MAYHEM_BIN: executable, [`MAYHEM_${rail.toUpperCase()}_SETTLEMENT_ENABLED`]: '1',
    FIXTURE_REPORT: reportFile, FIXTURE_EFFECTS: effects, FIXTURE_CRASH: '1' });
  return { effects, outputs, report };
}

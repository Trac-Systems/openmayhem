import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const ROOT = fileURLToPath(new URL('../..', import.meta.url));
const SCRIPT = path.join(ROOT, 'scripts/ops-payout-settle.sh');
const FINALIZER = path.join(ROOT, 'scripts/ops-settle-epoch.sh');
const CADENCE = path.join(ROOT, 'scripts/ops-epoch-cadence.sh');
const INSTALLER = path.join(ROOT, 'scripts/install-mainnet-systemd.sh');
const TAP_ROLLER = path.join(ROOT, 'scripts/ops/run-tap-settlement-roller.sh');
const SERVICE = path.join(ROOT, 'ops/systemd/mayhem-payout-worker.service');
const TIMER = path.join(ROOT, 'ops/systemd/mayhem-payout-worker.timer');
const APPLY_HASH = 'a'.repeat(64);
const NEXT_APPLY_HASH = 'b'.repeat(64);
const EPOCH_COMMIT_HASH = '0'.repeat(64);
const EPOCH_APPLIED_AT = 'f'.repeat(64);
const FIAT_ROOT = 'b'.repeat(64);
const TNK_ROOT = 'c'.repeat(64);
const TAP_ROOT = `0x${'d'.repeat(64)}`;
const TAP_TX = `0x${'e'.repeat(64)}`;
const TAP_EXECUTION_TX = `0x${'f'.repeat(64)}`;
const TAP_PROVIDER = '8'.repeat(64);
const TAP_PAYOUT_REVISION = '9'.repeat(64);
const TAP_TARGET = `0x${'1'.repeat(40)}`;
const FIAT_PROVIDER = '6'.repeat(64);
const FIAT_PAYOUT_REVISION = '7'.repeat(64);
const FIAT_QUOTE_HASH = 'aeb7dc5278b6ad207b3dfad34b7708b1859690166b1d1f8751fd4afe5204b7d8';
const FIAT_EUR_QUOTE_HASH = '005b308d26296de4fee01e15fdf659b88e874f502f20d7d71a1d82e4b1ac4943';
const FIAT_SETTLED_BY = '9'.repeat(64);
const FIAT_PREPARATION_IDS = ['a'.repeat(64), 'b'.repeat(64)];
const FIAT_EFFECT_IDS = ['c'.repeat(64), 'd'.repeat(64)];
const TNK_PREPARATION_ID = 'e'.repeat(64);
const TNK_EFFECT_ID = 'f'.repeat(64);

function sha256(value) {
  return createHash('sha256').update(value).digest('hex');
}

function stableJson(value) {
  if (Array.isArray(value)) return `[${value.map(stableJson).join(',')}]`;
  if (value && typeof value === 'object') {
    return `{${Object.keys(value).sort().map(
      (key) => `${JSON.stringify(key)}:${stableJson(value[key])}`,
    ).join(',')}}`;
  }
  return JSON.stringify(value);
}

function writeJson(target, value) {
  const raw = `${JSON.stringify(value, null, 2)}\n`;
  fs.writeFileSync(target, raw);
  return raw;
}

function writeExecutable(target, source) {
  fs.writeFileSync(target, source, { mode: 0o755 });
}

function clone(value) {
  return structuredClone(value);
}

function writeFiatFixtures(root) {
  const fixtureDir = path.join(root, 'fiat-fixtures');
  fs.mkdirSync(fixtureDir, { recursive: true });
  const transferGroup = `mayhem_fiat_epoch_7_${APPLY_HASH.slice(0, 16)}`;
  const providerOutput = {
    role: 'provider',
    provider: FIAT_PROVIDER,
    payout_revision: FIAT_PAYOUT_REVISION,
    to: 'acct_provider',
    liability_au: '850000000000000000',
    paid_au: '840000000000000000',
    rounding_au: '10000000000000000',
    dust_au: '10000000000000000',
    source_currency: 'eur',
    source_amount_minor: '84',
    destination_currency: 'gbp',
    destination_amount_minor: '66',
    fx_quote_id: 'fxq_provider',
    fx_quote_hash: FIAT_QUOTE_HASH,
  };
  const operatorOutput = {
    role: 'operator_fee',
    to: 'platform_balance',
    liability_au: '150000000000000000',
    paid_au: '140000000000000000',
    rounding_au: '10000000000000000',
    dust_au: '10000000000000000',
    source_currency: 'eur',
    source_amount_minor: '14',
  };
  const providerTransfer = {
    schema_version: 2,
    kind: 'stripe_transfer',
    ref: 'tr_provider',
    destination: 'acct_provider',
    source_currency: 'eur',
    source_amount_minor: '84',
    destination_currency: 'gbp',
    destination_amount_minor: '66',
    fx_quote_id: 'fxq_provider',
    fx_quote_hash: FIAT_QUOTE_HASH,
    destination_payment: 'py_provider',
    transfer_group: transferGroup,
  };
  const operatorTransfer = {
    schema_version: 2,
    kind: 'platform_balance',
    ref: 'platform_balance:7:aaaaaaaaaaaaaaaa',
    destination: 'platform_balance',
    source_currency: 'eur',
    source_amount_minor: '14',
    transfer_group: null,
  };
  const settlement = {
    op: 'settle_targeted_fiat',
    epoch: 7,
    at: 1000,
    rail: 'fiat',
    processor: 'stripe',
    source_currency: 'eur',
    operator_to: 'platform_balance',
    epoch_apply_hash: APPLY_HASH,
    preparation_ids: FIAT_PREPARATION_IDS,
    external_effect_ids: FIAT_EFFECT_IDS,
    stripe_transfers: [providerTransfer, operatorTransfer],
    transfer_root: FIAT_ROOT,
    provider_count: 1,
    provider_liability_au: '850000000000000000',
    provider_paid_au: '840000000000000000',
    operator_fee_liability_au: '150000000000000000',
    operator_fee_retained_au: '140000000000000000',
    gross_liability_au: '1000000000000000000',
    gross_paid_au: '980000000000000000',
    rounding_au: '20000000000000000',
    dust_au: '20000000000000000',
    source_amount_minor: '98',
    destination_totals: [{ currency: 'gbp', amount_minor: '66' }],
    outputs: [providerOutput, operatorOutput],
  };
  const settlementState = {
    type: 'targeted_fiat_settlement',
    ...settlement,
    settled_by: FIAT_SETTLED_BY,
    settled_by_role: 'admin',
  };
  const providerReport = {
    // This is the pre-transfer plan index and may have gaps after sub-minor skips.
    output_index: 3,
    account: {
      id: 'acct_provider',
      default_currency: 'gbp',
      details_submitted: true,
      payouts_enabled: true,
      transfers_enabled: true,
      ready: true,
      attempts: 1,
    },
    fx: {
      liability_au: providerOutput.liability_au,
      paid_au: providerOutput.paid_au,
      rounding_au: providerOutput.rounding_au,
      dust_au: providerOutput.dust_au,
      source_currency: providerOutput.source_currency,
      source_amount_minor: providerOutput.source_amount_minor,
      destination_currency: providerOutput.destination_currency,
      target_destination_amount_minor: '65',
      maximum_destination_amount_minor: '67',
    },
    quote: {
      id: providerOutput.fx_quote_id,
      created: 1000,
      expires_at: 1300,
      lock_duration: 'five_minutes',
      lock_status: 'active',
      to_currency: providerOutput.destination_currency,
      usage: { type: 'transfer', destination: providerOutput.to },
      rates: {
        eur: { exchange_rate: '0.84', base_rate: '1.1' },
        usd: { exchange_rate: '0.7857142857', base_rate: '1' },
      },
    },
    transfer: {
      id: providerTransfer.ref,
      source_amount_minor: 84,
      source_currency: providerOutput.source_currency,
      destination: providerOutput.to,
      destination_payment: providerTransfer.destination_payment,
      balance_transaction: 'txn_transfer',
      fx_quote: providerOutput.fx_quote_id,
      created: 1001,
      reversed: false,
      amount_reversed: 0,
      transfer_group: transferGroup,
      verified: true,
      recovered: false,
      attempts: 1,
    },
    destination_payment: {
      id: providerTransfer.destination_payment,
      source_amount_minor: 84,
      source_currency: providerOutput.source_currency,
      amount_minor: 66,
      gross_amount_minor: 67,
      currency: providerOutput.destination_currency,
      fee_minor: 1,
      net_minor: 66,
      exchange_rate: '1.1000',
      paid: true,
      captured: true,
      source_transfer: providerTransfer.ref,
      balance_transaction: 'txn_destination',
    },
  };
  const operatorReport = {
    output_index: null,
    kind: 'platform_balance',
    platform_account: 'acct_platform',
    source_currency: operatorOutput.source_currency,
    source_amount_minor: operatorOutput.source_amount_minor,
    liability_au: operatorOutput.liability_au,
    retained_au: operatorOutput.paid_au,
    dust_au: operatorOutput.dust_au,
    quote: null,
  };
  const reconciliation = {
    denom: 'au_usd',
    provider_liability_au: settlement.provider_liability_au,
    provider_paid_au: settlement.provider_paid_au,
    operator_fee_liability_au: settlement.operator_fee_liability_au,
    operator_fee_retained_au: settlement.operator_fee_retained_au,
    gross_liability_au: settlement.gross_liability_au,
    gross_paid_au: settlement.gross_paid_au,
    rounding_au: settlement.rounding_au,
    dust_au: settlement.dust_au,
    provider_source_minor_by_currency: { eur: '84' },
    provider_destination_minor_by_currency: { gbp: '66' },
    operator_retained_minor_by_currency: { eur: '14' },
    operator_fee_mechanism: 'retained_platform_balance',
    provider_output_count: 1,
    verified_transfer_count: 1,
    all_provider_transfers_verified: true,
  };
  const finalReport = {
    ok: true,
    epoch: 7,
    submitted: true,
    already_settled: null,
    nothing_to_settle: false,
    no_work: false,
    carry_forward: false,
    settlement,
    settlement_state: settlementState,
    platform_account: {
      id: 'acct_platform',
      default_currency: 'eur',
      livemode: false,
      attempts: 1,
    },
    stripe_transfers: [providerReport, operatorReport],
    payout_preparations: FIAT_PREPARATION_IDS.map((economic_op_id, output_index) => ({
      type: 'targeted_payout_preparation',
      economic_op_id,
      rail: 'fiat',
      epoch: 7,
      epoch_apply_hash: APPLY_HASH,
      output_index,
      external_effect_ids: [FIAT_EFFECT_IDS[output_index]],
      consumed: false,
    })),
    reconciliation,
    skipped_providers: [],
  };
  const draftSettlement = {
    ...settlement,
    preparation_ids: undefined,
    external_effect_ids: undefined,
    source_currency: null,
    stripe_transfers: [],
    transfer_root: null,
    provider_paid_au: '0',
    operator_fee_retained_au: '0',
    gross_paid_au: '0',
    rounding_au: settlement.gross_liability_au,
    dust_au: settlement.gross_liability_au,
    source_amount_minor: '0',
    destination_totals: [],
    outputs: [],
  };
  delete draftSettlement.preparation_ids;
  delete draftSettlement.external_effect_ids;
  const planReport = {
    ok: true,
    epoch: 7,
    submitted: false,
    already_settled: null,
    nothing_to_settle: false,
    no_work: false,
    carry_forward: false,
    settlement: draftSettlement,
    skipped_providers: [],
  };
  const noWorkReport = {
    ...planReport,
    nothing_to_settle: true,
    no_work: true,
    carry_forward: true,
    settlement: {
      ...draftSettlement,
      provider_count: 0,
      provider_liability_au: '0',
      operator_fee_liability_au: '0',
      gross_liability_au: '0',
      rounding_au: '0',
      dust_au: '0',
    },
  };
  const blockingReport = clone(planReport);
  blockingReport.skipped_providers = [{ blocking: true }];
  const belowThresholdReport = clone(noWorkReport);
  belowThresholdReport.skipped_providers = [{
    provider: '6'.repeat(64),
    payout_revision: '7'.repeat(64),
    au: '50',
    payout_min_au: '100',
    reason: 'liability is below canonical payout_min_au and remains carried forward',
    blocking: false,
  }];
  const staleReport = clone(planReport);
  staleReport.settlement.epoch = 6;

  const variants = { success: finalReport };
  variants.bad_source_total = clone(finalReport);
  variants.bad_source_total.settlement.source_amount_minor = '99';
  variants.bad_source_total.settlement_state.source_amount_minor = '99';
  variants.bad_destination_totals = clone(finalReport);
  variants.bad_destination_totals.settlement.destination_totals[0].amount_minor = '65';
  variants.bad_destination_totals.settlement_state.destination_totals[0].amount_minor = '65';
  variants.bad_provider_au = clone(finalReport);
  variants.bad_provider_au.settlement.outputs[0].paid_au = '830000000000000000';
  variants.bad_provider_au.settlement_state.outputs[0].paid_au = '830000000000000000';
  variants.bad_quote_hash = clone(finalReport);
  variants.bad_quote_hash.settlement.stripe_transfers[0].fx_quote_hash = '0'.repeat(64);
  variants.bad_quote_hash.settlement_state.stripe_transfers[0].fx_quote_hash = '0'.repeat(64);
  variants.bad_transfer_readback = clone(finalReport);
  variants.bad_transfer_readback.stripe_transfers[0].destination_payment.amount_minor = 65;
  variants.bad_payment_schema_missing = clone(finalReport);
  delete variants.bad_payment_schema_missing.stripe_transfers[0].destination_payment.fee_minor;
  variants.bad_payment_schema_extra = clone(finalReport);
  variants.bad_payment_schema_extra.stripe_transfers[0].destination_payment.unexpected = true;
  variants.bad_payment_source_amount = clone(finalReport);
  variants.bad_payment_source_amount.stripe_transfers[0].destination_payment.source_amount_minor = 83;
  variants.bad_payment_source_currency = clone(finalReport);
  variants.bad_payment_source_currency.stripe_transfers[0].destination_payment.source_currency =
    'usd';
  variants.bad_payment_id = clone(finalReport);
  variants.bad_payment_id.stripe_transfers[0].destination_payment.id = 'py_other';
  variants.bad_payment_currency = clone(finalReport);
  variants.bad_payment_currency.stripe_transfers[0].destination_payment.currency = 'eur';
  variants.bad_payment_net_detail = clone(finalReport);
  variants.bad_payment_net_detail.stripe_transfers[0].destination_payment.net_minor = 65;
  variants.bad_payment_gross = clone(finalReport);
  variants.bad_payment_gross.stripe_transfers[0].destination_payment.gross_amount_minor = 68;
  variants.bad_payment_fee = clone(finalReport);
  variants.bad_payment_fee.stripe_transfers[0].destination_payment.fee_minor = 2;
  variants.bad_payment_rate = clone(finalReport);
  variants.bad_payment_rate.stripe_transfers[0].destination_payment.exchange_rate = '1.1001';
  variants.bad_payment_rate_null = clone(finalReport);
  variants.bad_payment_rate_null.stripe_transfers[0].destination_payment.exchange_rate = null;
  variants.bad_payment_rate_number = clone(finalReport);
  variants.bad_payment_rate_number.stripe_transfers[0].destination_payment.exchange_rate = 1.1;
  variants.bad_payment_amount_type = clone(finalReport);
  variants.bad_payment_amount_type.stripe_transfers[0].destination_payment.amount_minor = '66';
  variants.bad_payment_unpaid = clone(finalReport);
  variants.bad_payment_unpaid.stripe_transfers[0].destination_payment.paid = false;
  variants.bad_payment_uncaptured = clone(finalReport);
  variants.bad_payment_uncaptured.stripe_transfers[0].destination_payment.captured = false;
  variants.bad_payment_source_transfer = clone(finalReport);
  variants.bad_payment_source_transfer.stripe_transfers[0].destination_payment.source_transfer =
    'tr_other';
  variants.bad_payment_balance_transaction = clone(finalReport);
  variants.bad_payment_balance_transaction.stripe_transfers[0].destination_payment[
    'balance_transaction'
  ] = 'charge_not_a_balance_transaction';
  variants.bad_applied_quote = clone(finalReport);
  variants.bad_applied_quote.stripe_transfers[0].transfer.fx_quote = null;
  variants.bad_state = clone(finalReport);
  variants.bad_state.settlement_state.transfer_root = '0'.repeat(64);
  variants.bad_platform_source = clone(finalReport);
  variants.bad_platform_source.platform_account.default_currency = 'usd';
  variants.missing_preparation_ids = clone(finalReport);
  delete variants.missing_preparation_ids.settlement.preparation_ids;
  variants.duplicate_preparation_ids = clone(finalReport);
  variants.duplicate_preparation_ids.settlement.preparation_ids[1] =
    variants.duplicate_preparation_ids.settlement.preparation_ids[0];
  variants.missing_effect_ids = clone(finalReport);
  delete variants.missing_effect_ids.settlement.external_effect_ids;
  variants.duplicate_effect_ids = clone(finalReport);
  variants.duplicate_effect_ids.settlement.external_effect_ids[1] =
    variants.duplicate_effect_ids.settlement.external_effect_ids[0];
  variants.missing_preparation_readback = clone(finalReport);
  variants.missing_preparation_readback.payout_preparations.pop();
  variants.mismatched_preparation_readback = clone(finalReport);
  variants.mismatched_preparation_readback.payout_preparations[0].external_effect_ids = [
    '0'.repeat(64),
  ];

  const sameNonUsd = clone(finalReport);
  for (const value of [sameNonUsd.settlement, sameNonUsd.settlement_state]) {
    value.destination_totals = [{ currency: 'eur', amount_minor: '84' }];
    value.outputs[0].destination_currency = 'eur';
    value.outputs[0].destination_amount_minor = '84';
    value.outputs[0].fx_quote_id = 'fxq_provider_eur';
    value.outputs[0].fx_quote_hash = FIAT_EUR_QUOTE_HASH;
    value.stripe_transfers[0].destination_currency = 'eur';
    value.stripe_transfers[0].destination_amount_minor = '84';
    value.stripe_transfers[0].fx_quote_id = 'fxq_provider_eur';
    value.stripe_transfers[0].fx_quote_hash = FIAT_EUR_QUOTE_HASH;
  }
  const sameNonUsdReport = sameNonUsd.stripe_transfers[0];
  sameNonUsdReport.account.default_currency = 'eur';
  sameNonUsdReport.fx.destination_currency = 'eur';
  sameNonUsdReport.fx.target_destination_amount_minor = '84';
  sameNonUsdReport.fx.maximum_destination_amount_minor = '84';
  sameNonUsdReport.quote = {
    id: 'fxq_provider_eur',
    created: 1000,
    expires_at: 1300,
    lock_duration: 'five_minutes',
    lock_status: 'active',
    to_currency: 'eur',
    usage: { type: 'transfer', destination: providerOutput.to },
    rates: { usd: { exchange_rate: '0.92', base_rate: '1' } },
  };
  sameNonUsdReport.transfer.fx_quote = null;
  sameNonUsdReport.destination_payment.source_amount_minor = 84;
  sameNonUsdReport.destination_payment.source_currency = 'eur';
  sameNonUsdReport.destination_payment.amount_minor = 84;
  sameNonUsdReport.destination_payment.gross_amount_minor = 84;
  sameNonUsdReport.destination_payment.currency = 'eur';
  sameNonUsdReport.destination_payment.fee_minor = 0;
  sameNonUsdReport.destination_payment.net_minor = 84;
  sameNonUsdReport.destination_payment.exchange_rate = null;
  sameNonUsd.reconciliation.provider_destination_minor_by_currency = { eur: '84' };
  variants.same_currency_non_usd = sameNonUsd;

  variants.same_non_usd_missing_quote = clone(sameNonUsd);
  for (const value of [
    variants.same_non_usd_missing_quote.settlement,
    variants.same_non_usd_missing_quote.settlement_state,
  ]) {
    value.outputs[0].fx_quote_id = null;
    value.outputs[0].fx_quote_hash = null;
    value.stripe_transfers[0].fx_quote_id = null;
    value.stripe_transfers[0].fx_quote_hash = null;
  }
  variants.same_non_usd_missing_readback_quote = clone(sameNonUsd);
  variants.same_non_usd_missing_readback_quote.stripe_transfers[0].quote = null;
  variants.same_non_usd_unbound_quote = clone(sameNonUsd);
  variants.same_non_usd_unbound_quote.stripe_transfers[0].quote.usage.destination =
    'acct_other';
  variants.same_non_usd_bad_quote_hash = clone(sameNonUsd);
  for (const value of [
    variants.same_non_usd_bad_quote_hash.settlement,
    variants.same_non_usd_bad_quote_hash.settlement_state,
  ]) {
    value.outputs[0].fx_quote_hash = '0'.repeat(64);
    value.stripe_transfers[0].fx_quote_hash = '0'.repeat(64);
  }
  variants.same_non_usd_applied_transfer_quote = clone(sameNonUsd);
  variants.same_non_usd_applied_transfer_quote.stripe_transfers[0].transfer.fx_quote =
    'fxq_provider_eur';
  variants.same_non_usd_missing_transfer_quote = clone(sameNonUsd);
  delete variants.same_non_usd_missing_transfer_quote.stripe_transfers[0].transfer.fx_quote;
  variants.same_non_usd_payment_rate = clone(sameNonUsd);
  variants.same_non_usd_payment_rate.stripe_transfers[0].destination_payment.exchange_rate = '1';

  const directUsd = clone(sameNonUsd);
  for (const value of [directUsd.settlement, directUsd.settlement_state]) {
    value.source_currency = 'usd';
    value.destination_totals = [{ currency: 'usd', amount_minor: '84' }];
    value.outputs[0].source_currency = 'usd';
    value.outputs[0].destination_currency = 'usd';
    value.outputs[0].fx_quote_id = null;
    value.outputs[0].fx_quote_hash = null;
    value.outputs[1].source_currency = 'usd';
    value.stripe_transfers[0].source_currency = 'usd';
    value.stripe_transfers[0].destination_currency = 'usd';
    value.stripe_transfers[0].fx_quote_id = null;
    value.stripe_transfers[0].fx_quote_hash = null;
    value.stripe_transfers[1].source_currency = 'usd';
  }
  directUsd.platform_account.default_currency = 'usd';
  directUsd.stripe_transfers[0].account.default_currency = 'usd';
  directUsd.stripe_transfers[0].fx.source_currency = 'usd';
  directUsd.stripe_transfers[0].fx.destination_currency = 'usd';
  directUsd.stripe_transfers[0].quote = null;
  directUsd.stripe_transfers[0].transfer.source_currency = 'usd';
  directUsd.stripe_transfers[0].transfer.fx_quote = null;
  directUsd.stripe_transfers[0].destination_payment.source_currency = 'usd';
  directUsd.stripe_transfers[0].destination_payment.currency = 'usd';
  directUsd.stripe_transfers[1].source_currency = 'usd';
  directUsd.reconciliation.provider_source_minor_by_currency = { usd: '84' };
  directUsd.reconciliation.provider_destination_minor_by_currency = { usd: '84' };
  directUsd.reconciliation.operator_retained_minor_by_currency = { usd: '14' };
  variants.direct_usd_null_quotes = directUsd;

  variants.direct_usd_absent_quotes = clone(directUsd);
  for (const value of [
    variants.direct_usd_absent_quotes.settlement,
    variants.direct_usd_absent_quotes.settlement_state,
  ]) {
    delete value.outputs[0].fx_quote_id;
    delete value.outputs[0].fx_quote_hash;
    delete value.stripe_transfers[0].fx_quote_id;
    delete value.stripe_transfers[0].fx_quote_hash;
  }
  delete variants.direct_usd_absent_quotes.stripe_transfers[0].quote;
  delete variants.direct_usd_absent_quotes.stripe_transfers[0].transfer.fx_quote;

  variants.direct_usd_with_quote = clone(directUsd);
  for (const value of [
    variants.direct_usd_with_quote.settlement,
    variants.direct_usd_with_quote.settlement_state,
  ]) {
    value.outputs[0].fx_quote_id = 'fxq_provider';
    value.outputs[0].fx_quote_hash = FIAT_QUOTE_HASH;
    value.stripe_transfers[0].fx_quote_id = 'fxq_provider';
    value.stripe_transfers[0].fx_quote_hash = FIAT_QUOTE_HASH;
  }

  variants.direct_usd_quote_readback = clone(directUsd);
  variants.direct_usd_quote_readback.stripe_transfers[0].quote = clone(providerReport.quote);
  variants.direct_usd_bad_transfer = clone(directUsd);
  variants.direct_usd_bad_transfer.stripe_transfers[0].transfer.source_amount_minor = 83;
  variants.direct_usd_bad_payment = clone(directUsd);
  variants.direct_usd_bad_payment.stripe_transfers[0].destination_payment.amount_minor = 83;
  variants.direct_usd_payment_rate = clone(directUsd);
  variants.direct_usd_payment_rate.stripe_transfers[0].destination_payment.exchange_rate = '1';

  writeJson(path.join(fixtureDir, 'plan.json'), planReport);
  writeJson(path.join(fixtureDir, 'no_work.json'), noWorkReport);
  writeJson(path.join(fixtureDir, 'below_threshold.json'), belowThresholdReport);
  writeJson(path.join(fixtureDir, 'blocking.json'), blockingReport);
  writeJson(path.join(fixtureDir, 'stale_epoch.json'), staleReport);
  for (const [name, value] of Object.entries(variants)) {
    writeJson(path.join(fixtureDir, `${name}.json`), value);
  }
  return fixtureDir;
}

function writeV17FiatFixtures(root) {
  const fixtureDir = path.join(root, 'fiat-v17-fixtures');
  fs.mkdirSync(fixtureDir, { recursive: true });
  const economicOpId = '1'.repeat(64);
  const attemptId = '2'.repeat(64);
  const outputsRoot = '3'.repeat(64);
  const carryRoot = '4'.repeat(64);
  const planRoot = '5'.repeat(64);
  const admin = '6'.repeat(64);
  const adminSig = '7'.repeat(128);

  const plan = {
    op: 'prepare_targeted_payout_epoch',
    contract_version: 17,
    rail: 'fiat',
    epoch: 7,
    at: 1000,
    epoch_apply_hash: APPLY_HASH,
    snapshot_signed_length: 44,
    outcome: 'payouts',
    outputs: [{
      role: 'provider',
      provider: FIAT_PROVIDER,
      payout_revision: FIAT_PAYOUT_REVISION,
      to: 'acct_provider',
      liability_au: '100',
      paid_au: '100',
      dust_au: '0',
      source_currency: 'usd',
      source_amount_minor: '1',
      destination_currency: 'usd',
      destination_amount_min_minor: '1',
      destination_amount_max_minor: '1',
      economic_op_id: economicOpId,
      output_index: 0,
    }],
    carry: [],
    outputs_root: outputsRoot,
    carry_root: carryRoot,
    plan_root: planRoot,
    admin,
    admin_sig: adminSig,
  };
  const planRecord = {
    type: 'targeted_payout_epoch_plan',
    rail: 'fiat',
    epoch: 7,
    plan_root: planRoot,
    value: clone(plan),
  };
  const outputSettlement = {
    type: 'targeted_fiat_output_settlement',
    rail: 'fiat',
    epoch: 7,
    economic_op_id: economicOpId,
    value: {
      op: 'settle_targeted_fiat_output',
      rail: 'fiat',
      epoch: 7,
      epoch_apply_hash: APPLY_HASH,
      plan_root: planRoot,
      economic_op_id: economicOpId,
      output_index: 0,
      attempt_id: attemptId,
    },
  };
  const close = {
    type: 'targeted_payout_epoch_close',
    rail: 'fiat',
    epoch: 7,
    plan_root: planRoot,
    outcome: 'payouts',
    output_count: 1,
    carry_count: 0,
    outputs_root: outputsRoot,
    carry_root: carryRoot,
    value: {
      op: 'close_targeted_payout_epoch',
      rail: 'fiat',
      epoch: 7,
      epoch_apply_hash: APPLY_HASH,
      plan_root: planRoot,
    },
  };
  const report = (settlement, submitted, result = null) => ({
    ok: true,
    submitted,
    nothing_to_settle: settlement.outcome === 'no_work',
    already_settled: null,
    no_work: settlement.outcome === 'no_work',
    carry_forward: settlement.outcome === 'carry',
    epoch: 7,
    settlement,
    feature: result?.plan ?? null,
    feature_result: result,
    settlement_state: result?.close ?? null,
    payout_preparations: [],
    stripe_transfers: [],
    planned_liabilities: settlement.outputs,
    skipped_providers: [],
  });
  const finalReport = report(plan, true, {
    plan: planRecord,
    outputs: [outputSettlement],
    close,
  });

  const emptyPlan = clone(plan);
  emptyPlan.outcome = 'no_work';
  emptyPlan.outputs = [];
  emptyPlan.outputs_root = '8'.repeat(64);
  emptyPlan.plan_root = '9'.repeat(64);
  const emptyPlanRecord = {
    ...planRecord,
    plan_root: emptyPlan.plan_root,
    value: clone(emptyPlan),
  };
  const emptyClose = {
    ...close,
    plan_root: emptyPlan.plan_root,
    outcome: 'no_work',
    output_count: 0,
    outputs_root: emptyPlan.outputs_root,
    value: {
      ...close.value,
      plan_root: emptyPlan.plan_root,
    },
  };
  const emptyFinal = report(emptyPlan, true, {
    plan: emptyPlanRecord,
    outputs: [],
    close: emptyClose,
  });

  const carryPlan = clone(emptyPlan);
  carryPlan.outcome = 'carry';
  carryPlan.carry = [{
    provider: FIAT_PROVIDER,
    payout_revision: FIAT_PAYOUT_REVISION,
    liability_au: '50',
    held_au: '0',
    payable_au: '50',
    payout_min_au: '100',
    reason: 'below_payout_minimum',
  }];
  carryPlan.carry_root = 'a'.repeat(64);
  carryPlan.plan_root = 'b'.repeat(64);
  const carryPlanRecord = {
    ...planRecord,
    plan_root: carryPlan.plan_root,
    value: clone(carryPlan),
  };
  const carryClose = {
    ...emptyClose,
    plan_root: carryPlan.plan_root,
    outcome: 'carry',
    carry_count: 1,
    carry_root: carryPlan.carry_root,
    value: {
      ...emptyClose.value,
      plan_root: carryPlan.plan_root,
    },
  };
  const carrySkipped = [{
    provider: FIAT_PROVIDER,
    payout_revision: FIAT_PAYOUT_REVISION,
    au: '50',
    payout_min_au: '100',
    reason: 'liability is below canonical payout_min_au and remains carried forward',
    blocking: false,
  }];
  const carryPlanReport = report(carryPlan, false);
  carryPlanReport.skipped_providers = carrySkipped;
  const carryFinal = report(carryPlan, true, {
    plan: carryPlanRecord,
    outputs: [],
    close: carryClose,
  });
  carryFinal.skipped_providers = carrySkipped;

  const blocking = report(emptyPlan, false);
  blocking.skipped_providers = [{
    provider: FIAT_PROVIDER,
    payout_revision: FIAT_PAYOUT_REVISION,
    au: '100',
    reason: 'payout target unavailable',
    blocking: true,
  }];
  const stale = report(clone(plan), false);
  stale.epoch = 6;
  stale.settlement.epoch = 6;

  const variants = {
    final: finalReport,
    bad_attempt_id: clone(finalReport),
    missing_output_settlement: clone(finalReport),
    mismatched_output: clone(finalReport),
    missing_close: clone(finalReport),
    mismatched_close: clone(finalReport),
  };
  variants.bad_attempt_id.feature_result.outputs[0].value.attempt_id = 'bad';
  variants.missing_output_settlement.feature_result.outputs = [];
  variants.mismatched_output.feature_result.outputs[0].economic_op_id = '0'.repeat(64);
  delete variants.missing_close.feature_result.close;
  variants.missing_close.settlement_state = null;
  variants.mismatched_close.feature_result.close.plan_root = '0'.repeat(64);
  variants.mismatched_close.settlement_state =
    variants.mismatched_close.feature_result.close;

  writeJson(path.join(fixtureDir, 'plan.json'), report(plan, false));
  writeJson(path.join(fixtureDir, 'no_work-plan.json'), report(emptyPlan, false));
  writeJson(path.join(fixtureDir, 'no_work-final.json'), emptyFinal);
  writeJson(path.join(fixtureDir, 'below_threshold-plan.json'), carryPlanReport);
  writeJson(path.join(fixtureDir, 'below_threshold-final.json'), carryFinal);
  writeJson(path.join(fixtureDir, 'blocking.json'), blocking);
  writeJson(path.join(fixtureDir, 'stale_epoch.json'), stale);
  for (const [name, value] of Object.entries(variants)) {
    writeJson(path.join(fixtureDir, `${name}.json`), value);
  }
  return fixtureDir;
}

function writeV17TnkFixtures(root) {
  const fixtureDir = path.join(root, 'tnk-v17-fixtures');
  fs.mkdirSync(fixtureDir, { recursive: true });
  const economicOpId = 'c'.repeat(64);
  const outputsRoot = 'd'.repeat(64);
  const carryRoot = 'e'.repeat(64);
  const planRoot = 'f'.repeat(64);
  const plan = {
    op: 'prepare_targeted_payout_epoch',
    contract_version: 17,
    rail: 'tnk',
    epoch: 7,
    at: 1000,
    epoch_apply_hash: APPLY_HASH,
    snapshot_signed_length: 44,
    outcome: 'payouts',
    outputs: [{
      role: 'provider',
      provider: '8'.repeat(64),
      payout_revision: '9'.repeat(64),
      to: 'trac1provider',
      liability_au: '10',
      paid_au: '10',
      economic_op_id: economicOpId,
      output_index: 0,
    }],
    carry: [],
    outputs_root: outputsRoot,
    carry_root: carryRoot,
    plan_root: planRoot,
    admin: '6'.repeat(64),
    admin_sig: '7'.repeat(128),
  };
  const planRecord = {
    type: 'targeted_payout_epoch_plan',
    rail: 'tnk',
    epoch: 7,
    plan_root: planRoot,
    value: clone(plan),
  };
  const outputSettlement = {
    type: 'targeted_tnk_output_settlement',
    rail: 'tnk',
    epoch: 7,
    economic_op_id: economicOpId,
    value: {
      op: 'settle_targeted_tnk_output',
      rail: 'tnk',
      epoch: 7,
      epoch_apply_hash: APPLY_HASH,
      plan_root: planRoot,
      economic_op_id: economicOpId,
      output_index: 0,
    },
  };
  const close = {
    type: 'targeted_payout_epoch_close',
    rail: 'tnk',
    epoch: 7,
    plan_root: planRoot,
    outcome: 'payouts',
    output_count: 1,
    carry_count: 0,
    outputs_root: outputsRoot,
    carry_root: carryRoot,
    value: {
      op: 'close_targeted_payout_epoch',
      rail: 'tnk',
      epoch: 7,
      epoch_apply_hash: APPLY_HASH,
      plan_root: planRoot,
    },
  };
  const report = (settlement, submitted, result = null) => ({
    ok: true,
    submitted,
    already_settled: null,
    no_work: settlement.outputs.length === 0,
    carry_forward: settlement.outputs.length === 0,
    epoch: 7,
    settlement,
    feature: result?.plan ?? null,
    feature_result: result,
    settlement_state: result?.close ?? null,
    payout_preparations: [],
    msb_outputs: settlement.outputs,
    msb_transfers: [],
    skipped_providers: [],
  });
  const finalReport = report(plan, true, {
    plan: planRecord,
    outputs: [outputSettlement],
    close,
  });

  const emptyPlan = clone(plan);
  emptyPlan.outcome = 'no_work';
  emptyPlan.outputs = [];
  emptyPlan.outputs_root = '1'.repeat(64);
  emptyPlan.plan_root = '2'.repeat(64);
  const emptyClose = {
    ...close,
    plan_root: emptyPlan.plan_root,
    outcome: 'no_work',
    output_count: 0,
    outputs_root: emptyPlan.outputs_root,
    value: {
      ...close.value,
      plan_root: emptyPlan.plan_root,
    },
  };
  const emptyFinal = report(emptyPlan, true, {
    plan: { ...planRecord, plan_root: emptyPlan.plan_root, value: clone(emptyPlan) },
    outputs: [],
    close: emptyClose,
  });

  const carryPlan = clone(emptyPlan);
  carryPlan.outcome = 'carry';
  carryPlan.carry = [{
    provider: '8'.repeat(64),
    payout_revision: '9'.repeat(64),
    liability_au: '50',
    held_au: '0',
    payable_au: '50',
    payout_min_au: '100',
    reason: 'below_payout_minimum',
  }];
  carryPlan.carry_root = '3'.repeat(64);
  carryPlan.plan_root = '4'.repeat(64);
  const carryClose = {
    ...emptyClose,
    plan_root: carryPlan.plan_root,
    outcome: 'carry',
    carry_count: 1,
    carry_root: carryPlan.carry_root,
    value: { ...emptyClose.value, plan_root: carryPlan.plan_root },
  };
  const carrySkipped = [{
    provider: '8'.repeat(64),
    payout_revision: '9'.repeat(64),
    au: '50',
    payout_min_au: '100',
    reason: 'liability is below canonical payout_min_au and remains carried forward',
    blocking: false,
  }];
  const carryPlanReport = report(carryPlan, false);
  carryPlanReport.skipped_providers = carrySkipped;
  const carryFinal = report(carryPlan, true, {
    plan: { ...planRecord, plan_root: carryPlan.plan_root, value: clone(carryPlan) },
    outputs: [],
    close: carryClose,
  });
  carryFinal.skipped_providers = carrySkipped;

  const payableOmitted = clone(carryPlanReport);
  payableOmitted.skipped_providers[0].au = '100';
  const variants = {
    final: finalReport,
    missing_output_settlement: clone(finalReport),
    mismatched_output: clone(finalReport),
    missing_close: clone(finalReport),
  };
  variants.missing_output_settlement.feature_result.outputs = [];
  variants.mismatched_output.feature_result.outputs[0].economic_op_id = '0'.repeat(64);
  delete variants.missing_close.feature_result.close;
  variants.missing_close.settlement_state = null;

  writeJson(path.join(fixtureDir, 'plan.json'), report(plan, false));
  writeJson(path.join(fixtureDir, 'no_work-plan.json'), report(emptyPlan, false));
  writeJson(path.join(fixtureDir, 'no_work-final.json'), emptyFinal);
  writeJson(path.join(fixtureDir, 'below_threshold-plan.json'), carryPlanReport);
  writeJson(path.join(fixtureDir, 'below_threshold-final.json'), carryFinal);
  writeJson(path.join(fixtureDir, 'payable_omitted.json'), payableOmitted);
  for (const [name, value] of Object.entries(variants)) {
    writeJson(path.join(fixtureDir, `${name}.json`), value);
  }
  return fixtureDir;
}

function harness({ bundle = true, emptySeal = !bundle } = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-payout-worker-'));
  const state = path.join(root, 'settlement');
  const spool = path.join(state, 'tap');
  const bin = path.join(root, 'bin');
  const log = path.join(root, 'mayhem.log');
  const applyState = path.join(root, 'apply-state.json');
  const tapSettlementState = path.join(root, 'tap-settlement-state.json');
  const tapLiabilityState = path.join(root, 'tap-liability-state.json');
  const tapRemainingObservation = path.join(root, 'tap-remaining-observation.json');
  const payoutEventLog = path.join(root, 'payout-events.log');
  const externalEffectState = path.join(root, 'external-effects');
  const emptySealDefault = emptySeal ? '1' : '0';
  fs.mkdirSync(bin, { recursive: true });
  const fiatFixtures = writeV17FiatFixtures(root);
  const tnkFixtures = writeV17TnkFixtures(root);
  writeJson(applyState, {
    key: 'epoch/apply/state',
    confirmed: true,
    value: {
      updated_epoch: 7,
      pending_epoch: null,
      last_apply_hash: APPLY_HASH.toUpperCase(),
      last_settlement_unix: 900,
      last_receipt_commit_hash: EPOCH_COMMIT_HASH,
      last_receipt_index_count: 3,
      last_receipt_index_revision: 3,
      last_receipt_index_page_count: 1,
      last_receipt_index_updated_at: EPOCH_APPLIED_AT,
      last_receipt_allocation_count: 3,
      updated_at: EPOCH_APPLIED_AT,
    },
  });
  const nowFile = path.join(root, 'now');
  fs.writeFileSync(nowFile, '1000\n');

  writeExecutable(path.join(bin, 'curl'), `#!/usr/bin/env bash
if [[ "$*" == *"prefix=payout/liability/tnk/"* && "\${MOCK_TNK_OUTSTANDING:-0}" == "1" ]]; then
  printf '%s\\n' '{"values":[{"key":"payout/liability/tnk/provider/revision","value":{"total_au":"10","paid_cum_au":"0"}}]}'
elif [[ "$*" == *"key=settle/targeted/tap/"* ]]; then
  if [[ -f "$MOCK_TAP_SETTLEMENT_STATE" ]]; then
    cat "$MOCK_TAP_SETTLEMENT_STATE"
  else
    printf '%s\\n' '{"value":null}'
  fi
elif [[ "$*" == *"key=fee/tnk/cum"* ]]; then
  printf '%s\\n' '{"value":{"cum_au":"0","swept_cum_au":"0"}}'
elif [[ "$*" == *"key=epoch/commit/7"* ]]; then
  if [[ "\${MOCK_STALE_ROOT:-0}" == "1" ]]; then
    printf '%s\\n' '{"key":"epoch/commit/7","confirmed":true,"value":{"type":"epoch_commit","epoch":7,"roots":{"dep":"${'1'.repeat(64)}","use":"${'2'.repeat(64)}","earn":"${'9'.repeat(64)}","fee":"${'4'.repeat(64)}","price":"${'5'.repeat(64)}"},"totals":{"dep_count":0,"dep_au":"0","use_count":3,"use_au":"30","provider_count":3,"earn_au":"20","fee_au":"5","fee_cum_au":"5","burn_au":"1","burn_cum_au":"1","price_count":0},"status":"provisional","commit_hash":"${EPOCH_COMMIT_HASH}","at":900}}'
  else
    printf '%s\\n' '{"key":"epoch/commit/7","confirmed":true,"value":{"type":"epoch_commit","epoch":7,"roots":{"dep":"${'1'.repeat(64)}","use":"${'2'.repeat(64)}","earn":"${'3'.repeat(64)}","fee":"${'4'.repeat(64)}","price":"${'5'.repeat(64)}"},"totals":{"dep_count":0,"dep_au":"0","use_count":3,"use_au":"30","provider_count":3,"earn_au":"20","fee_au":"5","fee_cum_au":"5","burn_au":"1","burn_cum_au":"1","price_count":0},"status":"provisional","commit_hash":"${EPOCH_COMMIT_HASH}","at":900}}'
  fi
elif [[ "$*" == *"key=epoch/apply-anchor/7"* ]]; then
  if [[ "\${MOCK_STALE_ANCHOR:-0}" == "1" ]]; then
    printf '%s\\n' '{"key":"epoch/apply-anchor/7","confirmed":true,"value":{"type":"epoch_apply_anchor","epoch":7,"apply_hash":"${NEXT_APPLY_HASH}","settlement_unix":900,"applied_at":"${EPOCH_APPLIED_AT}"}}'
  else
    printf '%s\\n' '{"key":"epoch/apply-anchor/7","confirmed":true,"value":{"type":"epoch_apply_anchor","epoch":7,"apply_hash":"${APPLY_HASH}","settlement_unix":900,"applied_at":"${EPOCH_APPLIED_AT}"}}'
  fi
elif [[ "$*" == *"key=epoch/seal/7"* ]]; then
  if [[ "\${MOCK_EMPTY_SEAL:-${emptySealDefault}}" == "1" ]]; then
    printf '%s\\n' '{"value":{"type":"epoch_empty_seal","epoch":7,"seal_hash":"${APPLY_HASH}","totals":{"debited_au":"0","earned_au":"0","fee_au":"0","burn_au":"0"}}}'
  else
    printf '%s\\n' '{"value":null}'
  fi
elif [[ "$*" == *"key=epoch/seal/8"* ]]; then
  if [[ "\${MOCK_EMPTY_SEAL_8:-0}" == "1" ]]; then
    python3 - "$MOCK_TAP_LIABILITY_STATE" "$MOCK_TAP_REMAINING_OBSERVATION" <<'PY'
import json, os, sys
liability = json.load(open(sys.argv[1]))
total = int(liability["total_au"])
paid = int(liability["paid_cum_au"])
if total != 150 or paid != 100:
    raise SystemExit("current epoch derivation ran before prior TAP evidence reconciliation")
with open(f"{sys.argv[2]}.tmp", "w") as out:
    json.dump({"total_au": str(total), "paid_cum_au": str(paid), "remaining_au": str(total - paid)}, out)
    out.write("\\n")
os.replace(f"{sys.argv[2]}.tmp", sys.argv[2])
PY
    printf '%s\\n' '{"value":{"type":"epoch_empty_seal","epoch":8,"seal_hash":"${NEXT_APPLY_HASH}","totals":{"debited_au":"0","earned_au":"0","fee_au":"0","burn_au":"0"}}}'
  else
    printf '%s\\n' '{"value":null}'
  fi
else
  cat "$MOCK_APPLY_STATE"
fi
`);
  writeExecutable(path.join(bin, 'date'), `#!/usr/bin/env bash
if [[ "\${1:-}" == "+%s" ]]; then
  cat "$MOCK_NOW_FILE"
else
  /bin/date "$@"
fi
`);
writeExecutable(path.join(bin, 'mock-mayhem'), `#!/usr/bin/env bash
set -euo pipefail
printf '%s\\n' "$*" >>"$MOCK_MAYHEM_LOG"
record_external_effect() {
  local rail="$1"
  mkdir -p "$MOCK_EXTERNAL_EFFECT_STATE"
  printf '%s\\n' "$rail:canonical-preparation-readback" >>"$MOCK_PAYOUT_EVENT_LOG"
  if [[ ! -f "$MOCK_EXTERNAL_EFFECT_STATE/$rail.effect" ]]; then
    printf '%s\\n' "$rail:external-transfer" >>"$MOCK_PAYOUT_EVENT_LOG"
    : >"$MOCK_EXTERNAL_EFFECT_STATE/$rail.effect"
    if [[ "\${MOCK_CRASH_AFTER_EXTERNAL_TRANSFER:-}" == "$rail" ]]; then
      printf '%s\\n' "simulated crash after $rail external transfer" >&2
      exit 75
    fi
  else
    printf '%s\\n' "$rail:external-transfer-recovered" >>"$MOCK_PAYOUT_EVENT_LOG"
  fi
}
rail="\${2:-}"
submit=0
contract_submit=0
sim=0
checkpoint_file=""
previous=""
for arg in "$@"; do
  [[ "$arg" == "--submit-transfer" ]] && submit=1
  [[ "$arg" == "--submit" ]] && contract_submit=1
  [[ "$arg" == "--sim" ]] && sim=1
  [[ "$previous" == "--checkpoint-file" ]] && checkpoint_file="$arg"
  previous="$arg"
done
if [[ "$rail" == "fiat-settlement" ]]; then
  mode="\${MOCK_FIAT_MODE:-success}"
  if [[ "$mode" == "blocking" || "$mode" == "stale_epoch" ]]; then
    cat "$MOCK_FIAT_FIXTURES/$mode.json"
  elif [[ "$mode" == "no_work" || "$mode" == "below_threshold" ]]; then
    if (( submit == 1 )); then
      cat "$MOCK_FIAT_FIXTURES/$mode-final.json"
    else
      cat "$MOCK_FIAT_FIXTURES/$mode-plan.json"
    fi
  elif (( submit == 1 )); then
    record_external_effect fiat
    cat "$MOCK_FIAT_FIXTURES/\${mode/success/final}.json"
  else
    cat "$MOCK_FIAT_FIXTURES/plan.json"
  fi
elif [[ "$rail" == "tnk-settlement" ]]; then
  mode="\${MOCK_TNK_MODE:-success}"
  if [[ "$mode" == "no_work" || "$mode" == "below_threshold" ]]; then
    if (( submit == 1 )); then
      cat "$MOCK_TNK_FIXTURES/$mode-final.json"
    else
      cat "$MOCK_TNK_FIXTURES/$mode-plan.json"
    fi
  elif [[ "$mode" == "payable_omitted" ]]; then
    cat "$MOCK_TNK_FIXTURES/payable_omitted.json"
  elif (( submit == 1 )); then
    record_external_effect tnk
    cat "$MOCK_TNK_FIXTURES/\${mode/success/final}.json"
  else
    cat "$MOCK_TNK_FIXTURES/plan.json"
  fi
elif [[ "$rail" == "tap-settlement" ]]; then
  [[ -f "$checkpoint_file" ]] || {
    printf '%s\\n' "missing TAP checkpoint file" >&2
    exit 2
  }
  python3 - "$checkpoint_file" "$MOCK_TAP_SETTLEMENT_STATE" "$contract_submit" "$sim" <<'PY'
import hashlib, json, os, sys
checkpoint_path, state_path, submit_raw, sim_raw = sys.argv[1:]
checkpoint = json.load(open(checkpoint_path))
submitted = submit_raw == "1" and sim_raw != "1"
sim = submit_raw == "1" and sim_raw == "1"
key_hash = hashlib.sha256(
    json.dumps(checkpoint, sort_keys=True, separators=(",", ":")).encode()
).hexdigest()
feature = {
    "feature": "mayhem",
    "key": f"settle/targeted/tap/{checkpoint['epoch']}/{key_hash}",
    "value": checkpoint,
}
if submitted:
    record = {
        "type": "targeted_tap_settlement",
        **checkpoint,
        "settled_by": "9" * 64,
        "settled_by_role": "admin",
    }
    existing = json.load(open(state_path)).get("value") if os.path.exists(state_path) else None
    if existing is not None and existing != record:
        raise SystemExit("conflicting canonical TAP settlement retry")
    if existing is None:
        liability_path = os.environ.get("MOCK_TAP_LIABILITY_STATE")
        if liability_path and os.path.exists(liability_path):
            liability = json.load(open(liability_path))
            output = checkpoint["outputs"][0]
            if (
                liability.get("provider") != output.get("provider")
                or liability.get("payout_revision") != output.get("payout_revision")
            ):
                raise SystemExit("TAP checkpoint does not match canonical liability identity")
            paid = int(liability["paid_cum_au"]) + int(output["paid_au"])
            if paid > int(liability["total_au"]):
                raise SystemExit("TAP checkpoint overpays canonical liability")
            liability["paid_cum_au"] = str(paid)
            with open(f"{liability_path}.tmp", "w") as out:
                json.dump(liability, out)
                out.write("\\n")
            os.replace(f"{liability_path}.tmp", liability_path)
        with open(f"{state_path}.tmp", "w") as out:
            json.dump({"value": record}, out)
            out.write("\\n")
        os.replace(f"{state_path}.tmp", state_path)
report = {
    "ok": True,
    "submitted": submitted,
    "feature_type": "settleTargetedTap",
    "feature": feature,
    "copy_paste": {"peer_rpc": "mock"},
}
if sim:
    report["sim"] = True
print(json.dumps(report))
PY
else
  printf '%s\\n' "unexpected mock mayhem command: $*" >&2
  exit 2
fi
`);

  if (bundle) {
    const epochDir = path.join(state, 'epochs/epoch-7');
    fs.mkdirSync(epochDir, { recursive: true });
    const entry = (rail, id) => ({
      rail,
      receipt: {
        body: {
          rail,
          session_id: `${rail}-${id}`,
        },
      },
    });
    const receipts = [
      entry('fiat', 1),
      entry('tap', 2),
      entry('tnk', 3),
    ];
    const canonicalReceipts = {
      schema_version: 1,
      type: 'canonical_epoch_receipt_snapshot',
      settlement_epoch: 7,
      metadata: {
        type: 'canonical_receipt_epoch_index',
        epoch: 7,
        count: receipts.length,
        page_size: 128,
        page_count: 1,
        revision: receipts.length,
        updated_at: 'f'.repeat(64),
      },
      identities: receipts.map((_, index) => ({
        billing_id: (index + 1).toString(16).padStart(64, '0'),
        billing_attempt: 0,
      })),
      heads: receipts,
      snapshot_sha256: 'e'.repeat(64),
    };
    const canonicalReceiptsRaw = writeJson(
      path.join(epochDir, 'canonical-receipts.json'),
      canonicalReceipts
    );
    const bundleRaw = writeJson(path.join(epochDir, 'epoch-bundle.json'), {
      epoch: 7,
      params: { fee_bps: 1500 },
      receipts,
      receipt_snapshot: canonicalReceipts,
    });
    const recomputed = {
      epoch: 7,
      roots: Object.fromEntries(
        ['dep', 'use', 'earn', 'fee', 'price'].map(
          (key, index) => [key, String(index + 1).repeat(64)]
        )
      ),
      totals: {
        dep_count: 0,
        dep_au: '0',
        use_count: 3,
        use_au: '30',
        provider_count: 3,
        earn_au: '20',
        fee_au: '5',
        fee_cum_au: '5',
        burn_au: '1',
        burn_cum_au: '1',
        price_count: 0,
      },
    };
    const recomputedRaw = writeJson(path.join(epochDir, 'epoch-recomputed.json'), recomputed);
    writeJson(path.join(epochDir, 'epoch-artifact.json'), {
      schema_version: 1,
      type: 'canonical_epoch_artifact',
      rail: 'all',
      rails: ['fiat', 'tap', 'tnk'],
      epoch: 7,
      epoch_apply_hash: APPLY_HASH,
      bundle_sha256: sha256(bundleRaw),
      recomputed_sha256: sha256(recomputedRaw),
      canonical_receipts_sha256: sha256(canonicalReceiptsRaw),
      roots: recomputed.roots,
      totals: recomputed.totals,
    });
  }

  const env = {
    PATH: `${path.dirname(process.execPath)}:${bin}:/usr/bin:/bin:/usr/sbin:/sbin`,
    HOME: path.join(root, 'home'),
    LANG: 'C',
    MAYHEM_BIN: path.join(bin, 'mock-mayhem'),
    MAYHEM_SOURCE_DIR: ROOT,
    MAYHEM_RPC_URL: 'http://mock.invalid/v1',
    MAYHEM_ADMIN_HOME: path.join(root, 'admin-home'),
    MAYHEM_ADMIN_STORE: 'test-admin',
    MAYHEM_CADENCE_STATE_DIR: state,
    MAYHEM_TAP_SETTLEMENT_SPOOL: spool,
    MAYHEM_PAYOUT_LOCK_HELD: '1',
    MAYHEM_PAYOUT_TEST_MODE: '1',
    MAYHEM_PAYOUT_TEST_ROOT: root,
    MOCK_APPLY_STATE: applyState,
    MOCK_MAYHEM_LOG: log,
    MOCK_NOW_FILE: nowFile,
    MOCK_FIAT_FIXTURES: fiatFixtures,
    MOCK_TNK_FIXTURES: tnkFixtures,
    MOCK_TAP_SETTLEMENT_STATE: tapSettlementState,
    MOCK_TAP_LIABILITY_STATE: tapLiabilityState,
    MOCK_TAP_REMAINING_OBSERVATION: tapRemainingObservation,
    MOCK_PAYOUT_EVENT_LOG: payoutEventLog,
    MOCK_EXTERNAL_EFFECT_STATE: externalEffectState,
  };
  return {
    root,
    state,
    spool,
    log,
    nowFile,
    applyState,
    tapSettlementState,
    tapLiabilityState,
    tapRemainingObservation,
    payoutEventLog,
    externalEffectState,
    env,
  };
}

function runWorker(ctx, extraEnv = {}) {
  return spawnSync('bash', [SCRIPT], {
    cwd: ROOT,
    env: { ...ctx.env, ...extraEnv },
    encoding: 'utf8',
  });
}

function logLines(ctx) {
  if (!fs.existsSync(ctx.log)) return [];
  return fs.readFileSync(ctx.log, 'utf8').trim().split('\n').filter(Boolean);
}

function payoutEventLines(ctx) {
  if (!fs.existsSync(ctx.payoutEventLog)) return [];
  return fs.readFileSync(ctx.payoutEventLog, 'utf8').trim().split('\n').filter(Boolean);
}

function rebindArtifact(ctx) {
  const epochDir = path.join(ctx.state, 'epochs/epoch-7');
  const bundleRaw = fs.readFileSync(path.join(epochDir, 'epoch-bundle.json'));
  const recomputedRaw = fs.readFileSync(path.join(epochDir, 'epoch-recomputed.json'));
  const bundle = JSON.parse(bundleRaw);
  writeJson(path.join(epochDir, 'canonical-receipts.json'), bundle.receipt_snapshot);
  const canonicalReceiptsRaw = fs.readFileSync(path.join(epochDir, 'canonical-receipts.json'));
  const recomputed = JSON.parse(recomputedRaw);
  writeJson(path.join(epochDir, 'epoch-artifact.json'), {
    schema_version: 1,
    type: 'canonical_epoch_artifact',
    rail: 'all',
    rails: ['fiat', 'tap', 'tnk'],
    epoch: 7,
    epoch_apply_hash: APPLY_HASH,
    bundle_sha256: sha256(bundleRaw),
    recomputed_sha256: sha256(recomputedRaw),
    canonical_receipts_sha256: sha256(canonicalReceiptsRaw),
    roots: recomputed.roots,
    totals: recomputed.totals,
  });
}

function writeTapReport(ctx, bundleName, overrides = {}) {
  const processed = path.join(ctx.spool, 'processed');
  const bundleRaw = fs.readFileSync(path.join(processed, bundleName));
  const rateLock = {
    type: 'tap_settlement_rate_lock',
    epoch: 7,
    bundle_sha256: '2'.repeat(64),
    denom: 'tap_usd_au',
    tap_usd_au: '1000000000000000000',
    source: 'uniswap_v3_twap',
    rate_ts: 1_000,
    rate_record_key: `rate/tap/1000/${'3'.repeat(64)}`,
    posted_by: '4'.repeat(64),
    posted_by_role: 'admin',
    chain_id: 1,
    token_address: `0x${'5'.repeat(40)}`,
    pool_address: `0x${'6'.repeat(40)}`,
    payment_config_ver: 1,
  };
  const checkpoint = {
    op: 'settle_targeted_tap',
    epoch: 7,
    at: 1_000,
    rail: 'tap',
    epoch_apply_hash: APPLY_HASH,
    chain_id: rateLock.chain_id,
    token_address: rateLock.token_address,
    pool_address: rateLock.pool_address,
    payment_config_ver: rateLock.payment_config_ver,
    tap_rate_lock: rateLock,
    root: TAP_ROOT,
    root_confirmed: true,
    proposal_tx: TAP_TX,
    proposal_block_number: 100,
    proposal_block_hash: `0x${'7'.repeat(64)}`,
    execution_tx: TAP_EXECUTION_TX,
    execution_status: 1,
    execution_block_number: 101,
    execution_block_hash: `0x${'8'.repeat(64)}`,
    finalized_block_number: 113,
    confirmation_depth: 12,
    confirmation_policy: 'finalized-tag',
    cumulative_spent_wei: '14',
    provider_count: 1,
    provider_paid_au: '10',
    provider_tap_wei: '10',
    entries: [
      {
        account: `0x${'0'.repeat(39)}2`,
        cumulative_wei: '4',
      },
      {
        account: TAP_TARGET,
        cumulative_wei: '10',
      },
    ],
    outputs: [{
      provider: TAP_PROVIDER,
      payout_revision: TAP_PAYOUT_REVISION,
      to: TAP_TARGET,
      paid_au: '10',
      tap_wei: '10',
      cumulative_claim_wei: '10',
    }],
  };
  writeJson(
    path.join(processed, `epoch-7-${APPLY_HASH}.settlement.json`),
    {
      rail: 'tap',
      epoch: 7,
      epoch_apply_hash: APPLY_HASH,
      bundle_sha256: sha256(bundleRaw),
      root: TAP_ROOT,
      blocked: false,
      root_confirmed: true,
      posted: true,
      proposal_tx: TAP_TX,
      execution_tx: TAP_EXECUTION_TX,
      tap_rate_lock: rateLock,
      root_confirmation: {
        confirmed: true,
        onchain_epoch: 7,
        onchain_root: TAP_ROOT,
        onchain_cumulative_spent_wei: '14',
        finalized_block_number: 113,
        confirmation_depth: 12,
        confirmation_policy: 'finalized-tag',
        proposal: {
          tx_hash: TAP_TX,
          block_number: 100,
          block_hash: `0x${'7'.repeat(64)}`,
          nonce: '1',
          execute_after: 1_001,
        },
        execution: {
          tx_hash: TAP_EXECUTION_TX,
          status: 1,
          block_number: 101,
          block_hash: `0x${'8'.repeat(64)}`,
          nonce: '1',
        },
      },
      tap_settlement_checkpoint: checkpoint,
      operator_fee: {
        completed: true,
        predicted_claimable_wei: '0',
        remaining_claimable_wei: '0',
      },
      burn: {
        completed: true,
        predicted_claimable_wei: '0',
        remaining_claimable_wei: '0',
      },
      ...overrides,
    }
  );
}

test('payout worker isolates TAP spool work and replays all rails idempotently', (t) => {
  const ctx = harness();
  t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));

  const first = runWorker(ctx);
  assert.equal(first.status, 0, first.stderr);
  const workDir = path.join(ctx.state, `payout/epoch-7-${APPLY_HASH}`);
  const queuedSummary = JSON.parse(fs.readFileSync(path.join(workDir, 'summary.json'), 'utf8'));
  assert.equal(queuedSummary.complete, false);
  assert.deepEqual(
    Object.fromEntries(
      Object.entries(queuedSummary.rails).map(([rail, value]) => [rail, value.complete])
    ),
    { fiat: true, tap: false, tnk: true }
  );

  const ready = fs.readdirSync(path.join(ctx.spool, 'ready'));
  assert.deepEqual(ready, [`epoch-7-${APPLY_HASH}.receipts.json`]);
  const tapBundle = JSON.parse(
    fs.readFileSync(path.join(ctx.spool, 'ready', ready[0]), 'utf8')
  );
  assert.equal(tapBundle.receipts.length, 1);
  assert.equal(tapBundle.receipts[0].rail, 'tap');
  assert.equal(tapBundle.receipts[0].receipt.body.rail, 'tap');
  assert.equal(tapBundle.receipts[0].receipt_epoch, 7);
  assert.equal(tapBundle.rail, 'tap');
  assert.equal(tapBundle.epoch_apply_hash, APPLY_HASH);
  assert.equal(fs.existsSync(path.join(workDir, 'tap.complete')), false);
  assert.equal(fs.existsSync(path.join(workDir, 'tap.produced')), true);

  const firstLog = logLines(ctx);
  assert.equal(firstLog.length, 3);
  assert.equal(firstLog.filter((line) => line.startsWith('admin fiat-settlement')).length, 1);
  assert.equal(firstLog.filter((line) => line.startsWith('admin tnk-settlement')).length, 2);

  const working = path.join(ctx.spool, 'working');
  fs.renameSync(
    path.join(ctx.spool, 'ready', ready[0]),
    path.join(working, ready[0])
  );
  const pendingReplay = runWorker(ctx);
  assert.equal(pendingReplay.status, 0, pendingReplay.stderr);
  assert.deepEqual(logLines(ctx), firstLog);
  assert.equal(fs.readFileSync(path.join(workDir, 'tap.attempts'), 'utf8').trim(), '1');
  assert.deepEqual(fs.readdirSync(path.join(ctx.spool, 'ready')), []);
  assert.deepEqual(fs.readdirSync(working), ready);

  const processed = path.join(ctx.spool, 'processed');
  fs.mkdirSync(processed, { recursive: true });
  fs.renameSync(
    path.join(working, ready[0]),
    path.join(processed, ready[0])
  );
  writeTapReport(ctx, ready[0]);
  const second = runWorker(ctx);
  assert.equal(second.status, 0, second.stderr);
  const secondLog = logLines(ctx);
  assert.equal(secondLog.length, firstLog.length + 2);
  assert.equal(
    secondLog.filter((line) => line.startsWith('admin tap-settlement')).length,
    2
  );
  assert.match(secondLog.at(-2), /--submit --sim/);
  assert.match(secondLog.at(-1), /--submit/);
  assert.doesNotMatch(secondLog.at(-1), /--sim/);
  const settledSummary = JSON.parse(fs.readFileSync(path.join(workDir, 'summary.json'), 'utf8'));
  assert.equal(settledSummary.complete, true);
  assert.equal(settledSummary.rails.tap.result.status, 'settled');
  assert.equal(
    JSON.parse(fs.readFileSync(ctx.tapSettlementState, 'utf8')).value.execution_tx,
    TAP_EXECUTION_TX
  );
  assert.deepEqual(fs.readdirSync(path.join(ctx.spool, 'ready')), []);
});

test('ambiguous prior-epoch TAP payout reconciles before current remaining liability', (t) => {
  const ctx = harness();
  t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));

  assert.equal(runWorker(ctx).status, 0);
  const name = `epoch-7-${APPLY_HASH}.receipts.json`;
  fs.renameSync(
    path.join(ctx.spool, 'ready', name),
    path.join(ctx.spool, 'processed', name)
  );
  writeTapReport(ctx, name);
  const reportPath = path.join(
    ctx.spool,
    'processed',
    `epoch-7-${APPLY_HASH}.settlement.json`
  );
  const report = JSON.parse(fs.readFileSync(reportPath, 'utf8'));
  report.tap_settlement_checkpoint.cumulative_spent_wei = '104';
  report.tap_settlement_checkpoint.provider_paid_au = '100';
  report.tap_settlement_checkpoint.provider_tap_wei = '100';
  report.tap_settlement_checkpoint.entries[1].cumulative_wei = '100';
  report.tap_settlement_checkpoint.outputs[0].paid_au = '100';
  report.tap_settlement_checkpoint.outputs[0].tap_wei = '100';
  report.tap_settlement_checkpoint.outputs[0].cumulative_claim_wei = '100';
  report.root_confirmation.onchain_cumulative_spent_wei = '104';
  writeJson(reportPath, report);
  writeJson(ctx.tapLiabilityState, {
    provider: TAP_PROVIDER,
    payout_revision: TAP_PAYOUT_REVISION,
    total_au: '150',
    paid_cum_au: '0',
  });
  writeJson(ctx.applyState, {
    key: 'epoch/apply/state',
    confirmed: true,
    value: {
      updated_epoch: 8,
      pending_epoch: null,
      last_apply_hash: NEXT_APPLY_HASH.toUpperCase(),
      last_settlement_unix: 1000,
    },
  });

  const resumed = runWorker(ctx, {
    MAYHEM_FIAT_SETTLEMENT_ENABLED: '0',
    MAYHEM_TNK_SETTLEMENT_ENABLED: '0',
    MOCK_EMPTY_SEAL_8: '1',
  });
  assert.equal(resumed.status, 0, resumed.stderr);
  assert.match(resumed.stdout, /resuming unresolved tap payout work for epoch 7/);
  assert.match(resumed.stdout, /epoch 7 tap payout work reconciled/);
  const liability = JSON.parse(fs.readFileSync(ctx.tapLiabilityState, 'utf8'));
  assert.equal(liability.total_au, '150');
  assert.equal(liability.paid_cum_au, '100');
  assert.deepEqual(
    JSON.parse(fs.readFileSync(ctx.tapRemainingObservation, 'utf8')),
    { total_au: '150', paid_cum_au: '100', remaining_au: '50' }
  );
  assert.equal(
    logLines(ctx).filter((line) => line.startsWith('admin tap-settlement')).length,
    2
  );

  const replay = runWorker(ctx, {
    MAYHEM_FIAT_SETTLEMENT_ENABLED: '0',
    MAYHEM_TNK_SETTLEMENT_ENABLED: '0',
    MOCK_EMPTY_SEAL_8: '1',
  });
  assert.equal(replay.status, 0, replay.stderr);
  assert.equal(
    logLines(ctx).filter((line) => line.startsWith('admin tap-settlement')).length,
    2
  );
  assert.equal(
    JSON.parse(fs.readFileSync(ctx.tapLiabilityState, 'utf8')).paid_cum_au,
    '100'
  );
});

test('TAP canonical submit survives restart before the local completion marker', (t) => {
  const ctx = harness();
  t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));

  assert.equal(runWorker(ctx).status, 0);
  const name = `epoch-7-${APPLY_HASH}.receipts.json`;
  fs.renameSync(
    path.join(ctx.spool, 'ready', name),
    path.join(ctx.spool, 'processed', name)
  );
  writeTapReport(ctx, name);
  writeJson(ctx.tapLiabilityState, {
    provider: TAP_PROVIDER,
    payout_revision: TAP_PAYOUT_REVISION,
    total_au: '20',
    paid_cum_au: '0',
  });

  const crashed = runWorker(ctx, {
    MAYHEM_PAYOUT_TEST_CRASH_AFTER_TAP_CONTRACT: '1',
  });
  assert.notEqual(crashed.status, 0);
  assert.match(crashed.stderr, /simulated crash after canonical settlement/);
  const workDir = path.join(ctx.state, `payout/epoch-7-${APPLY_HASH}`);
  assert.equal(fs.existsSync(path.join(workDir, 'tap.complete')), false);
  assert.equal(
    JSON.parse(fs.readFileSync(ctx.tapLiabilityState, 'utf8')).paid_cum_au,
    '10'
  );

  const replay = runWorker(ctx);
  assert.equal(replay.status, 0, replay.stderr);
  assert.equal(fs.existsSync(path.join(workDir, 'tap.complete')), true);
  assert.equal(
    JSON.parse(fs.readFileSync(ctx.tapLiabilityState, 'utf8')).paid_cum_au,
    '10'
  );
  assert.equal(
    logLines(ctx).filter((line) => line.startsWith('admin tap-settlement')).length,
    4
  );
});

test('epoch advance fails closed before current planning when prior evidence conflicts', (t) => {
  const ctx = harness();
  t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));

  assert.equal(runWorker(ctx).status, 0);
  const name = `epoch-7-${APPLY_HASH}.receipts.json`;
  fs.renameSync(
    path.join(ctx.spool, 'ready', name),
    path.join(ctx.spool, 'processed', name)
  );
  writeTapReport(ctx, name, { epoch_apply_hash: '0'.repeat(64) });
  writeJson(ctx.applyState, {
    key: 'epoch/apply/state',
    confirmed: true,
    value: {
      updated_epoch: 8,
      pending_epoch: null,
      last_apply_hash: NEXT_APPLY_HASH.toUpperCase(),
      last_settlement_unix: 1000,
    },
  });

  const result = runWorker(ctx, {
    MAYHEM_FIAT_SETTLEMENT_ENABLED: '0',
    MAYHEM_TNK_SETTLEMENT_ENABLED: '0',
    MOCK_EMPTY_SEAL_8: '1',
  });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /prior payout work remains failed/);
  assert.equal(fs.existsSync(ctx.tapRemainingObservation), false);
  assert.equal(
    fs.existsSync(path.join(ctx.state, `payout/epoch-8-${NEXT_APPLY_HASH}/tap.complete`)),
    false
  );
});

test('a failed rail predecessor does not block later eligible work on other rails', (t) => {
  const ctx = harness({ bundle: false });
  t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));

  const first = runWorker(ctx, {
    MOCK_FIAT_MODE: 'blocking',
    MOCK_TNK_MODE: 'no_work',
  });
  assert.notEqual(first.status, 0);
  const priorWork = path.join(ctx.state, `payout/epoch-7-${APPLY_HASH}`);
  assert.equal(fs.existsSync(path.join(priorWork, 'fiat.complete')), false);
  assert.equal(
    JSON.parse(fs.readFileSync(path.join(priorWork, 'tnk.complete'), 'utf8')).status,
    'no_work'
  );
  assert.equal(
    JSON.parse(fs.readFileSync(path.join(priorWork, 'tap.complete'), 'utf8')).status,
    'no_work'
  );

  writeJson(ctx.applyState, {
    key: 'epoch/apply/state',
    confirmed: true,
    value: {
      updated_epoch: 8,
      pending_epoch: null,
      last_apply_hash: NEXT_APPLY_HASH.toUpperCase(),
      last_settlement_unix: 1000,
    },
  });
  const next = runWorker(ctx, {
    MOCK_FIAT_MODE: 'blocking',
    MAYHEM_TNK_SETTLEMENT_ENABLED: '0',
    MOCK_EMPTY_SEAL_8: '1',
  });
  assert.notEqual(next.status, 0);
  assert.match(next.stderr, /prior payout work remains failed for fiat/);
  const currentWork = path.join(ctx.state, `payout/epoch-8-${NEXT_APPLY_HASH}`);
  assert.equal(fs.existsSync(path.join(currentWork, 'fiat.complete')), false);
  assert.equal(
    JSON.parse(fs.readFileSync(path.join(currentWork, 'tap.complete'), 'utf8')).status,
    'no_work'
  );
  assert.equal(
    logLines(ctx).filter((line) => line.startsWith('admin fiat-settlement')).length,
    2
  );
});

test('TAP queue publication survives a crash without a duplicate item or complete marker', (t) => {
  const ctx = harness();
  t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));

  const crashed = runWorker(ctx, { MAYHEM_PAYOUT_TEST_CRASH_AFTER_TAP_QUEUE: '1' });
  assert.notEqual(crashed.status, 0);
  assert.match(crashed.stderr, /simulated crash after atomic queue publication/);

  const workDir = path.join(ctx.state, `payout/epoch-7-${APPLY_HASH}`);
  const name = `epoch-7-${APPLY_HASH}.receipts.json`;
  assert.deepEqual(fs.readdirSync(path.join(ctx.spool, 'ready')), [name]);
  assert.equal(fs.existsSync(path.join(workDir, 'tap.produced')), false);
  assert.equal(fs.existsSync(path.join(workDir, 'tap.complete')), false);

  const replay = runWorker(ctx);
  assert.equal(replay.status, 0, replay.stderr);
  assert.deepEqual(fs.readdirSync(path.join(ctx.spool, 'ready')), [name]);
  assert.equal(fs.existsSync(path.join(workDir, 'tap.produced')), true);
  assert.equal(fs.existsSync(path.join(workDir, 'tap.complete')), false);
  assert.equal(fs.readFileSync(path.join(workDir, 'tap.attempts'), 'utf8').trim(), '1');
});

test('duplicate TAP lifecycle entries are rejected without burning an attempt', (t) => {
  const ctx = harness();
  t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));
  assert.equal(runWorker(ctx).status, 0);

  const workDir = path.join(ctx.state, `payout/epoch-7-${APPLY_HASH}`);
  const name = `epoch-7-${APPLY_HASH}.receipts.json`;
  fs.copyFileSync(
    path.join(ctx.spool, 'ready', name),
    path.join(ctx.spool, 'working', name)
  );
  const replay = runWorker(ctx);
  assert.notEqual(replay.status, 0);
  assert.match(replay.stderr, /duplicate spool item exists in more than one lifecycle state/);
  assert.equal(fs.readFileSync(path.join(workDir, 'tap.attempts'), 'utf8').trim(), '1');
  assert.equal(fs.existsSync(path.join(workDir, 'tap.complete')), false);
});

test('stale fiat evidence is rejected before a completion marker is written', (t) => {
  const ctx = harness({ bundle: false });
  t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));

  const result = runWorker(ctx, {
    MOCK_FIAT_MODE: 'stale_epoch',
    MOCK_TNK_MODE: 'no_work',
  });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /report outer epoch or ok flag is invalid/);
  const workDir = path.join(ctx.state, `payout/epoch-7-${APPLY_HASH}`);
  assert.equal(fs.existsSync(path.join(workDir, 'fiat.complete')), false);
});

test('fiat v17 final report requires attempts, output settlements, and close', async (t) => {
  const cases = [
    ['bad_attempt_id', /does not match its planned output/],
    ['missing_output_settlement', /does not cover every planned output/],
    ['mismatched_output', /does not match its planned output/],
    ['missing_close', /missing plan\/output\/close evidence/],
    ['mismatched_close', /canonical close does not match its immutable plan/],
  ];
  for (const [mode, error] of cases) {
    await t.test(mode, () => {
      const ctx = harness({ bundle: false });
      t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));
      const result = runWorker(ctx, {
        MOCK_FIAT_MODE: mode,
        MOCK_TNK_MODE: 'no_work',
      });
      assert.notEqual(result.status, 0);
      assert.match(result.stderr, error);
      const workDir = path.join(ctx.state, `payout/epoch-7-${APPLY_HASH}`);
      assert.equal(fs.existsSync(path.join(workDir, 'fiat.complete')), false);
      assert.equal(
        logLines(ctx).filter((line) => line.startsWith('admin fiat-settlement')).length,
        1
      );
    });
  }
});

test('canonical preparation readback precedes fiat and TNK effects across restart', async (t) => {
  for (const rail of ['fiat', 'tnk']) {
    await t.test(rail, () => {
      const ctx = harness({ bundle: false });
      t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));
      const otherRail = rail === 'fiat'
        ? { MOCK_TNK_MODE: 'no_work' }
        : { MOCK_FIAT_MODE: 'no_work' };
      const crashEnv = {
        ...otherRail,
        MOCK_CRASH_AFTER_EXTERNAL_TRANSFER: rail,
      };

      const crashed = runWorker(ctx, crashEnv);
      assert.notEqual(crashed.status, 0);
      const workDir = path.join(ctx.state, `payout/epoch-7-${APPLY_HASH}`);
      assert.match(
        fs.readFileSync(path.join(workDir, `${rail}-attempt-1.stderr.log`), 'utf8'),
        new RegExp(`simulated crash after ${rail} external transfer`)
      );
      assert.deepEqual(payoutEventLines(ctx), [
        `${rail}:canonical-preparation-readback`,
        `${rail}:external-transfer`,
      ]);

      const replay = runWorker(ctx, crashEnv);
      assert.equal(replay.status, 0, replay.stderr);
      const events = payoutEventLines(ctx);
      assert.equal(events.filter((event) => event === `${rail}:external-transfer`).length, 1);
      assert.deepEqual(events, [
        `${rail}:canonical-preparation-readback`,
        `${rail}:external-transfer`,
        `${rail}:canonical-preparation-readback`,
        `${rail}:external-transfer-recovered`,
      ]);
      assert.equal(fs.existsSync(path.join(workDir, `${rail}.complete`)), true);
    });
  }
});

test('TNK v17 final report requires every output settlement and canonical close', async (t) => {
  const cases = [
    ['missing_output_settlement', /does not cover every planned output/],
    ['mismatched_output', /does not match its planned output/],
    ['missing_close', /missing plan\/output\/close evidence/],
  ];
  for (const [mode, error] of cases) {
    await t.test(mode, () => {
      const ctx = harness({ bundle: false });
      t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));
      const result = runWorker(ctx, {
        MOCK_FIAT_MODE: 'no_work',
        MOCK_TNK_MODE: mode,
      });
      assert.notEqual(result.status, 0);
      assert.match(result.stderr, error);
      const workDir = path.join(ctx.state, `payout/epoch-7-${APPLY_HASH}`);
      assert.equal(fs.existsSync(path.join(workDir, 'tnk.complete')), false);
    });
  }
});

test('cross-epoch TAP processed evidence is rejected', (t) => {
  const ctx = harness();
  t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));

  assert.equal(runWorker(ctx).status, 0);
  const name = `epoch-7-${APPLY_HASH}.receipts.json`;
  fs.renameSync(
    path.join(ctx.spool, 'ready', name),
    path.join(ctx.spool, 'processed', name)
  );
  writeTapReport(ctx, name, { epoch: 8 });

  const replay = runWorker(ctx);
  assert.notEqual(replay.status, 0);
  assert.match(replay.stderr, /processed spool item lacks exact settlement evidence/);
  const workDir = path.join(ctx.state, `payout/epoch-7-${APPLY_HASH}`);
  assert.equal(fs.existsSync(path.join(workDir, 'tap.complete')), false);
});

test('outer-only and cross-rail receipt classification are rejected', async (t) => {
  await t.test('outer rail cannot substitute for a signed body rail', () => {
    const ctx = harness();
    t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));
    const bundlePath = path.join(ctx.state, 'epochs/epoch-7/epoch-bundle.json');
    const bundle = JSON.parse(fs.readFileSync(bundlePath, 'utf8'));
    delete bundle.receipts[1].receipt.body.rail;
    bundle.receipt_snapshot.heads = bundle.receipts;
    writeJson(bundlePath, bundle);
    rebindArtifact(ctx);

    const result = runWorker(ctx);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /failed to derive a rail-isolated spool bundle/);
    assert.deepEqual(fs.readdirSync(path.join(ctx.spool, 'ready')), []);
  });

  await t.test('outer rail must exactly match the signed body rail', () => {
    const ctx = harness();
    t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));
    const bundlePath = path.join(ctx.state, 'epochs/epoch-7/epoch-bundle.json');
    const bundle = JSON.parse(fs.readFileSync(bundlePath, 'utf8'));
    bundle.receipts[1].rail = 'fiat';
    bundle.receipt_snapshot.heads = bundle.receipts;
    writeJson(bundlePath, bundle);
    rebindArtifact(ctx);

    const result = runWorker(ctx);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /failed to derive a rail-isolated spool bundle/);
    assert.deepEqual(fs.readdirSync(path.join(ctx.spool, 'ready')), []);
  });
});

test('stale retained epoch artifact and inherited live credentials are refused', async (t) => {
  await t.test('stale artifact apply hash', () => {
    const ctx = harness();
    t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));
    const artifactPath = path.join(ctx.state, 'epochs/epoch-7/epoch-artifact.json');
    const artifact = JSON.parse(fs.readFileSync(artifactPath, 'utf8'));
    artifact.epoch_apply_hash = '9'.repeat(64);
    writeJson(artifactPath, artifact);

    const result = runWorker(ctx);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /does not match the canonical epoch\/apply hash/);
    assert.deepEqual(logLines(ctx), []);
  });

  await t.test('stale canonical commit root evidence', () => {
    const ctx = harness();
    t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));
    const result = runWorker(ctx, { MOCK_STALE_ROOT: '1' });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /canonical epoch commit does not match retained epoch evidence/);
    assert.deepEqual(logLines(ctx), []);
  });

  await t.test('stale canonical apply anchor', () => {
    const ctx = harness();
    t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));
    const result = runWorker(ctx, { MOCK_STALE_ANCHOR: '1' });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /canonical epoch apply anchor does not match retained epoch evidence/);
    assert.deepEqual(logLines(ctx), []);
  });

  await t.test('test mode credential isolation', () => {
    const ctx = harness({ bundle: false });
    t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));
    const result = runWorker(ctx, { MAYHEM_STRIPE_SECRET_KEY: 'sk_live_must_not_leak' });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /refuses inherited credential MAYHEM_STRIPE_SECRET_KEY/);
    assert.deepEqual(logLines(ctx), []);
  });
});

test('payout worker records canonical no-work outcomes without requiring live keys', (t) => {
  const ctx = harness({ bundle: false });
  t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));

  const result = runWorker(ctx, {
    MOCK_FIAT_MODE: 'no_work',
    MOCK_TNK_MODE: 'no_work',
  });
  assert.equal(result.status, 0, result.stderr);
  const workDir = path.join(ctx.state, `payout/epoch-7-${APPLY_HASH}`);
  for (const rail of ['fiat', 'tap', 'tnk']) {
    const marker = JSON.parse(fs.readFileSync(path.join(workDir, `${rail}.complete`), 'utf8'));
    assert.equal(marker.status, 'no_work');
  }
  const calls = logLines(ctx);
  assert.equal(calls.filter((line) => line.startsWith('admin fiat-settlement')).length, 1);
  assert.equal(calls.filter((line) => line.startsWith('admin tnk-settlement')).length, 2);
  assert.match(calls.find((line) => line.startsWith('admin fiat-settlement') && line.includes('--submit-transfer')), /--submit/);
  assert.match(calls.find((line) => line.startsWith('admin tnk-settlement') && line.includes('--submit-transfer')), /--submit/);
});

test('bounded payout attempts reopen automatically after backoff and transient recovery', (t) => {
  const ctx = harness({ bundle: false });
  t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));
  const extraEnv = {
    MOCK_FIAT_MODE: 'blocking',
    MOCK_TNK_MODE: 'no_work',
    MAYHEM_PAYOUT_MAX_ATTEMPTS: '2',
    MAYHEM_PAYOUT_RETRY_BACKOFF_SECONDS: '300',
  };

  assert.notEqual(runWorker(ctx, extraEnv).status, 0);
  assert.notEqual(runWorker(ctx, extraEnv).status, 0);
  const backingOff = runWorker(ctx, extraEnv);
  assert.equal(backingOff.status, 0, backingOff.stderr);
  assert.match(backingOff.stderr, /retry window reopens at 1300/);
  assert.equal(
    logLines(ctx).filter((line) => line.startsWith('admin fiat-settlement')).length,
    2
  );
  const workDir = path.join(ctx.state, `payout/epoch-7-${APPLY_HASH}`);
  assert.equal(fs.existsSync(path.join(workDir, 'fiat.complete')), false);
  assert.equal(fs.readFileSync(path.join(workDir, 'fiat.attempts'), 'utf8').trim(), '2');

  fs.writeFileSync(ctx.nowFile, '1300\n');
  const recovered = runWorker(ctx, {
    ...extraEnv,
    MOCK_FIAT_MODE: 'no_work',
  });
  assert.equal(recovered.status, 0, recovered.stderr);
  assert.match(recovered.stderr, /reopening fiat payout attempts/);
  assert.equal(fs.readFileSync(path.join(workDir, 'fiat.attempts'), 'utf8').trim(), '1');
  assert.equal(fs.existsSync(path.join(workDir, 'complete')), true);
});

test('held-only and below-threshold payout work close as no_work or carry', (t) => {
  const ctx = harness({ bundle: false });
  t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));
  const result = runWorker(ctx, {
    MOCK_FIAT_MODE: 'below_threshold',
    MOCK_TNK_MODE: 'below_threshold',
  });
  assert.equal(result.status, 0, result.stderr);
  const workDir = path.join(ctx.state, `payout/epoch-7-${APPLY_HASH}`);
  assert.equal(JSON.parse(fs.readFileSync(path.join(workDir, 'fiat.complete'))).status, 'carry');
  assert.equal(JSON.parse(fs.readFileSync(path.join(workDir, 'tnk.complete'))).status, 'carry');
  assert.equal(fs.existsSync(path.join(workDir, 'complete')), true);
  const calls = logLines(ctx);
  assert.equal(calls.filter((line) => line.startsWith('admin fiat-settlement')).length, 1);
  assert.equal(calls.filter((line) => line.startsWith('admin tnk-settlement')).length, 2);

  writeJson(ctx.applyState, {
    key: 'epoch/apply/state',
    confirmed: true,
    value: {
      updated_epoch: 8,
      pending_epoch: null,
      last_apply_hash: NEXT_APPLY_HASH.toUpperCase(),
      last_settlement_unix: 1000,
    },
  });
  writeJson(ctx.tapLiabilityState, {
    provider: TAP_PROVIDER,
    payout_revision: TAP_PAYOUT_REVISION,
    total_au: '150',
    paid_cum_au: '100',
  });
  const next = runWorker(ctx, {
    MAYHEM_FIAT_SETTLEMENT_ENABLED: '0',
    MAYHEM_TNK_SETTLEMENT_ENABLED: '0',
    MAYHEM_TAP_SETTLEMENT_ENABLED: '0',
    MOCK_EMPTY_SEAL_8: '1',
  });
  assert.equal(next.status, 0, next.stderr);
  assert.match(next.stdout, /epoch 8 payout work reconciled/);
});

test('no-work report fails closed when a payable liability is omitted', (t) => {
  const ctx = harness({ bundle: false });
  t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));
  const result = runWorker(ctx, {
    MOCK_FIAT_MODE: 'no_work',
    MOCK_TNK_MODE: 'payable_omitted',
  });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /nonblocking carry evidence omits payable work/);
  const workDir = path.join(ctx.state, `payout/epoch-7-${APPLY_HASH}`);
  assert.equal(fs.existsSync(path.join(workDir, 'tnk.complete')), false);
});

test('missing TAP bundle fails unless the exact apply hash is an empty-epoch seal', (t) => {
  const ctx = harness({ bundle: false, emptySeal: false });
  t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));
  const result = runWorker(ctx, {
    MOCK_FIAT_MODE: 'no_work',
    MOCK_TNK_MODE: 'no_work',
  });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /non-empty epoch is missing its retained receipt bundle/);
  const workDir = path.join(ctx.state, `payout/epoch-7-${APPLY_HASH}`);
  assert.equal(fs.existsSync(path.join(workDir, 'tap.complete')), false);
});

test('systemd keeps payout retries separate from non-blocking epoch finalization', () => {
  const finalizer = fs.readFileSync(FINALIZER, 'utf8');
  const cadence = fs.readFileSync(CADENCE, 'utf8');
  const installer = fs.readFileSync(INSTALLER, 'utf8');
  const tapRoller = fs.readFileSync(TAP_ROLLER, 'utf8');
  const service = fs.readFileSync(SERVICE, 'utf8');
  const timer = fs.readFileSync(TIMER, 'utf8');

  assert.match(finalizer, /epochs\/epoch-\$epoch/);
  assert.match(finalizer, /bind_epoch_artifact/);
  assert.doesNotMatch(finalizer, /ops-payout-settle\.sh/);
  assert.doesNotMatch(finalizer, /admin fiat-settlement/);
  assert.doesNotMatch(cadence, /ops-payout-settle\.sh/);
  assert.match(cadence, /ops-settle-epoch\.sh" "\$target"/);
  assert.doesNotMatch(cadence, /manual settlement required|receipts export ->/);
  assert.match(cadence, /payout\.lock/);
  assert.match(service, /ExecStart=\/opt\/mayhem\/source\/scripts\/ops-payout-settle\.sh/);
  assert.match(service, /mayhem-tap-settlement\.service/);
  assert.match(timer, /OnUnitActiveSec=1min/);
  assert.match(installer, /systemctl enable --now mayhem-payout-worker\.timer/);
  assert.match(installer, /require_private_key MAYHEM_TAP_ROLLER_PRIVATE_KEY/);
  assert.match(installer, /require_file_env MAYHEM_TNK_TREASURY_KEYPAIR_PATH/);
  assert.match(tapRoller, /find "\$working"/);
  assert.match(tapRoller, /timeout "\$attempt_timeout"/);
  assert.doesNotMatch(
    fs.readFileSync(SCRIPT, 'utf8'),
    /FIAT_OPERATOR_CURRENCY|--operator-currency/
  );
});

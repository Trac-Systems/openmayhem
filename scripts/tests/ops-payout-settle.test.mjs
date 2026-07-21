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
const FIAT_ROOT = 'b'.repeat(64);
const TNK_ROOT = 'c'.repeat(64);
const TAP_ROOT = `0x${'d'.repeat(64)}`;
const TAP_TX = `0x${'e'.repeat(64)}`;
const FIAT_PROVIDER = '6'.repeat(64);
const FIAT_PAYOUT_REVISION = '7'.repeat(64);
const FIAT_QUOTE_HASH = 'aeb7dc5278b6ad207b3dfad34b7708b1859690166b1d1f8751fd4afe5204b7d8';
const FIAT_EUR_QUOTE_HASH = '005b308d26296de4fee01e15fdf659b88e874f502f20d7d71a1d82e4b1ac4943';
const FIAT_SETTLED_BY = '9'.repeat(64);

function sha256(value) {
  return createHash('sha256').update(value).digest('hex');
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
    settlement,
    settlement_state: settlementState,
    platform_account: {
      id: 'acct_platform',
      default_currency: 'eur',
      livemode: false,
      attempts: 1,
    },
    stripe_transfers: [providerReport, operatorReport],
    reconciliation,
    skipped_providers: [],
  };
  const draftSettlement = {
    ...settlement,
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
  const planReport = {
    ok: true,
    epoch: 7,
    submitted: false,
    already_settled: null,
    nothing_to_settle: false,
    settlement: draftSettlement,
    skipped_providers: [],
  };
  const noWorkReport = {
    ...planReport,
    nothing_to_settle: true,
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
  writeJson(path.join(fixtureDir, 'blocking.json'), blockingReport);
  writeJson(path.join(fixtureDir, 'stale_epoch.json'), staleReport);
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
  const emptySealDefault = emptySeal ? '1' : '0';
  fs.mkdirSync(bin, { recursive: true });
  const fiatFixtures = writeFiatFixtures(root);
  writeJson(applyState, {
    value: {
      updated_epoch: 7,
      pending_epoch: null,
      last_apply_hash: APPLY_HASH.toUpperCase(),
    },
  });
  const nowFile = path.join(root, 'now');
  fs.writeFileSync(nowFile, '1000\n');

  writeExecutable(path.join(bin, 'curl'), `#!/usr/bin/env bash
if [[ "$*" == *"prefix=payout/liability/tnk/"* && "\${MOCK_TNK_OUTSTANDING:-0}" == "1" ]]; then
  printf '%s\\n' '{"values":[{"key":"payout/liability/tnk/provider/revision","value":{"total_au":"10","paid_cum_au":"0"}}]}'
elif [[ "$*" == *"key=fee/tnk/cum"* ]]; then
  printf '%s\\n' '{"value":{"cum_au":"0","swept_cum_au":"0"}}'
elif [[ "$*" == *"key=ev/dep/7"* ]]; then
  printf '%s\\n' '{"value":{"type":"deposit_root","epoch":7,"merkle_root":"${'1'.repeat(64)}","count":0,"au_total":"0"}}'
elif [[ "$*" == *"key=ev/use/7"* ]]; then
  printf '%s\\n' '{"value":{"type":"usage_root","epoch":7,"merkle_root":"${'2'.repeat(64)}","sessions":3,"au_total":"30","providers":3}}'
elif [[ "$*" == *"key=ev/earn/7"* ]]; then
  if [[ "\${MOCK_STALE_ROOT:-0}" == "1" ]]; then
    printf '%s\\n' '{"value":{"type":"earn_root","epoch":7,"merkle_root":"${'9'.repeat(64)}","provider_count":3,"au_cum_total":"20"}}'
  else
    printf '%s\\n' '{"value":{"type":"earn_root","epoch":7,"merkle_root":"${'3'.repeat(64)}","provider_count":3,"au_cum_total":"20"}}'
  fi
elif [[ "$*" == *"key=ev/fee/7"* ]]; then
  printf '%s\\n' '{"value":{"type":"fee_root","epoch":7,"merkle_root":"${'4'.repeat(64)}","au_fee_epoch":"5","au_fee_cum":"5","au_burn_epoch":"1","au_burn_cum":"1"}}'
elif [[ "$*" == *"key=ev/price/7"* ]]; then
  printf '%s\\n' '{"value":{"type":"price_root","epoch":7,"merkle_root":"${'5'.repeat(64)}","price_count":0}}'
elif [[ "$*" == *"key=epoch/seal/7"* ]]; then
  if [[ "\${MOCK_EMPTY_SEAL:-${emptySealDefault}}" == "1" ]]; then
    printf '%s\\n' '{"value":{"type":"epoch_empty_seal","epoch":7,"seal_hash":"${APPLY_HASH}","totals":{"debited_au":"0","earned_au":"0","fee_au":"0","burn_au":"0"}}}'
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
rail="\${2:-}"
submit=0
for arg in "$@"; do
  [[ "$arg" == "--submit-transfer" ]] && submit=1
done
if [[ "$rail" == "fiat-settlement" ]]; then
  mode="\${MOCK_FIAT_MODE:-success}"
  if [[ "$mode" == "blocking" || "$mode" == "no_work" || "$mode" == "stale_epoch" ]]; then
    cat "$MOCK_FIAT_FIXTURES/$mode.json"
  elif (( submit == 1 )); then
    cat "$MOCK_FIAT_FIXTURES/$mode.json"
  else
    cat "$MOCK_FIAT_FIXTURES/plan.json"
  fi
elif [[ "$rail" == "tnk-settlement" ]]; then
  tnk_plan='{"ok":true,"epoch":7,"submitted":false,"already_settled":null,"settlement":{"op":"settle_targeted_tnk","rail":"tnk","epoch":7,"epoch_apply_hash":"${APPLY_HASH}","transfer_root":"${TNK_ROOT}","outputs":[{"role":"provider","provider":"provider","payout_revision":"revision","to":"trac1provider","au":"10","tnk_e18":"10"}],"msb_transfers":[]},"skipped_providers":[],"msb_outputs":[{"to":"trac1provider","amount":"0.000000000000000010"}]}'
  tnk_final='{"ok":true,"epoch":7,"submitted":true,"already_settled":null,"settlement":{"op":"settle_targeted_tnk","rail":"tnk","epoch":7,"epoch_apply_hash":"${APPLY_HASH}","transfer_root":"${TNK_ROOT}","outputs":[{"role":"provider","provider":"provider","payout_revision":"revision","to":"trac1provider","au":"10","tnk_e18":"10"}],"msb_transfers":[{"schema_version":1,"network":"mainnet","tx_hash":"${'f'.repeat(64)}","confirmed_length":9,"observed_signed_length":10,"from":"trac1treasury","to":"trac1provider","amount_e18":"10"}]},"settlement_state":{"op":"settle_targeted_tnk","rail":"tnk","epoch":7,"epoch_apply_hash":"${APPLY_HASH}","transfer_root":"${TNK_ROOT}","outputs":[{"role":"provider","provider":"provider","payout_revision":"revision","to":"trac1provider","au":"10","tnk_e18":"10"}],"msb_transfers":[{"schema_version":1,"network":"mainnet","tx_hash":"${'f'.repeat(64)}","confirmed_length":9,"observed_signed_length":10,"from":"trac1treasury","to":"trac1provider","amount_e18":"10"}]},"msb_outputs":[{"to":"trac1provider","amount":"0.000000000000000010"}],"msb_transfers":[{"output_index":0,"operation_id":"op","transfer":{"tx_hash":"${'f'.repeat(64)}","confirmed_length":9,"observed_signed_length":10,"from":"trac1treasury","to":"trac1provider"}}],"skipped_providers":[]}'
  if [[ "\${MOCK_TNK_MODE:-success}" == "no_work" ]]; then
    printf '%s\\n' 'TNK settlement has no positive provider or operator fee outputs; nothing to broadcast' >&2
    exit 1
  elif (( submit == 1 )); then
    printf '%s\\n' "$tnk_final"
  else
    printf '%s\\n' "$tnk_plan"
  fi
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
    const gatewayReceiptsRaw = writeJson(
      path.join(epochDir, 'gateway-receipts.json'),
      receipts
    );
    const bundleRaw = writeJson(path.join(epochDir, 'epoch-bundle.json'), {
      epoch: 7,
      params: { fee_bps: 1500 },
      receipts,
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
      type: 'retained_epoch_artifact',
      rail: 'all',
      rails: ['fiat', 'tap', 'tnk'],
      epoch: 7,
      epoch_apply_hash: APPLY_HASH,
      bundle_sha256: sha256(bundleRaw),
      recomputed_sha256: sha256(recomputedRaw),
      gateway_receipts_sha256: sha256(gatewayReceiptsRaw),
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
  };
  return { root, state, spool, log, nowFile, env };
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

function rebindArtifact(ctx) {
  const epochDir = path.join(ctx.state, 'epochs/epoch-7');
  const bundleRaw = fs.readFileSync(path.join(epochDir, 'epoch-bundle.json'));
  const recomputedRaw = fs.readFileSync(path.join(epochDir, 'epoch-recomputed.json'));
  const bundle = JSON.parse(bundleRaw);
  writeJson(path.join(epochDir, 'gateway-receipts.json'), bundle.receipts);
  const gatewayReceiptsRaw = fs.readFileSync(path.join(epochDir, 'gateway-receipts.json'));
  const recomputed = JSON.parse(recomputedRaw);
  writeJson(path.join(epochDir, 'epoch-artifact.json'), {
    schema_version: 1,
    type: 'retained_epoch_artifact',
    rail: 'all',
    rails: ['fiat', 'tap', 'tnk'],
    epoch: 7,
    epoch_apply_hash: APPLY_HASH,
    bundle_sha256: sha256(bundleRaw),
    recomputed_sha256: sha256(recomputedRaw),
    gateway_receipts_sha256: sha256(gatewayReceiptsRaw),
    roots: recomputed.roots,
    totals: recomputed.totals,
  });
}

function writeTapReport(ctx, bundleName, overrides = {}) {
  const processed = path.join(ctx.spool, 'processed');
  const bundleRaw = fs.readFileSync(path.join(processed, bundleName));
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
  assert.equal(firstLog.length, 4);
  assert.equal(firstLog.filter((line) => line.startsWith('admin fiat-settlement')).length, 2);
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
  const processedBundle = fs.readFileSync(path.join(processed, ready[0]));
  writeJson(
    path.join(processed, `epoch-7-${APPLY_HASH}.settlement.json`),
    {
      rail: 'tap',
      epoch: 7,
      epoch_apply_hash: APPLY_HASH,
      bundle_sha256: sha256(processedBundle),
      root: TAP_ROOT,
      blocked: false,
      root_confirmed: true,
      posted: true,
      proposal_tx: TAP_TX,
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
    }
  );
  const second = runWorker(ctx);
  assert.equal(second.status, 0, second.stderr);
  assert.deepEqual(logLines(ctx), firstLog);
  const settledSummary = JSON.parse(fs.readFileSync(path.join(workDir, 'summary.json'), 'utf8'));
  assert.equal(settledSummary.complete, true);
  assert.equal(settledSummary.rails.tap.result.status, 'settled');
  assert.deepEqual(fs.readdirSync(path.join(ctx.spool, 'ready')), []);
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
  assert.match(result.stderr, /plan is not bound to the current rail\/epoch\/apply hash/);
  const workDir = path.join(ctx.state, `payout/epoch-7-${APPLY_HASH}`);
  assert.equal(fs.existsSync(path.join(workDir, 'fiat.complete')), false);
});

test('fiat settlement requires exact cross-currency state and Stripe readbacks', async (t) => {
  const cases = [
    ['bad_source_total', /settlement totals do not match outputs/],
    ['bad_destination_totals', /destination totals do not match provider outputs/],
    ['bad_provider_au', /output AU liability, paid amount, rounding, and dust do not balance/],
    ['bad_quote_hash', /provider FX\/transfer evidence is inconsistent/],
    ['bad_transfer_readback', /destination-payment readback disagrees with retained evidence/],
    [
      'bad_payment_schema_missing',
      /destination-payment readback does not have the exact canonical schema/,
    ],
    [
      'bad_payment_schema_extra',
      /destination-payment readback does not have the exact canonical schema/,
    ],
    ['bad_payment_source_amount', /destination-payment readback disagrees with retained evidence/],
    ['bad_payment_source_currency', /destination-payment readback disagrees with retained evidence/],
    ['bad_payment_id', /destination-payment readback disagrees with retained evidence/],
    ['bad_payment_currency', /destination-payment readback disagrees with retained evidence/],
    ['bad_payment_net_detail', /destination-payment readback disagrees with retained evidence/],
    ['bad_payment_gross', /destination-payment readback disagrees with retained evidence/],
    ['bad_payment_fee', /destination-payment readback disagrees with retained evidence/],
    ['bad_payment_rate', /destination-payment readback disagrees with retained evidence/],
    [
      'bad_payment_rate_null',
      /destination-payment exchange rate is not a positive exact decimal rate/,
    ],
    [
      'bad_payment_rate_number',
      /destination-payment exchange rate is not a positive exact decimal rate/,
    ],
    [
      'bad_payment_amount_type',
      /destination-payment net amount is not a canonical JSON minor-unit integer/,
    ],
    ['bad_payment_unpaid', /destination-payment readback disagrees with retained evidence/],
    ['bad_payment_uncaptured', /destination-payment readback disagrees with retained evidence/],
    ['bad_payment_source_transfer', /destination-payment readback disagrees with retained evidence/],
    ['bad_payment_balance_transaction', /destination-payment readback disagrees with retained evidence/],
    ['bad_applied_quote', /Stripe transfer readback disagrees with retained evidence/],
    ['bad_state', /canonical state disagrees on transfer_root/],
    ['bad_platform_source', /settlement source disagrees with Stripe platform account/],
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
        2
      );
    });
  }
});

test('fiat reconciliation accepts valuation-quoted same-currency non-USD settlement', (t) => {
  const ctx = harness({ bundle: false });
  t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));
  const result = runWorker(ctx, {
    MOCK_FIAT_MODE: 'same_currency_non_usd',
    MOCK_TNK_MODE: 'no_work',
  });
  assert.equal(result.status, 0, result.stderr);
  const workDir = path.join(ctx.state, `payout/epoch-7-${APPLY_HASH}`);
  assert.equal(fs.existsSync(path.join(workDir, 'fiat.complete')), true);
});

test('fiat reconciliation accepts direct USD with null or absent quotes', async (t) => {
  for (const mode of ['direct_usd_null_quotes', 'direct_usd_absent_quotes']) {
    await t.test(mode, () => {
      const ctx = harness({ bundle: false });
      t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));
      const result = runWorker(ctx, {
        MOCK_FIAT_MODE: mode,
        MOCK_TNK_MODE: 'no_work',
      });
      assert.equal(result.status, 0, result.stderr);
      const workDir = path.join(ctx.state, `payout/epoch-7-${APPLY_HASH}`);
      assert.equal(fs.existsSync(path.join(workDir, 'fiat.complete')), true);
    });
  }
});

test('fiat reconciliation enforces same-currency valuation and direct-USD quote rules', async (t) => {
  const cases = [
    ['same_non_usd_missing_quote', /provider FX quote id is invalid/],
    ['same_non_usd_missing_readback_quote', /provider Stripe readback is incomplete/],
    ['same_non_usd_unbound_quote', /FX quote readback disagrees with output/],
    ['same_non_usd_bad_quote_hash', /FX quote readback hash disagrees with retained evidence/],
    ['same_non_usd_applied_transfer_quote', /Stripe transfer readback disagrees with retained evidence/],
    ['same_non_usd_missing_transfer_quote', /Stripe transfer readback disagrees with retained evidence/],
    ['same_non_usd_payment_rate', /destination-payment readback disagrees with retained evidence/],
    ['direct_usd_with_quote', /provider output does not have the exact canonical schema/],
    ['direct_usd_quote_readback', /direct-USD provider has an FX quote readback/],
    ['direct_usd_bad_transfer', /Stripe transfer readback disagrees with retained evidence/],
    ['direct_usd_bad_payment', /destination-payment readback disagrees with retained evidence/],
    ['direct_usd_payment_rate', /destination-payment readback disagrees with retained evidence/],
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

  await t.test('stale canonical root evidence', () => {
    const ctx = harness();
    t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));
    const result = runWorker(ctx, { MOCK_STALE_ROOT: '1' });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /canonical ev\/earn root does not match retained epoch evidence/);
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
  assert.equal(logLines(ctx).length, 2);
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

test('TNK no-output is not accepted while canonical liabilities remain', (t) => {
  const ctx = harness({ bundle: false });
  t.after(() => fs.rmSync(ctx.root, { recursive: true, force: true }));
  const result = runWorker(ctx, {
    MOCK_FIAT_MODE: 'no_work',
    MOCK_TNK_MODE: 'no_work',
    MOCK_TNK_OUTSTANDING: '1',
  });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /canonical TNK liabilities remain held or blocked/);
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

test('systemd and finalizer wiring preserve the automatic payout handoff', () => {
  const finalizer = fs.readFileSync(FINALIZER, 'utf8');
  const cadence = fs.readFileSync(CADENCE, 'utf8');
  const installer = fs.readFileSync(INSTALLER, 'utf8');
  const tapRoller = fs.readFileSync(TAP_ROLLER, 'utf8');
  const service = fs.readFileSync(SERVICE, 'utf8');
  const timer = fs.readFileSync(TIMER, 'utf8');

  assert.match(finalizer, /epochs\/epoch-\$epoch/);
  assert.match(finalizer, /ops-payout-settle\.sh/);
  assert.match(finalizer, /bind_epoch_artifact/);
  assert.match(finalizer, /outer_rail != rail/);
  assert.match(finalizer, /payouts remain incomplete; refusing to finalize/);
  assert.doesNotMatch(finalizer, /admin fiat-settlement/);
  assert.match(cadence, /ops-payout-settle\.sh/);
  assert.match(cadence, /ops-settle-epoch\.sh" "\$target"/);
  assert.doesNotMatch(cadence, /manual settlement required|receipts export ->/);
  assert.match(cadence, /current epoch \$updated_epoch payouts remain incomplete/);
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

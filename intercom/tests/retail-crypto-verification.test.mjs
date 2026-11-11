import assert from 'node:assert/strict';
import test from 'node:test';

import {
  RetryWork,
  ReviewWork,
  addressTopic,
  normalizeTnkAddress,
  summarizePayoutLiabilities,
  uniqueIntentByAmount,
  validateTapBridgePreflight,
  verifyTapTransferReceipt,
} from '../scripts/retail-crypto-verification.mjs';

const HASH = `0x${'1'.repeat(64)}`;
const BLOCK_HASH = `0x${'2'.repeat(64)}`;
const TOKEN = `0x${'3'.repeat(40)}`;
const SENDER = `0x${'4'.repeat(40)}`;
const DESTINATION = `0x${'5'.repeat(40)}`;
const TRANSFER = '0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef';

function receipt(overrides = {}) {
  return {
    transactionHash: HASH,
    status: '0x1',
    blockNumber: '0x64',
    blockHash: BLOCK_HASH,
    logs: [{
      address: TOKEN,
      topics: [TRANSFER, addressTopic(SENDER), addressTopic(DESTINATION)],
      data: '0x2a',
      logIndex: '0x7',
    }],
    ...overrides,
  };
}

const options = {
  transactionHash: HASH,
  token: TOKEN,
  destination: DESTINATION,
  amountBaseUnits: '42',
  latestBlock: 120n,
  finalizedBlock: 110n,
};

test('accepts exactly one finalized transfer to the quoted token and destination', () => {
  const evidence = verifyTapTransferReceipt(receipt(), options);
  assert.equal(evidence.transactionHash, HASH);
  assert.equal(evidence.logIndex, 7);
  assert.equal(evidence.fromAddress, SENDER);
  assert.equal(evidence.tokenAmountBaseUnits, 42n);
  assert.equal(evidence.confirmations, 21);
  assert.equal(evidence.finalized, true);
  assert.equal(evidence.externalRecordKey, `tap/${HASH}/7`);
});

test('keeps an unseen or unfinalized transfer pending', () => {
  assert.throws(() => verifyTapTransferReceipt(null, options), (error) =>
    error instanceof RetryWork && error.code === 'transfer_pending' && !error.transferObserved);
  assert.throws(() => verifyTapTransferReceipt(receipt(), { ...options, finalizedBlock: 99n }), (error) =>
    error instanceof RetryWork && error.code === 'awaiting_finality' && error.transferObserved);
});

test('sends a mismatched transfer to review instead of crediting it', () => {
  assert.throws(() => verifyTapTransferReceipt(receipt(), { ...options, amountBaseUnits: '43' }), (error) =>
    error instanceof ReviewWork && error.reason === 'amount_mismatch' &&
      error.evidence?.tokenAmountBaseUnits === 42n && error.evidence?.toAddress === DESTINATION);
  assert.throws(() => verifyTapTransferReceipt(receipt(), { ...options, destination: `0x${'6'.repeat(40)}` }), (error) =>
    error instanceof ReviewWork && error.reason === 'wrong_destination' &&
      error.evidence?.toAddress === DESTINATION);
  assert.throws(() => verifyTapTransferReceipt(receipt(), { ...options, token: `0x${'7'.repeat(40)}` }), (error) =>
    error instanceof ReviewWork && error.reason === 'wrong_token');
});

test('rejects reverted or identity-mismatched receipts', () => {
  assert.throws(() => verifyTapTransferReceipt(receipt({ status: '0x0' }), options), (error) =>
    error instanceof ReviewWork && error.reason === 'malformed_transfer');
  assert.throws(() => verifyTapTransferReceipt(receipt({ transactionHash: `0x${'8'.repeat(64)}` }), options), (error) =>
    error instanceof ReviewWork && error.reason === 'malformed_transfer');
});

test('requires the TAP collection account to be bound to the platform buyer', () => {
  const collection = `0x${'7'.repeat(40)}`;
  const buyer = '8'.repeat(64);
  const report = {
    from: collection,
    tap_account_binding: { user: buyer },
    payment_config: { peer_rpc_url: 'http://127.0.0.1:49223/v1/' },
  };
  assert.deepEqual(validateTapBridgePreflight(report, {
    platformBuyer: buyer,
    collection,
    coreRpc: 'http://127.0.0.1:49223/v1/',
  }), { ethereumAccount: collection, boundUser: buyer });
  assert.throws(() => validateTapBridgePreflight({
    ...report, tap_account_binding: { user: '9'.repeat(64) },
  }, {
    platformBuyer: buyer,
    collection,
    coreRpc: 'http://127.0.0.1:49223/v1/',
  }), /configured buyer/);
});

test('validates TNK collection addresses against their configured network', () => {
  assert.equal(normalizeTnkAddress(`trac1${'a'.repeat(40)}`, 'mainnet', 'collection'), `trac1${'a'.repeat(40)}`);
  assert.equal(normalizeTnkAddress(`testtrac1${'b'.repeat(40)}`, 'testnet1', 'collection'), `testtrac1${'b'.repeat(40)}`);
  assert.throws(() => normalizeTnkAddress(`trac1${'a'.repeat(40)}`, 'testnet1', 'collection'), /invalid/);
});

test('summarizes unsettled, held, and currently payable rail liabilities', () => {
  assert.deepEqual(summarizePayoutLiabilities([
    { value: { type: 'provider_payout_liability', rail: 'tnk', total_au: '100', held_au: '20', paid_cum_au: '30' } },
    { value: { type: 'provider_payout_liability', rail: 'tnk', total_au: '40', held_au: '10', paid_cum_au: '5' } },
  ], 'TNK'), {
    totalAu: 140n,
    heldAu: 30n,
    paidAu: 35n,
    unsettledAu: 105n,
    payableAu: 75n,
  });
  assert.throws(() => summarizePayoutLiabilities([
    { value: { type: 'provider_payout_liability', rail: 'tap', total_au: '10', held_au: '8', paid_cum_au: '4' } },
  ], 'TAP'), /totals/);
});

test('attributes a transfer only when its exact amount identifies one quote', () => {
  const intents = [
    { id: 'one', token_amount_base_units: '1001' },
    { id: 'two', token_amount_base_units: '1002' },
  ];
  assert.equal(uniqueIntentByAmount(intents, 1002n)?.id, 'two');
  assert.equal(uniqueIntentByAmount(intents, 999n), null);
  assert.equal(uniqueIntentByAmount([...intents, { id: 'duplicate', token_amount_base_units: '1002' }], 1002n), null);
});

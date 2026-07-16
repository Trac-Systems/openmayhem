import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import {
  executeJournaledSettlementTransfer,
} from '../src/msb-settlement-transfer-helper.js';
import {
  bigIntTo16ByteBuffer,
  decimalStringToBigInt,
} from 'trac-msb/src/utils/amountSerialization.js';

const from = 'testtrac1settlementtreasury';
const to = 'testtrac1provider';
const amount = '0.25';
const amountE18 = decimalStringToBigInt(amount);
const txHash = 'a'.repeat(64);
const operationId = 'b'.repeat(64);

function makeMsb({ accept = false } = {}) {
  const state = { confirmed: false };
  let broadcasts = 0;
  const msb = {
    wallet: { address: from },
    network: {
      validatorConnectionManager: { connectionCount: () => 2 },
    },
    state: {
      getNodeEntry: async () => ({ balance: bigIntTo16ByteBuffer(decimalStringToBigInt('10')) }),
      getFee: () => bigIntTo16ByteBuffer(decimalStringToBigInt('0.001')),
      getIndexerSequenceState: async () => 'c'.repeat(64),
      getSigned: async (hash) => (state.confirmed && hash === txHash ? Buffer.from('confirmed') : null),
      getSignedLength: () => 48,
      getTransactionConfirmedLength: async (hash) => (
        state.confirmed && hash === txHash ? 42 : null
      ),
    },
    broadcastPartialTransaction: async (payload) => {
      broadcasts += 1;
      assert.equal(payload.tro.tx, txHash);
      if (accept) {
        state.confirmed = true;
        return true;
      }
      throw new Error('simulated crash after journal, before validator acceptance');
    },
  };
  return {
    msb,
    state,
    broadcasts: () => broadcasts,
  };
}

const buildPayload = async () => ({
  type: 9,
  address: from,
  tro: {
    tx: txHash,
    txv: 'c'.repeat(64),
    to,
    am: bigIntTo16ByteBuffer(amountE18).toString('hex'),
    in: 'd'.repeat(64),
    is: 'e'.repeat(128),
  },
});

const readConfirmedTransfer = async () => ({
  confirmed_length: 42,
  txDetails: {
    type: 9,
    address: from,
    tro: { tx: txHash, to, am: amountE18.toString() },
  },
});

function transferArgs(msb, journalFile, overrides = {}) {
  return {
    msb,
    config: {},
    network: 'testnet1',
    to,
    amount,
    operationId,
    journalFile,
    timeoutSeconds: 1,
    stderr: { write: () => {} },
    sleepFn: async () => {},
    buildPayload,
    readConfirmedTransfer,
    ...overrides,
  };
}

test('journaled MSB settlement retries the exact prepared transaction after interruption', async (t) => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-msb-settlement-'));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const journalFile = path.join(directory, 'output-0.json');

  const first = makeMsb();
  await assert.rejects(
    executeJournaledSettlementTransfer(transferArgs(first.msb, journalFile)),
    /simulated crash/
  );
  assert.equal(first.broadcasts(), 1);
  const prepared = JSON.parse(fs.readFileSync(journalFile, 'utf8'));
  assert.equal(prepared.status, 'prepared');
  assert.equal(prepared.tx_hash, txHash);

  const retry = makeMsb({ accept: true });
  const recovered = await executeJournaledSettlementTransfer(
    transferArgs(retry.msb, journalFile)
  );
  assert.equal(retry.broadcasts(), 1);
  assert.equal(recovered.tx_hash, txHash);
  assert.equal(recovered.recovered, true);
  assert.equal(recovered.rebroadcast, true);
  assert.equal(recovered.confirmed_length, 42);
  assert.equal(recovered.observed_signed_length, 48);
  assert.equal(JSON.parse(fs.readFileSync(journalFile, 'utf8')).status, 'confirmed');

  const replay = await executeJournaledSettlementTransfer(
    transferArgs(retry.msb, journalFile)
  );
  assert.equal(retry.broadcasts(), 1);
  assert.equal(replay.tx_hash, txHash);
  assert.equal(replay.rebroadcast, false);
});

test('journaled MSB settlement recovers an accepted transfer without rebroadcasting', async (t) => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-msb-accepted-'));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const journalFile = path.join(directory, 'output-0.json');
  const first = makeMsb();

  first.msb.broadcastPartialTransaction = async (payload) => {
    assert.equal(payload.tro.tx, txHash);
    first.state.confirmed = true;
    throw new Error('simulated process loss after validator acceptance');
  };
  await assert.rejects(
    executeJournaledSettlementTransfer(transferArgs(first.msb, journalFile)),
    /simulated process loss/
  );

  let recoveryBroadcasts = 0;
  first.msb.broadcastPartialTransaction = async () => {
    recoveryBroadcasts += 1;
    return true;
  };
  const recovered = await executeJournaledSettlementTransfer(
    transferArgs(first.msb, journalFile)
  );
  assert.equal(recoveryBroadcasts, 0);
  assert.equal(recovered.tx_hash, txHash);
  assert.equal(recovered.recovered, true);
});

test('journaled MSB settlement rejects changed output details before broadcast', async (t) => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-msb-mismatch-'));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const journalFile = path.join(directory, 'output-0.json');
  const first = makeMsb();
  await assert.rejects(
    executeJournaledSettlementTransfer(transferArgs(first.msb, journalFile)),
    /simulated crash/
  );

  const retry = makeMsb({ accept: true });
  await assert.rejects(
    executeJournaledSettlementTransfer(
      transferArgs(retry.msb, journalFile, { to: 'testtrac1changedprovider' })
    ),
    /journal to does not match/
  );
  assert.equal(retry.broadcasts(), 0);
});

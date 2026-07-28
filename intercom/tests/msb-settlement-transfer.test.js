import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import PeerWallet from 'trac-wallet';
import {
  bigIntTo16ByteBuffer,
  bufferToBigInt,
  decimalStringToBigInt,
} from 'trac-msb/src/utils/amountSerialization.js';

import {
  executeJournaledSettlementTransfer,
  executePreparedSettlementTransfer,
  prepareSettlementTransferPayload,
} from '../src/msb-settlement-transfer-helper.js';
import {
  createMayhemMsbConfig,
  MAYHEM_NETWORK_ENV,
} from '../src/network-config.js';

const config = createMayhemMsbConfig(MAYHEM_NETWORK_ENV.TESTNET1);
const amount = '0.25';
const amountE18 = decimalStringToBigInt(amount);
const operationId = 'b'.repeat(64);
const txValidity = 'c'.repeat(64);

async function makeWallet() {
  const wallet = new PeerWallet({ networkPrefix: config.addressPrefix });
  await wallet.ready;
  await wallet.generateKeyPair();
  return wallet;
}

const senderWallet = await makeWallet();
const recipientWallet = await makeWallet();
const otherRecipientWallet = await makeWallet();
const from = senderWallet.address;
const to = recipientWallet.address;

function makeSharedState() {
  return {
    confirmed: new Map(),
    signedLength: 48,
  };
}

function makeMsb({
  shared = makeSharedState(),
  onBroadcast = async (payload, state) => {
    state.confirmed.set(payload.tro.tx, structuredClone(payload));
    return true;
  },
} = {}) {
  const broadcasts = [];
  const msb = {
    wallet: senderWallet,
    network: {
      validatorConnectionManager: { connectionCount: () => 2 },
    },
    state: {
      getNodeEntry: async () => ({
        balance: bigIntTo16ByteBuffer(decimalStringToBigInt('10')),
      }),
      getFee: () => bigIntTo16ByteBuffer(decimalStringToBigInt('0.001')),
      getIndexerSequenceState: async () => txValidity,
      getSigned: async (hash) => (
        shared.confirmed.has(hash) ? Buffer.from('confirmed') : null
      ),
      getSignedLength: () => shared.signedLength,
      getTransactionConfirmedLength: async () => {
        throw new Error('sparse confirmation must not scan transaction history');
      },
    },
    broadcastPartialTransaction: async (payload) => {
      broadcasts.push(structuredClone(payload));
      return await onBroadcast(payload, shared);
    },
  };
  return {
    msb,
    shared,
    broadcasts,
  };
}

function readConfirmedTransfer(shared) {
  return async (_msb, hash, _config, confirmedLength) => {
    const payload = shared.confirmed.get(hash);
    if (!payload) return null;
    return {
      confirmed_length: confirmedLength,
      txDetails: {
        type: payload.type,
        address: payload.address,
        tro: {
          tx: payload.tro.tx,
          to: payload.tro.to,
          am: bufferToBigInt(Buffer.from(payload.tro.am, 'hex')).toString(),
        },
      },
    };
  };
}

function baseArgs(msb, overrides = {}) {
  return {
    msb,
    config,
    network: 'testnet1',
    to,
    amount,
    timeoutSeconds: 1,
    stderr: { write: () => {} },
    sleepFn: async () => {},
    ...overrides,
  };
}

async function prepare(msb) {
  return await prepareSettlementTransferPayload(baseArgs(msb));
}

function executeArgs(msb, shared, prepared, overrides = {}) {
  return baseArgs(msb, {
    payload: prepared.payload,
    txHash: prepared.tx_hash,
    readConfirmedTransfer: readConfirmedTransfer(shared),
    ...overrides,
  });
}

test('canonical MSB payload recovers on another host without a shared journal', async () => {
  const shared = makeSharedState();
  const hostA = makeMsb({
    shared,
    onBroadcast: async () => {
      throw new Error('simulated host-A loss before validator acceptance');
    },
  });
  const prepared = await prepare(hostA.msb);

  await assert.rejects(
    executePreparedSettlementTransfer(
      executeArgs(hostA.msb, shared, prepared)
    ),
    /simulated host-A loss/
  );
  assert.equal(hostA.broadcasts.length, 1);
  assert.deepEqual(hostA.broadcasts[0], prepared.payload);

  const hostB = makeMsb({ shared });
  const recovered = await executePreparedSettlementTransfer(
    executeArgs(hostB.msb, shared, prepared)
  );
  assert.equal(hostB.broadcasts.length, 1);
  assert.deepEqual(hostB.broadcasts[0], prepared.payload);
  assert.equal(recovered.tx_hash, prepared.tx_hash);
  assert.equal(recovered.confirmed_length, shared.signedLength);
  assert.equal(recovered.rebroadcast, true);

  const hostC = makeMsb({ shared });
  const replay = await executePreparedSettlementTransfer(
    executeArgs(hostC.msb, shared, prepared)
  );
  assert.equal(hostC.broadcasts.length, 0);
  assert.equal(replay.tx_hash, prepared.tx_hash);
  assert.equal(replay.recovered, true);
});

test('canonical MSB payload recovers accepted transfer after host-A loses the response', async () => {
  const shared = makeSharedState();
  const hostA = makeMsb({
    shared,
    onBroadcast: async (payload, state) => {
      state.confirmed.set(payload.tro.tx, structuredClone(payload));
      throw new Error('simulated host-A loss after validator acceptance');
    },
  });
  const prepared = await prepare(hostA.msb);
  await assert.rejects(
    executePreparedSettlementTransfer(
      executeArgs(hostA.msb, shared, prepared)
    ),
    /simulated host-A loss/
  );

  const hostB = makeMsb({ shared });
  const recovered = await executePreparedSettlementTransfer(
    executeArgs(hostB.msb, shared, prepared)
  );
  assert.equal(hostB.broadcasts.length, 0);
  assert.equal(recovered.tx_hash, prepared.tx_hash);
  assert.equal(recovered.recovered, true);
});

test('canonical MSB execution rejects conflicting or malformed payloads before broadcast', async () => {
  const hostA = makeMsb();
  const prepared = await prepare(hostA.msb);
  const hostB = makeMsb();

  const changedRecipient = structuredClone(prepared);
  changedRecipient.payload.tro.to = otherRecipientWallet.address;
  await assert.rejects(
    executePreparedSettlementTransfer(
      executeArgs(hostB.msb, hostB.shared, changedRecipient)
    ),
    /recipient does not match/
  );

  const changedAmount = structuredClone(prepared);
  changedAmount.payload.tro.am = bigIntTo16ByteBuffer(
    decimalStringToBigInt('0.5')
  ).toString('hex');
  await assert.rejects(
    executePreparedSettlementTransfer(
      executeArgs(hostB.msb, hostB.shared, changedAmount)
    ),
    /amount does not match/
  );

  const changedNonce = structuredClone(prepared);
  changedNonce.payload.tro.in = `${changedNonce.payload.tro.in[0] === '0' ? '1' : '0'}${changedNonce.payload.tro.in.slice(1)}`;
  await assert.rejects(
    executePreparedSettlementTransfer(
      executeArgs(hostB.msb, hostB.shared, changedNonce)
    ),
    /transaction hash does not match the signed payload/
  );

  const changedHash = structuredClone(prepared);
  changedHash.payload.tro.tx = `${changedHash.payload.tro.tx[0] === '0' ? '1' : '0'}${changedHash.payload.tro.tx.slice(1)}`;
  await assert.rejects(
    executePreparedSettlementTransfer(
      executeArgs(hostB.msb, hostB.shared, changedHash)
    ),
    /transaction hash does not match the canonical preparation/
  );

  const extraField = structuredClone(prepared);
  extraField.payload.tro.replacement = 'forbidden';
  await assert.rejects(
    executePreparedSettlementTransfer(
      executeArgs(hostB.msb, hostB.shared, extraField)
    ),
    /must contain exactly/
  );
  assert.equal(hostB.broadcasts.length, 0);
});

test('legacy journal remains an optional cache around the canonical payload API', async (t) => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-msb-settlement-'));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const journalFile = path.join(directory, 'output-0.json');
  const shared = makeSharedState();
  const hostA = makeMsb({
    shared,
    onBroadcast: async () => {
      throw new Error('simulated interruption after journal creation');
    },
  });

  await assert.rejects(
    executeJournaledSettlementTransfer(baseArgs(hostA.msb, {
      operationId,
      journalFile,
      readConfirmedTransfer: readConfirmedTransfer(shared),
    })),
    /simulated interruption/
  );
  const journal = JSON.parse(fs.readFileSync(journalFile, 'utf8'));
  assert.equal(journal.status, 'prepared');
  assert.equal(journal.tx_hash, journal.payload.tro.tx);

  const hostB = makeMsb({ shared });
  const recovered = await executeJournaledSettlementTransfer(baseArgs(hostB.msb, {
    operationId,
    journalFile,
    readConfirmedTransfer: readConfirmedTransfer(shared),
  }));
  assert.equal(hostB.broadcasts.length, 1);
  assert.deepEqual(hostB.broadcasts[0], journal.payload);
  assert.equal(recovered.tx_hash, journal.tx_hash);
  assert.equal(
    JSON.parse(fs.readFileSync(journalFile, 'utf8')).status,
    'confirmed'
  );
});

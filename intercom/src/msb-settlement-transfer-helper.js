import fs from 'fs';
import path from 'path';

import b4a from 'b4a';
import PeerWallet from 'trac-wallet';
import { MainSettlementBus } from 'trac-msb/src/index.js';
import { applyStateMessageFactory } from 'trac-msb/src/messages/state/applyStateMessageFactory.js';
import { createMessage } from 'trac-msb/src/utils/buffer.js';
import {
  bigIntTo16ByteBuffer,
  bigIntToDecimalString,
  bufferToBigInt,
  decimalStringToBigInt,
} from 'trac-msb/src/utils/amountSerialization.js';
import { OperationType } from 'trac-msb/src/utils/constants.js';
import {
  normalizeDecodedPayloadForJson,
  normalizeTransferOperation,
} from 'trac-msb/src/utils/normalizers.js';
import { safeDecodeApplyOperation } from 'trac-msb/src/utils/protobuf/operationHelpers.js';
import {
  createMayhemMsbConfig,
  MAYHEM_NETWORK_ENV,
} from './network-config.js';
import {
  openMsbWithDirectPeers,
  parseCanonicalMsbDirectPeers,
} from './msb-direct-peers.js';

const JOURNAL_SCHEMA_VERSION = 1;
const MAX_TRANSFER_AMOUNT = 0xffffffffffffffffffffffffffffffffn;

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function fail(message) {
  throw new Error(message);
}

function takeOption(args, name) {
  const index = args.indexOf(name);
  if (index === -1) return null;
  const value = args[index + 1];
  if (value === undefined || value.startsWith('--')) fail(`Missing value for ${name}.`);
  args.splice(index, 2);
  return value;
}

function normalizeNetwork(value) {
  const network = String(value || '').trim().toLowerCase();
  if (network === 'testnet') return 'testnet1';
  if (network !== 'mainnet' && network !== 'testnet1') {
    fail('--network must be mainnet or testnet1.');
  }
  return network;
}

function syncDirectory(directory) {
  let descriptor;
  try {
    descriptor = fs.openSync(directory, 'r');
    fs.fsyncSync(descriptor);
  } catch (_error) {
    // Some platforms do not permit fsync on directories.
  } finally {
    if (descriptor !== undefined) fs.closeSync(descriptor);
  }
}

function writeDurableExclusive(file, value) {
  const directory = path.dirname(file);
  fs.mkdirSync(directory, { recursive: true, mode: 0o700 });
  let descriptor;
  try {
    descriptor = fs.openSync(file, 'wx', 0o600);
    fs.writeFileSync(descriptor, `${JSON.stringify(value, null, 2)}\n`, 'utf8');
    fs.fsyncSync(descriptor);
  } finally {
    if (descriptor !== undefined) fs.closeSync(descriptor);
  }
  try {
    fs.chmodSync(file, 0o600);
  } catch (_error) {}
  syncDirectory(directory);
}

function replaceDurably(file, value) {
  const temporary = `${file}.tmp-${Date.now()}-${Math.random().toString(16).slice(2)}`;
  writeDurableExclusive(temporary, value);
  fs.renameSync(temporary, file);
  try {
    fs.chmodSync(file, 0o600);
  } catch (_error) {}
  syncDirectory(path.dirname(file));
}

function readJournal(file) {
  let parsed;
  try {
    parsed = JSON.parse(fs.readFileSync(file, 'utf8'));
  } catch (error) {
    throw new Error(`TNK transfer journal is unreadable at ${file}: ${error.message}`);
  }
  if (typeof process !== 'undefined' && process.platform !== 'win32') {
    const mode = fs.statSync(file).mode & 0o777;
    if ((mode & 0o077) !== 0) {
      throw new Error(`TNK transfer journal must be owner-only (0600): ${file}`);
    }
  }
  return parsed;
}

function expectedTransfer({ operationId, network, from, to, amountE18 }) {
  return {
    operation_id: operationId,
    network,
    from,
    to,
    amount_e18: amountE18.toString(),
    amount_hex: bigIntTo16ByteBuffer(amountE18).toString('hex'),
  };
}

export function validateSettlementTransferJournal(journal, expected) {
  if (!journal || typeof journal !== 'object' || Array.isArray(journal)) {
    fail('TNK transfer journal must be a JSON object.');
  }
  if (journal.schema_version !== JOURNAL_SCHEMA_VERSION) {
    fail(`Unsupported TNK transfer journal schema ${journal.schema_version}.`);
  }
  for (const key of ['operation_id', 'network', 'from', 'to', 'amount_e18']) {
    if (journal[key] !== expected[key]) {
      fail(`TNK transfer journal ${key} does not match the requested settlement output.`);
    }
  }
  if (!['prepared', 'confirmed'].includes(journal.status)) {
    fail('TNK transfer journal has an invalid status.');
  }
  const payload = journal.payload;
  const txHash = payload?.tro?.tx;
  if (
    payload?.address !== expected.from
    || payload?.tro?.to !== expected.to
    || payload?.tro?.am !== expected.amount_hex
    || typeof txHash !== 'string'
    || !/^[0-9a-f]{64}$/.test(txHash)
    || journal.tx_hash !== txHash
  ) {
    fail('TNK transfer journal payload does not match its settlement output.');
  }
  return journal;
}

async function waitForPreflight(msb, timeoutSeconds, stderr, sleepFn) {
  let entry = null;
  let validators = 0;
  for (let waited = 0; waited <= timeoutSeconds; waited += 1) {
    entry = await msb.state.getNodeEntry(msb.wallet.address);
    validators = msb.network.validatorConnectionManager.connectionCount();
    if (entry && validators > 0) break;
    if (waited % 5 === 0) {
      const balance = entry
        ? bigIntToDecimalString(bufferToBigInt(entry.balance))
        : 'syncing';
      stderr.write(
        `MSB settlement preflight ${waited}s: sender_balance=${balance} validator_connections=${validators}\n`
      );
    }
    if (waited < timeoutSeconds) await sleepFn(1_000);
  }
  if (!entry) fail('sender account did not sync before timeout');
  if (validators <= 0) fail('no validator connection before timeout');
  return {
    beforeBalance: bigIntToDecimalString(bufferToBigInt(entry.balance)),
    validators,
    balanceE18: bufferToBigInt(entry.balance),
  };
}

async function waitForConfirmation(msb, txHash, timeoutSeconds, sleepFn) {
  for (let waited = 0; waited <= timeoutSeconds; waited += 1) {
    if (await msb.state.getSigned(txHash)) {
      return msb.state.getSignedLength();
    }
    if (waited < timeoutSeconds) await sleepFn(1_000);
  }
  return null;
}

async function defaultReadConfirmedTransfer(msb, txHash, config, confirmedLength) {
  const rawPayload = typeof msb.getConfirmedTxInfo === 'function'
    ? await msb.getConfirmedTxInfo(txHash)
    : null;
  const fallbackPayload = rawPayload ? null : await msb.state.getSigned(txHash);
  const decoded = rawPayload?.decoded ?? safeDecodeApplyOperation(fallbackPayload);
  if (!decoded) {
    throw new Error(`No payload found for tx hash: ${txHash}`);
  }
  return {
    txDetails: normalizeDecodedPayloadForJson(decoded, config),
    confirmed_length: confirmedLength,
  };
}

function requireExactKeys(value, expectedKeys, label) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    fail(`${label} must be a JSON object.`);
  }
  const actual = Object.keys(value).sort();
  const expected = [...expectedKeys].sort();
  if (
    actual.length !== expected.length
    || actual.some((key, index) => key !== expected[index])
  ) {
    fail(`${label} must contain exactly: ${expected.join(', ')}.`);
  }
}

function requireCanonicalHex(value, bytes, label) {
  if (typeof value !== 'string' || !new RegExp(`^[0-9a-f]{${bytes * 2}}$`).test(value)) {
    fail(`${label} must be ${bytes} bytes of lowercase hexadecimal.`);
  }
}

export async function validatePreparedSettlementTransferPayload({
  payload,
  config,
  from,
  to,
  amount,
  txHash = null,
}) {
  const amountE18 = decimalStringToBigInt(amount);
  if (amountE18 <= 0n) fail('settlement transfer amount must be positive');
  if (amountE18 > MAX_TRANSFER_AMOUNT) fail('settlement transfer amount exceeds MSB maximum');

  requireExactKeys(payload, ['type', 'address', 'tro'], 'TNK settlement transfer payload');
  requireExactKeys(
    payload.tro,
    ['tx', 'txv', 'to', 'am', 'in', 'is'],
    'TNK settlement transfer operation'
  );
  if (payload.type !== OperationType.TRANSFER) {
    fail('TNK settlement payload is not an MSB transfer operation.');
  }
  if (payload.address !== from) {
    fail('TNK settlement payload sender does not match the expected treasury.');
  }
  if (payload.tro.to !== to) {
    fail('TNK settlement payload recipient does not match the expected payout target.');
  }
  const expectedAmountHex = bigIntTo16ByteBuffer(amountE18).toString('hex');
  if (payload.tro.am !== expectedAmountHex) {
    fail('TNK settlement payload amount does not match the expected payout amount.');
  }
  requireCanonicalHex(payload.tro.tx, 32, 'TNK settlement transaction hash');
  requireCanonicalHex(payload.tro.txv, 32, 'TNK settlement transaction validity');
  requireCanonicalHex(payload.tro.am, 16, 'TNK settlement transfer amount');
  requireCanonicalHex(payload.tro.in, 32, 'TNK settlement transfer nonce');
  requireCanonicalHex(payload.tro.is, 64, 'TNK settlement transfer signature');
  if (txHash !== null && payload.tro.tx !== txHash) {
    fail('TNK settlement payload transaction hash does not match the canonical preparation.');
  }

  let normalized;
  try {
    normalized = normalizeTransferOperation(payload, config);
  } catch (error) {
    fail(`TNK settlement payload is invalid: ${error.message}`);
  }
  const message = createMessage(
    config.networkId,
    normalized.tro.txv,
    normalized.tro.to,
    normalized.tro.am,
    normalized.tro.in,
    OperationType.TRANSFER
  );
  const regeneratedHash = await PeerWallet.blake3(message);
  if (!b4a.equals(regeneratedHash, normalized.tro.tx)) {
    fail('TNK settlement transaction hash does not match the signed payload.');
  }
  const publicKey = PeerWallet.decodeBech32mSafe(from);
  if (
    publicKey === null
    || !PeerWallet.verify(normalized.tro.is, regeneratedHash, publicKey)
  ) {
    fail('TNK settlement payload signature is invalid.');
  }

  return {
    payload: JSON.parse(JSON.stringify(payload)),
    tx_hash: payload.tro.tx,
    amount_e18: amountE18.toString(),
    amount_hex: expectedAmountHex,
  };
}

export async function prepareSettlementTransferPayload({
  msb,
  config,
  network,
  to,
  amount,
  timeoutSeconds,
  expectedBalanceBefore = null,
  stderr,
  sleepFn = sleep,
  buildPayload = null,
}) {
  const from = msb.wallet.address;
  const amountE18 = decimalStringToBigInt(amount);
  if (amountE18 <= 0n) fail('settlement transfer amount must be positive');
  if (amountE18 > MAX_TRANSFER_AMOUNT) fail('settlement transfer amount exceeds MSB maximum');
  const preflight = await waitForPreflight(msb, timeoutSeconds, stderr, sleepFn);
  if (expectedBalanceBefore && preflight.beforeBalance !== expectedBalanceBefore) {
    fail(
      `sender balance is ${preflight.beforeBalance}, expected ${expectedBalanceBefore}; refusing initial transfer`
    );
  }
  const feeE18 = bufferToBigInt(msb.state.getFee());
  const deductedE18 = from === to ? feeE18 : amountE18 + feeE18;
  if (preflight.balanceE18 < deductedE18) {
    fail('insufficient balance for settlement transfer and fee');
  }

  const txValidity = await msb.state.getIndexerSequenceState();
  const payload = buildPayload
    ? await buildPayload({ from, to, amountE18, txValidity })
    : await applyStateMessageFactory(msb.wallet, config)
      .buildPartialTransferOperationMessage(
        from,
        to,
        bigIntTo16ByteBuffer(amountE18),
        txValidity,
        'json'
      );
  const prepared = await validatePreparedSettlementTransferPayload({
    payload,
    config,
    from,
    to,
    amount,
  });
  return {
    schema_version: JOURNAL_SCHEMA_VERSION,
    network,
    from,
    to,
    amount,
    amount_e18: prepared.amount_e18,
    amount_hex: prepared.amount_hex,
    before_balance: preflight.beforeBalance,
    validator_connections: preflight.validators,
    tx_hash: prepared.tx_hash,
    payload: prepared.payload,
  };
}

export async function executePreparedSettlementTransfer({
  msb,
  config,
  network,
  to,
  amount,
  payload,
  txHash = null,
  timeoutSeconds,
  stderr,
  sleepFn = sleep,
  readConfirmedTransfer = defaultReadConfirmedTransfer,
  preflight = null,
}) {
  const from = msb.wallet.address;
  const amountE18 = decimalStringToBigInt(amount);
  const prepared = await validatePreparedSettlementTransferPayload({
    payload,
    config,
    from,
    to,
    amount,
    txHash,
  });
  const canonicalPayload = prepared.payload;
  const canonicalTxHash = prepared.tx_hash;

  let confirmedLength = await waitForConfirmation(msb, canonicalTxHash, 0, sleepFn);
  let rebroadcast = false;
  let activePreflight = preflight;
  if (confirmedLength === null) {
    activePreflight ??= await waitForPreflight(msb, timeoutSeconds, stderr, sleepFn);
    rebroadcast = true;
    const accepted = await msb.broadcastPartialTransaction(canonicalPayload);
    if (!accepted) fail('MSB validators did not accept the prepared settlement transaction.');
    confirmedLength = await waitForConfirmation(
      msb,
      canonicalTxHash,
      timeoutSeconds,
      sleepFn
    );
  }
  if (confirmedLength === null) {
    fail(
      `MSB settlement transaction ${canonicalTxHash} was not confirmed before timeout; recover by rebroadcasting the same canonical payload`
    );
  }

  const confirmed = await readConfirmedTransfer(
    msb,
    canonicalTxHash,
    config,
    confirmedLength
  );
  const details = confirmed?.txDetails;
  if (
    details?.address !== from
    || details?.tro?.tx !== canonicalTxHash
    || details?.tro?.to !== to
    || details?.tro?.am !== amountE18.toString()
    || confirmed?.confirmed_length !== confirmedLength
  ) {
    fail('confirmed MSB transaction does not match the canonical settlement payload');
  }

  return {
    ok: true,
    command: 'settlement-transfer',
    network,
    from,
    to,
    amount,
    tx_hash: canonicalTxHash,
    validator_connections: activePreflight?.validators ?? null,
    confirmed_length: confirmedLength,
    observed_signed_length: msb.state.getSignedLength(),
    recovered: !rebroadcast,
    rebroadcast,
  };
}

export async function executeJournaledSettlementTransfer({
  msb,
  config,
  network,
  to,
  amount,
  operationId,
  journalFile,
  timeoutSeconds,
  expectedBalanceBefore = null,
  stderr,
  sleepFn = sleep,
  buildPayload = null,
  readConfirmedTransfer = defaultReadConfirmedTransfer,
}) {
  const from = msb.wallet.address;
  const amountE18 = decimalStringToBigInt(amount);
  if (amountE18 <= 0n) fail('settlement transfer amount must be positive');
  if (amountE18 > MAX_TRANSFER_AMOUNT) fail('settlement transfer amount exceeds MSB maximum');
  const expected = expectedTransfer({ operationId, network, from, to, amountE18 });

  let recovered = fs.existsSync(journalFile);
  let journal = recovered
    ? validateSettlementTransferJournal(readJournal(journalFile), expected)
    : null;
  let preflight = null;

  if (!journal) {
    const prepared = await prepareSettlementTransferPayload({
      msb,
      config,
      network,
      to,
      amount,
      timeoutSeconds,
      expectedBalanceBefore,
      stderr,
      sleepFn,
      buildPayload,
    });
    preflight = {
      beforeBalance: prepared.before_balance,
      validators: prepared.validator_connections,
    };
    journal = validateSettlementTransferJournal({
      schema_version: JOURNAL_SCHEMA_VERSION,
      operation_id: operationId,
      network,
      from,
      to,
      amount_e18: amountE18.toString(),
      before_balance: prepared.before_balance,
      tx_hash: prepared.tx_hash,
      status: 'prepared',
      payload: prepared.payload,
    }, expected);
    try {
      writeDurableExclusive(journalFile, journal);
    } catch (error) {
      if (error?.code !== 'EEXIST') throw error;
      recovered = true;
      journal = validateSettlementTransferJournal(readJournal(journalFile), expected);
    }
  } else if (
    expectedBalanceBefore
    && journal.before_balance !== expectedBalanceBefore
  ) {
    fail('TNK transfer journal pre-broadcast balance does not match --expected-balance-before.');
  }

  const result = await executePreparedSettlementTransfer({
    msb,
    config,
    network,
    to,
    amount,
    payload: journal.payload,
    txHash: journal.tx_hash,
    timeoutSeconds,
    stderr,
    sleepFn,
    readConfirmedTransfer,
    preflight,
  });
  const confirmedLength = result.confirmed_length;
  if (journal.status !== 'confirmed' || journal.confirmed_length !== confirmedLength) {
    journal = {
      ...journal,
      status: 'confirmed',
      confirmed_length: confirmedLength,
    };
    replaceDurably(journalFile, journal);
  }

  return {
    ok: true,
    command: 'settlement-transfer',
    network,
    from,
    to,
    amount,
    tx_hash: result.tx_hash,
    before_balance: journal.before_balance,
    validator_connections: result.validator_connections,
    confirmed_length: confirmedLength,
    observed_signed_length: result.observed_signed_length,
    recovered,
    rebroadcast: result.rebroadcast && recovered,
    journal_file: journalFile,
  };
}

function defaultStderr() {
  if (typeof process !== 'undefined' && process.stderr?.write) return process.stderr;
  return { write: (message) => console.error(String(message).replace(/\n$/, '')) };
}

export async function runSettlementTransferHelper(rawArgs, options = {}) {
  const args = [...rawArgs];
  const network = normalizeNetwork(takeOption(args, '--network') ?? fail('Missing --network.'));
  const storesDirectory = takeOption(args, '--stores-directory') ?? fail('Missing --stores-directory.');
  const storeName = takeOption(args, '--store-name') ?? fail('Missing --store-name.');
  const to = takeOption(args, '--to') ?? fail('Missing --to.');
  const amount = takeOption(args, '--amount') ?? fail('Missing --amount.');
  const operationId = takeOption(args, '--operation-id') ?? fail('Missing --operation-id.');
  const journalFile = path.resolve(
    takeOption(args, '--journal-file') ?? fail('Missing --journal-file.')
  );
  const timeoutSeconds = Number.parseInt(takeOption(args, '--timeout-seconds') ?? '180', 10);
  const maxRetries = Number.parseInt(takeOption(args, '--max-retries') ?? '3', 10);
  const expectedBalanceBefore = takeOption(args, '--expected-balance-before');
  const directPeers = parseCanonicalMsbDirectPeers(
    takeOption(args, '--direct-peer')
  );
  if (args.length > 0) fail(`Unknown argument: ${args[0]}`);
  if (!/^[0-9a-f]{64}$/.test(operationId)) fail('--operation-id must be 64 lowercase hex characters.');
  if (!Number.isSafeInteger(timeoutSeconds) || timeoutSeconds <= 0) {
    fail('--timeout-seconds must be a positive safe integer.');
  }
  if (!Number.isSafeInteger(maxRetries) || maxRetries < 0) {
    fail('--max-retries must be a non-negative safe integer.');
  }

  const environment = network === 'mainnet'
    ? MAYHEM_NETWORK_ENV.MAINNET
    : MAYHEM_NETWORK_ENV.TESTNET1;
  const config = createMayhemMsbConfig(environment, {
    storeName,
    storesDirectory: path.resolve(storesDirectory) + path.sep,
    enableInteractiveMode: false,
    enableWallet: true,
    maxRetries,
  });
  const stderr = options.stderr ?? defaultStderr();
  const originalLog = console.log;
  const originalInfo = console.info;
  const redirect = (...items) => stderr.write(`${items.map(String).join(' ')}\n`);
  console.log = redirect;
  console.info = redirect;

  const msb = options.msb ?? new MainSettlementBus(config);
  let opened = Boolean(options.ready);
  try {
    if (!options.ready) {
      await openMsbWithDirectPeers(msb, {
        directPeers,
        timeoutSeconds,
        ...options.opening,
      });
      opened = true;
    }
    return await executeJournaledSettlementTransfer({
      msb,
      config,
      network,
      to,
      amount,
      operationId,
      journalFile,
      timeoutSeconds,
      expectedBalanceBefore,
      stderr,
      ...options.execution,
    });
  } finally {
    console.log = originalLog;
    console.info = originalInfo;
    if (!options.keepOpen && opened) {
      try {
        await msb.close();
      } catch (_error) {}
    }
  }
}

export async function runPreparedSettlementTransferHelper(command, rawArgs, options = {}) {
  if (!['settlement-transfer-prepare', 'settlement-transfer-execute'].includes(command)) {
    fail('Unsupported prepared settlement transfer helper command.');
  }
  const args = [...rawArgs];
  const network = normalizeNetwork(takeOption(args, '--network') ?? fail('Missing --network.'));
  const storesDirectory = takeOption(args, '--stores-directory') ?? fail('Missing --stores-directory.');
  const storeName = takeOption(args, '--store-name') ?? fail('Missing --store-name.');
  const to = takeOption(args, '--to') ?? fail('Missing --to.');
  const amount = takeOption(args, '--amount') ?? fail('Missing --amount.');
  const timeoutSeconds = Number.parseInt(takeOption(args, '--timeout-seconds') ?? '180', 10);
  const maxRetries = Number.parseInt(takeOption(args, '--max-retries') ?? '3', 10);
  const expectedBalanceBefore = takeOption(args, '--expected-balance-before');
  const payloadJson = takeOption(args, '--payload-json');
  const txHash = takeOption(args, '--tx-hash');
  const directPeers = parseCanonicalMsbDirectPeers(
    takeOption(args, '--direct-peer')
  );
  if (args.length > 0) fail(`Unknown argument: ${args[0]}`);
  if (!Number.isSafeInteger(timeoutSeconds) || timeoutSeconds <= 0) {
    fail('--timeout-seconds must be a positive safe integer.');
  }
  if (!Number.isSafeInteger(maxRetries) || maxRetries < 0) {
    fail('--max-retries must be a non-negative safe integer.');
  }
  if (command === 'settlement-transfer-prepare') {
    if (payloadJson !== null || txHash !== null) {
      fail('settlement-transfer-prepare does not accept a prepared payload.');
    }
  } else {
    if (expectedBalanceBefore !== null) {
      fail('settlement-transfer-execute does not accept --expected-balance-before.');
    }
    if (payloadJson === null || payloadJson.length > 16_384) {
      fail('settlement-transfer-execute requires a bounded --payload-json.');
    }
    requireCanonicalHex(txHash, 32, 'TNK settlement transaction hash');
  }

  const environment = network === 'mainnet'
    ? MAYHEM_NETWORK_ENV.MAINNET
    : MAYHEM_NETWORK_ENV.TESTNET1;
  const config = createMayhemMsbConfig(environment, {
    storeName,
    storesDirectory: path.resolve(storesDirectory) + path.sep,
    enableInteractiveMode: false,
    enableWallet: true,
    maxRetries,
  });
  const stderr = options.stderr ?? defaultStderr();
  const originalLog = console.log;
  const originalInfo = console.info;
  const redirect = (...items) => stderr.write(`${items.map(String).join(' ')}\n`);
  console.log = redirect;
  console.info = redirect;

  const msb = options.msb ?? new MainSettlementBus(config);
  let opened = Boolean(options.ready);
  try {
    if (!options.ready) {
      await openMsbWithDirectPeers(msb, {
        directPeers,
        timeoutSeconds,
        ...options.opening,
      });
      opened = true;
    }
    if (command === 'settlement-transfer-prepare') {
      return await prepareSettlementTransferPayload({
        msb,
        config,
        network,
        to,
        amount,
        timeoutSeconds,
        expectedBalanceBefore,
        stderr,
        ...options.execution,
      });
    }
    let payload;
    try {
      payload = JSON.parse(payloadJson);
    } catch {
      fail('--payload-json must contain valid JSON.');
    }
    return await executePreparedSettlementTransfer({
      msb,
      config,
      network,
      to,
      amount,
      payload,
      txHash,
      timeoutSeconds,
      stderr,
      ...options.execution,
    });
  } finally {
    console.log = originalLog;
    console.info = originalInfo;
    if (!options.keepOpen && opened) {
      try {
        await msb.close();
      } catch (_error) {}
    }
  }
}

export async function executeTransfer({
  msb,
  config,
  network,
  to,
  amount,
  timeoutSeconds,
  expectedBalanceBefore = null,
  stderr,
  sleepFn = sleep,
  buildPayload = null,
  readConfirmedTransfer = defaultReadConfirmedTransfer,
}) {
  const from = msb.wallet.address;
  const amountE18 = decimalStringToBigInt(amount);
  if (amountE18 <= 0n) fail('transfer amount must be positive');
  if (amountE18 > MAX_TRANSFER_AMOUNT) fail('transfer amount exceeds MSB maximum');
  const preflight = await waitForPreflight(msb, timeoutSeconds, stderr, sleepFn);
  if (expectedBalanceBefore && preflight.beforeBalance !== expectedBalanceBefore) {
    fail(
      `sender balance is ${preflight.beforeBalance}, expected ${expectedBalanceBefore}; refusing transfer`
    );
  }
  const feeE18 = bufferToBigInt(msb.state.getFee());
  const deductedE18 = from === to ? feeE18 : amountE18 + feeE18;
  if (preflight.balanceE18 < deductedE18) {
    fail('insufficient balance for transfer and fee');
  }

  const txValidity = await msb.state.getIndexerSequenceState();
  const payload = buildPayload
    ? await buildPayload({ from, to, amountE18, txValidity })
    : await applyStateMessageFactory(msb.wallet, config)
      .buildPartialTransferOperationMessage(
        from,
        to,
        bigIntTo16ByteBuffer(amountE18),
        txValidity,
        'json'
      );
  const txHash = payload?.tro?.tx;
  if (typeof txHash !== 'string' || !/^[0-9a-f]{64}$/.test(txHash)) {
    fail('MSB transfer payload did not contain a canonical transaction hash');
  }
  const accepted = await msb.broadcastPartialTransaction(payload);
  if (!accepted) fail('MSB validators did not accept the transfer.');
  const confirmedLength = await waitForConfirmation(msb, txHash, timeoutSeconds, sleepFn);
  if (confirmedLength === null) {
    fail(`MSB transfer ${txHash} was not confirmed before timeout`);
  }
  const confirmed = await readConfirmedTransfer(
    msb,
    txHash,
    config,
    confirmedLength
  );
  const details = confirmed?.txDetails;
  if (
    details?.address !== from ||
    details?.tro?.tx !== txHash ||
    details?.tro?.to !== to ||
    details?.tro?.am !== amountE18.toString() ||
    confirmed?.confirmed_length !== confirmedLength
  ) {
    fail('confirmed MSB transaction does not match the requested transfer');
  }
  return {
    ok: true,
    command: 'transfer',
    network,
    from,
    to,
    amount,
    tx_hash: txHash,
    before_balance: preflight.beforeBalance,
    validator_connections: preflight.validators,
    confirmed_length: confirmedLength,
    observed_signed_length: msb.state.getSignedLength(),
  };
}

export async function runTransferHelper(rawArgs, options = {}) {
  const args = [...rawArgs];
  const command = args.shift();
  if (command !== 'transfer') {
    fail('Supported MSB transfer helper command: transfer.');
  }
  const network = normalizeNetwork(takeOption(args, '--network') ?? fail('Missing --network.'));
  const storesDirectory = takeOption(args, '--stores-directory') ?? fail('Missing --stores-directory.');
  const storeName = takeOption(args, '--store-name') ?? fail('Missing --store-name.');
  const to = takeOption(args, '--to') ?? fail('Missing --to.');
  const amount = takeOption(args, '--amount') ?? fail('Missing --amount.');
  const timeoutSeconds = Number.parseInt(takeOption(args, '--timeout-seconds') ?? '180', 10);
  const maxRetries = Number.parseInt(takeOption(args, '--max-retries') ?? '3', 10);
  const expectedBalanceBefore = takeOption(args, '--expected-balance-before');
  const directPeers = parseCanonicalMsbDirectPeers(
    takeOption(args, '--direct-peer')
  );
  if (args.length > 0) fail(`Unknown argument: ${args[0]}`);
  if (!Number.isSafeInteger(timeoutSeconds) || timeoutSeconds <= 0) {
    fail('--timeout-seconds must be a positive safe integer.');
  }
  if (!Number.isSafeInteger(maxRetries) || maxRetries < 0) {
    fail('--max-retries must be a non-negative safe integer.');
  }

  const environment = network === 'mainnet'
    ? MAYHEM_NETWORK_ENV.MAINNET
    : MAYHEM_NETWORK_ENV.TESTNET1;
  const config = createMayhemMsbConfig(environment, {
    storeName,
    storesDirectory: path.resolve(storesDirectory) + path.sep,
    enableInteractiveMode: false,
    enableWallet: true,
    maxRetries,
  });
  const stderr = options.stderr ?? defaultStderr();
  const originalLog = console.log;
  const originalInfo = console.info;
  const redirect = (...items) => stderr.write(`${items.map(String).join(' ')}\n`);
  console.log = redirect;
  console.info = redirect;

  const msb = options.msb ?? new MainSettlementBus(config);
  let opened = Boolean(options.ready);
  try {
    if (!options.ready) {
      await openMsbWithDirectPeers(msb, {
        directPeers,
        timeoutSeconds,
        ...options.opening,
      });
      opened = true;
    }
    return await executeTransfer({
      msb,
      config,
      network,
      to,
      amount,
      timeoutSeconds,
      expectedBalanceBefore,
      stderr,
      ...options.execution,
    });
  } finally {
    console.log = originalLog;
    console.info = originalInfo;
    if (!options.keepOpen && opened) {
      try {
        await msb.close();
      } catch (_error) {}
    }
  }
}

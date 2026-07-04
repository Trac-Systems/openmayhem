#!/usr/bin/env node
import path from 'path';
import { fileURLToPath, pathToFileURL } from 'url';

const helperPath = fileURLToPath(import.meta.url);
const repoRoot = path.resolve(path.dirname(helperPath), '../../..');
const intercomRoot = path.join(repoRoot, 'intercom');
const { MainSettlementBus } = await import(
  pathToFileURL(path.join(intercomRoot, 'trac/msb/src/index.js')).href
);
const { createConfig, ENV } = await import(
  pathToFileURL(path.join(intercomRoot, 'trac/msb/src/config/env.js')).href
);
const amountModule = await import(
  pathToFileURL(path.join(intercomRoot, 'trac/msb/src/utils/amountSerialization.js')).href
);

const { bufferToBigInt, bigIntToDecimalString } = amountModule;

const args = process.argv.slice(2);

const usage = () => {
  console.error(`Usage:
  msb-transfer-helper.mjs transfer --network <mainnet|testnet1> --stores-directory <path> --store-name <name> --to <address> --amount <tnk> [--timeout-seconds <n>] [--expected-balance-before <tnk>]`);
};

const fail = (message) => {
  console.error(message);
  process.exit(1);
};

const takeOption = (name) => {
  const idx = args.indexOf(name);
  if (idx === -1) return null;
  const value = args[idx + 1];
  if (value === undefined || value.startsWith('--')) fail(`Missing value for ${name}.`);
  args.splice(idx, 2);
  return value;
};

const command = args.shift();
if (command !== 'transfer') {
  usage();
  process.exit(command ? 2 : 2);
}

const network = takeOption('--network') ?? fail('Missing --network.');
const storesDirectory = takeOption('--stores-directory') ?? fail('Missing --stores-directory.');
const storeName = takeOption('--store-name') ?? fail('Missing --store-name.');
const to = takeOption('--to') ?? fail('Missing --to.');
const amount = takeOption('--amount') ?? fail('Missing --amount.');
const timeoutSeconds = Number.parseInt(takeOption('--timeout-seconds') ?? '180', 10);
const expectedBalanceBefore = takeOption('--expected-balance-before');

if (args.length > 0) fail(`Unknown argument: ${args[0]}`);
if (!Number.isSafeInteger(timeoutSeconds) || timeoutSeconds <= 0) {
  fail('--timeout-seconds must be a positive safe integer.');
}

const env = {
  mainnet: ENV.MAINNET,
  testnet1: ENV.TESTNET1,
  testnet: ENV.TESTNET1,
}[String(network).trim().toLowerCase()];
if (!env) fail('--network must be mainnet or testnet1.');

const originalLog = console.log;
const captured = [];
const redirect = (...items) => {
  const line = items.map((item) => (
    typeof item === 'string' ? item : JSON.stringify(item)
  )).join(' ');
  captured.push(line);
  process.stderr.write(`${line}\n`);
};
console.log = redirect;
console.info = redirect;

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const balanceDecimal = (entry) => (
  entry?.balance ? bigIntToDecimalString(bufferToBigInt(entry.balance)) : '0'
);

const normalizedStoresDirectory = path.resolve(storesDirectory) + path.sep;
const config = createConfig(env, {
  storeName,
  storesDirectory: normalizedStoresDirectory,
  enableInteractiveMode: false,
  enableWallet: true,
});

const msb = new MainSettlementBus(config);
let result;
try {
  await msb.ready();
  const from = msb.wallet.address;
  let entry = null;
  let validators = 0;
  for (let waited = 0; waited <= timeoutSeconds; waited += 1) {
    entry = await msb.state.getNodeEntry(from);
    validators = msb.network.validatorConnectionManager.connectionCount();
    if (entry && validators > 0) break;
    if (waited % 5 === 0) {
      redirect(`MSB transfer preflight ${waited}s: sender_balance=${entry ? balanceDecimal(entry) : 'syncing'} validator_connections=${validators}`);
    }
    await sleep(1000);
  }
  if (!entry) throw new Error('sender account did not sync before timeout');
  const beforeBalance = balanceDecimal(entry);
  if (expectedBalanceBefore && beforeBalance !== expectedBalanceBefore) {
    throw new Error(`sender balance is ${beforeBalance}, expected ${expectedBalanceBefore}; refusing transfer retry`);
  }
  if (validators <= 0) throw new Error('no validator connection before timeout');

  await msb.handleCommand(`/transfer ${to} ${amount}`);
  const joined = captured.join('\n');
  const txHash = joined.match(/Transfer transaction broadcasted successfully\. Tx hash: ([0-9a-f]{64})/i)?.[1]
    ?? joined.match(/Transaction hash:?\s+([0-9a-f]{64})/i)?.[1]
    ?? null;
  if (!txHash) throw new Error('transfer broadcast did not expose a tx hash');
  result = {
    ok: true,
    network: String(network).trim().toLowerCase() === 'testnet' ? 'testnet1' : String(network).trim().toLowerCase(),
    from,
    to,
    amount,
    tx_hash: txHash,
    before_balance: beforeBalance,
    validator_connections: validators,
  };
} finally {
  try {
    await msb.close();
  } catch (_error) {}
}

console.log = originalLog;
originalLog(JSON.stringify(result, null, 2));

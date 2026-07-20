import path from 'path';

import { MainSettlementBus } from 'trac-msb/src/index.js';
import { bufferToBigInt, bigIntToDecimalString } from 'trac-msb/src/utils/amountSerialization.js';
import {
  createMayhemMsbConfig,
  MAYHEM_NETWORK_ENV,
} from './network-config.js';

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

export function parseRootMsbBalanceHelperArgs(rawArgs) {
  const args = [...rawArgs];
  const networkValue = (takeOption(args, '--network') ?? fail('Missing --network.'))
    .trim()
    .toLowerCase();
  const network = networkValue === 'testnet' ? 'testnet1' : networkValue;
  if (network !== 'mainnet' && network !== 'testnet1') {
    fail('--network must be mainnet or testnet1.');
  }
  const storesDirectory = takeOption(args, '--stores-directory')
    ?? fail('Missing --stores-directory.');
  const storeName = takeOption(args, '--store-name') ?? fail('Missing --store-name.');
  const address = takeOption(args, '--address') ?? fail('Missing --address.');
  const timeoutSeconds = Number.parseInt(takeOption(args, '--timeout-seconds') ?? '180', 10);
  if (!Number.isSafeInteger(timeoutSeconds) || timeoutSeconds <= 0) {
    fail('--timeout-seconds must be a positive safe integer.');
  }
  if (args.length > 0) fail(`Unknown argument: ${args[0]}`);
  const prefix = network === 'mainnet' ? 'trac1' : 'testtrac1';
  if (!address.startsWith(prefix) || /\s/.test(address)) {
    fail(`--address must be a ${prefix} address.`);
  }
  return { network, storesDirectory, storeName, address, timeoutSeconds };
}

function defaultStderr() {
  if (typeof process !== 'undefined' && process.stderr?.write) return process.stderr;
  return {
    write(message) {
      console.error(String(message).replace(/\n$/, ''));
    },
  };
}

export async function runRootMsbBalanceHelper(rawArgs, options = {}) {
  const parsed = parseRootMsbBalanceHelperArgs(rawArgs);
  const stderr = options.stderr ?? defaultStderr();
  const environment = parsed.network === 'mainnet'
    ? MAYHEM_NETWORK_ENV.MAINNET
    : MAYHEM_NETWORK_ENV.TESTNET1;
  const config = createMayhemMsbConfig(environment, {
    storeName: parsed.storeName,
    storesDirectory: `${path.resolve(parsed.storesDirectory)}${path.sep}`,
    enableInteractiveMode: false,
    enableWallet: false,
  });

  const originalLog = console.log;
  const originalInfo = console.info;
  const redirect = (...items) => {
    stderr.write(`${items.map((item) => (
      typeof item === 'string' ? item : JSON.stringify(item)
    )).join(' ')}\n`);
  };
  console.log = redirect;
  console.info = redirect;

  const msb = new MainSettlementBus(config);
  try {
    await msb.ready();
    let entry = null;
    let validators = 0;
    for (let waited = 0; waited <= parsed.timeoutSeconds; waited += 1) {
      entry = await msb.state.getNodeEntryUnsigned(parsed.address);
      validators = msb.network.validatorConnectionManager.connectionCount();
      if (entry && validators > 0) break;
      if (waited % 5 === 0) {
        redirect(`MSB balance preflight ${waited}s: account=${entry ? 'synced' : 'syncing'} validator_connections=${validators}`);
      }
      await new Promise((resolve) => setTimeout(resolve, 1000));
    }
    if (!entry) throw new Error('account did not sync before timeout');
    if (validators <= 0) throw new Error('no validator connection before timeout');
    const balance = entry.balance
      ? bigIntToDecimalString(bufferToBigInt(entry.balance))
      : '0';
    return {
      ok: true,
      command: 'balance',
      network: parsed.network,
      address: parsed.address,
      balance,
      validator_connections: validators,
    };
  } finally {
    console.log = originalLog;
    console.info = originalInfo;
    try {
      await msb.close();
    } catch (_error) {}
  }
}

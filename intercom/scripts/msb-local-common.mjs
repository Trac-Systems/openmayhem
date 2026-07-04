import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import PeerWallet from 'trac-wallet';
import { createConfig, ENV as MSB_ENV } from 'trac-msb/src/config/env.js';
import { bufferToBigInt, bigIntToDecimalString } from 'trac-msb/src/utils/amountSerialization.js';

export const INTERCOM_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
export const REPO_ROOT = path.resolve(INTERCOM_ROOT, '..');

export function parseArgs(argv = process.argv.slice(2)) {
  const out = {};
  for (let i = 0; i < argv.length; i += 1) {
    const raw = argv[i];
    if (!raw.startsWith('--')) continue;
    const eq = raw.indexOf('=');
    if (eq !== -1) {
      out[raw.slice(2, eq)] = raw.slice(eq + 1);
      continue;
    }
    const key = raw.slice(2);
    const next = argv[i + 1];
    if (next !== undefined && !String(next).startsWith('--')) {
      out[key] = next;
      i += 1;
    } else {
      out[key] = true;
    }
  }
  return out;
}

export function boolArg(value, fallback = false) {
  if (value === undefined || value === null || value === '') return fallback;
  if (value === true) return true;
  return ['1', 'true', 'yes', 'on'].includes(String(value).trim().toLowerCase());
}

export function normalizeNetwork(raw = 'testnet1') {
  const value = String(raw || 'testnet1').trim().toLowerCase();
  if (value === 'mainnet') return 'mainnet';
  if (value === 'testnet' || value === 'testnet1') return 'testnet1';
  if (value === 'development' || value === 'dev') return 'development';
  throw new Error(`Unsupported network "${raw}". Expected mainnet, testnet1, or development.`);
}

export function msbEnvironment(network) {
  return {
    mainnet: MSB_ENV.MAINNET,
    testnet1: MSB_ENV.TESTNET1,
    development: MSB_ENV.DEVELOPMENT,
  }[normalizeNetwork(network)];
}

export function defaultChannel(network) {
  return {
    mainnet: 'mayhem-local-msb-mainnet',
    testnet1: 'mayhem-local-msb-testnet1',
    development: 'mayhem-local-msb-dev',
  }[normalizeNetwork(network)];
}

export function defaultStateDir() {
  return path.resolve(process.env.MAYHEM_MSB_STATE_DIR || path.join(REPO_ROOT, '.mayhem-local', 'msb'));
}

export function trailingSlash(value) {
  const raw = path.resolve(value);
  return raw.endsWith('/') || raw.endsWith('\\') ? raw : `${raw}/`;
}

export function parseCsv(raw) {
  if (!raw) return null;
  const values = String(raw)
    .split(',')
    .map((item) => item.trim())
    .filter((item) => item.length > 0);
  return values.length > 0 ? values : null;
}

export function chmodQuiet(filePath, mode) {
  try {
    fs.chmodSync(filePath, mode);
  } catch (_error) {}
}

export function ensurePrivateDir(dir) {
  fs.mkdirSync(dir, { recursive: true });
  chmodQuiet(dir, 0o700);
}

export async function ensureKeypairFile(config) {
  if (fs.existsSync(config.keyPairPath)) return;
  ensurePrivateDir(path.dirname(config.keyPairPath));
  const wallet = new PeerWallet({ networkPrefix: config.addressPrefix });
  await wallet.ready;
  await wallet.generateKeyPair();
  wallet.exportToFile(config.keyPairPath, Buffer.alloc(0));
  chmodQuiet(config.keyPairPath, 0o600);
}

export function createLocalConfig({
  network = process.env.MAYHEM_MSB_NETWORK || 'testnet1',
  stateDir = defaultStateDir(),
  storeName = 'admin',
  channel = process.env.MAYHEM_MSB_CHANNEL || undefined,
  bootstrap,
  enableWallet = true,
  dhtBootstrap = parseCsv(process.env.MAYHEM_MSB_DHT_BOOTSTRAP || ''),
}) {
  return createConfig(msbEnvironment(network), {
    storeName,
    storesDirectory: trailingSlash(stateDir),
    enableInteractiveMode: false,
    enableWallet,
    ...(channel ? { channel } : {}),
    ...(bootstrap ? { bootstrap } : {}),
    ...(dhtBootstrap ? { dhtBootstrap } : {}),
  });
}

export function networkFilePath(stateDir = defaultStateDir()) {
  return path.join(path.resolve(stateDir), 'network.json');
}

export function readLocalNetwork(stateDir = defaultStateDir()) {
  const file = networkFilePath(stateDir);
  return JSON.parse(fs.readFileSync(file, 'utf8'));
}

export function balanceDecimal(entry) {
  if (!entry?.balance) return '0';
  return bigIntToDecimalString(bufferToBigInt(entry.balance));
}

export function printCopyPastePeerArgs(net, stateDir = defaultStateDir()) {
  console.log('Copy/paste Mayhem Intercom args:');
  console.log(
    [
      `--network ${net.network}`,
      `--msb-bootstrap ${net.bootstrap}`,
      `--msb-channel ${JSON.stringify(net.channel)}`,
      `--msb-stores-directory ${trailingSlash(path.resolve(stateDir))}`,
    ].join(' ')
  );
}

export const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
export const tick = () => new Promise((resolve) => setImmediate(resolve));

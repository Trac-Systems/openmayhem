import b4a from 'b4a';

import { Config as MsbConfig } from 'trac-msb/src/config/config.js';
import {
  createConfig as createPinnedMsbConfig,
  ENV as PINNED_MSB_ENV,
} from 'trac-msb/src/config/env.js';
import { Config as PeerConfig } from 'trac-peer/src/config/config.js';
import {
  createConfig as createPinnedPeerConfig,
  ENV as PINNED_PEER_ENV,
} from 'trac-peer/src/config/env.js';

const PUBLIC_DHT_BOOTSTRAP = Object.freeze([
  '116.202.214.149:10001',
  '157.180.12.214:10001',
  'node1.hyperdht.org:49737',
  'node2.hyperdht.org:49737',
  'node3.hyperdht.org:49737',
]);

const TESTNET1_MSB_DEFAULTS = Object.freeze({
  addressLength: 67,
  addressPrefix: 'testtrac',
  addressPrefixLength: 8,
  bech32mHrpLength: 9,
  bootstrap: 'c184f4ad8e9cf5e911f9415b60e7dcfb30aed73ebd8a402ef68e1b154624f5ef',
  channel: '1111trac1network1msb1testnet1111',
  dhtBootstrap: PUBLIC_DHT_BOOTSTRAP,
  disableRateLimit: false,
  enableErrorApplyLogs: false,
  enableInteractiveMode: true,
  enableRoleRequester: false,
  enableTxApplyLogs: false,
  enableValidatorObserver: true,
  enableWallet: true,
  maxValidators: 50,
  maxRetries: 3,
  messageThreshold: 3,
  messageValidatorRetryDelay: 1_000,
  messageValidatorResponseTimeout: 9_000,
  networkId: 919,
  storesDirectory: 'stores/',
});

const TESTNET1_PEER_DEFAULTS = Object.freeze({
  channel: '0000trac0network0peer0testnet1',
  storesDirectory: 'stores/',
  storeName: 'testnet1',
  txPoolMaxSize: 1_000,
  maxTxDelay: 60,
  maxMsbSignedLength: 1_000_000_000,
  maxMsbApplyOperationBytes: 1024 * 1024,
  enableInteractiveMode: true,
  enableBackgroundTasks: true,
  enableUpdater: true,
  replicate: true,
  dhtBootstrap: PUBLIC_DHT_BOOTSTRAP,
  enableTxlogs: false,
  apiTxExposed: false,
  apiMsgExposed: false,
  bootstrap: null,
});

export const MAYHEM_NETWORK_ENV = Object.freeze({
  MAINNET: 'mainnet',
  TESTNET1: 'testnet1',
  DEVELOPMENT: 'development',
});

const normalizeEnvironment = (environment) => {
  const value = String(environment ?? '').trim().toLowerCase();
  if (value === MAYHEM_NETWORK_ENV.MAINNET ||
      value === MAYHEM_NETWORK_ENV.TESTNET1 ||
      value === MAYHEM_NETWORK_ENV.DEVELOPMENT) {
    return value;
  }
  throw new Error(`Unknown Mayhem network environment: ${environment}`);
};

const validateBootstrapOverride = (options, label) => {
  if (!Object.hasOwn(options, 'bootstrap')) return;
  const value = options.bootstrap;
  if (b4a.isBuffer(value) && value.length === 32) return;
  if (typeof value === 'string' && /^[0-9a-fA-F]{64}$/.test(value)) return;
  if (label === 'peer' && value === null) return;
  throw new Error(`${label} bootstrap must be 32-byte hexadecimal${label === 'peer' ? ' or null' : ''}.`);
};

export const createMayhemMsbConfig = (environment, options = {}) => {
  const normalized = normalizeEnvironment(environment);
  validateBootstrapOverride(options, 'MSB');
  if (normalized === MAYHEM_NETWORK_ENV.TESTNET1) {
    return new MsbConfig(options, TESTNET1_MSB_DEFAULTS);
  }
  const pinnedEnvironment = normalized === MAYHEM_NETWORK_ENV.MAINNET
    ? PINNED_MSB_ENV.MAINNET
    : PINNED_MSB_ENV.DEVELOPMENT;
  return createPinnedMsbConfig(pinnedEnvironment, options);
};

export const createMayhemPeerConfig = (environment, options = {}) => {
  const normalized = normalizeEnvironment(environment);
  validateBootstrapOverride(options, 'peer');
  if (normalized === MAYHEM_NETWORK_ENV.TESTNET1) {
    return new PeerConfig(options, TESTNET1_PEER_DEFAULTS);
  }
  const pinnedEnvironment = normalized === MAYHEM_NETWORK_ENV.MAINNET
    ? PINNED_PEER_ENV.MAINNET
    : PINNED_PEER_ENV.DEVELOPMENT;
  return createPinnedPeerConfig(pinnedEnvironment, options);
};

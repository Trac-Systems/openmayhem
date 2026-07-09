/** @typedef {import('pear-interface')} */
import fs from 'fs';
import path from 'path';
import b4a from 'b4a';
import PeerWallet from 'trac-wallet';
import { Peer, createConfig as createPeerConfig, ENV as PEER_ENV } from 'trac-peer';
import { createServer as createRpcServer } from './rpc.js';
import { MainSettlementBus } from 'trac-msb/src/index.js';
import { createConfig as createMsbConfig, ENV as MSB_ENV } from 'trac-msb/src/config/env.js';
import { ensureTextCodecs } from 'trac-peer/src/textCodec.js';
import { getPearRuntime, ensureTrailingSlash } from 'trac-peer/src/runnerArgs.js';
import { Terminal } from 'trac-peer/src/terminal/index.js';
import MayhemProtocol from '../contract/protocol.js';
import MayhemContract from '../contract/contract.js';
import Sidechannel from '../features/sidechannel/index.js';
import DirectSession from '../features/direct-session/index.js';
import ScBridge from '../features/sc-bridge/index.js';
import MayhemFeature from '../features/mayhem/index.js';

const { env, storeLabel, flags } = getPearRuntime();

if (flags['wallet-helper']) {
  try {
    const { runWalletHelper } = await import('./wallet-helper.js');
    const output = await runWalletHelper(flags);
    console.log(JSON.stringify(output));
    Bare.exit(0);
  } catch (error) {
    console.error(error?.message ?? error);
    Bare.exit(1);
  }
}

class MayhemWallet extends PeerWallet {
  get publicKey() {
    const publicKey = super.publicKey;
    return publicKey ? b4a.toString(publicKey, 'hex') : null;
  }

  get secretKey() {
    const secretKey = super.secretKey;
    return secretKey ? b4a.toString(secretKey, 'hex') : null;
  }

  sign(message) {
    const messageBuffer = b4a.isBuffer(message) ? message : b4a.from(String(message));
    const signature = super.sign(messageBuffer);
    return b4a.toString(signature, 'hex');
  }

  verify(signature, message, publicKey = this.publicKey) {
    const signatureBuffer = b4a.isBuffer(signature) ? signature : b4a.from(String(signature), 'hex');
    const messageBuffer = b4a.isBuffer(message) ? message : b4a.from(String(message));
    const publicKeyBuffer = b4a.isBuffer(publicKey) ? publicKey : b4a.from(String(publicKey), 'hex');
    return PeerWallet.verify(signatureBuffer, messageBuffer, publicKeyBuffer);
  }
}

const parseBool = (value, fallback) => {
  if (value === undefined || value === null || value === '') return fallback;
  return ['1', 'true', 'yes', 'on'].includes(String(value).trim().toLowerCase());
};

const flagValue = (name, fallback = undefined) => {
  const value = flags[name];
  if (value === undefined) return fallback;
  if (value === true) return 'true';
  return String(value);
};

const parseInteger = (value, fallback) => {
  if (value === undefined || value === null || value === '') return fallback;
  const parsed = Number.parseInt(String(value), 10);
  return Number.isSafeInteger(parsed) ? parsed : fallback;
};

const parseCsvList = (raw) => {
  if (!raw) return null;
  return String(raw)
    .split(',')
    .map((value) => value.trim())
    .filter((value) => value.length > 0);
};

const normalizeNetworkEnv = (raw) => {
  const value = String(raw || 'mainnet').trim().toLowerCase();
  if (value === 'local') return 'local';
  if (value === 'mainnet') return 'mainnet';
  if (value === 'development' || value === 'dev') return 'development';
  if (value === 'testnet' || value === 'testnet1') return 'testnet1';
  throw new Error(`Unsupported --network value "${raw}". Expected mainnet, testnet1, local, or development.`);
};

const networkEnv = normalizeNetworkEnv(
  (flags.network && String(flags.network)) ||
    (flags['network-env'] && String(flags['network-env'])) ||
    env.MAYHEM_NETWORK ||
    env.TRAC_NETWORK_ENV ||
    ''
);

const msbEnvironment = {
  mainnet: MSB_ENV.MAINNET,
  local: MSB_ENV.DEVELOPMENT,
  development: MSB_ENV.DEVELOPMENT,
  testnet1: MSB_ENV.TESTNET1,
}[networkEnv];
const peerEnvironment = {
  mainnet: PEER_ENV.MAINNET,
  local: PEER_ENV.DEVELOPMENT,
  development: PEER_ENV.DEVELOPMENT,
  testnet1: PEER_ENV.TESTNET1,
}[networkEnv];

const headless = parseBool(flagValue('headless', env.MAYHEM_HEADLESS || ''), false);
const peerInteractive = parseBool(
  flagValue('peer-interactive', env.PEER_INTERACTIVE || ''),
  !headless
);
const peerBackgroundTasks = parseBool(
  flagValue('peer-background-tasks', env.PEER_BACKGROUND_TASKS || ''),
  true
);
const peerUpdater = parseBool(
  flagValue('peer-updater', env.PEER_UPDATER || ''),
  true
);
const peerReplicate = parseBool(
  flagValue('peer-replicate', env.PEER_REPLICATE || ''),
  true
);
const peerReplicateFlushTimeoutMs = parseInteger(
  flagValue('peer-replicate-flush-timeout-ms', env.PEER_REPLICATE_FLUSH_TIMEOUT_MS || ''),
  headless ? 5_000 : 0
);
const keepAlive = parseBool(flagValue('keep-alive', env.MAYHEM_KEEP_ALIVE || ''), headless);

const peerStoreName =
  (flags['peer-store-name'] && String(flags['peer-store-name'])) ||
  env.PEER_STORE_NAME ||
  storeLabel ||
  'peer';

const peerStoresDirectory = ensureTrailingSlash(
  (flags['peer-stores-directory'] && String(flags['peer-stores-directory'])) ||
    env.PEER_STORES_DIRECTORY ||
    'stores/'
);

const msbStoreName =
  (flags['msb-store-name'] && String(flags['msb-store-name'])) ||
  env.MSB_STORE_NAME ||
  `${peerStoreName}-msb`;

const msbStoresDirectory = ensureTrailingSlash(
  (flags['msb-stores-directory'] && String(flags['msb-stores-directory'])) ||
    env.MSB_STORES_DIRECTORY ||
    'stores/'
);

const subnetChannel =
  (flags['subnet-channel'] && String(flags['subnet-channel'])) ||
  env.SUBNET_CHANNEL ||
  'mayhem-router-subnet';

const subnetBootstrapHex =
  (flags['subnet-bootstrap'] && String(flags['subnet-bootstrap'])) ||
  env.SUBNET_BOOTSTRAP ||
  null;

const sidechannelEntry = '0000intercom';
const sidechannelsRaw =
  (flags.sidechannels && String(flags.sidechannels)) ||
  (flags.sidechannel && String(flags.sidechannel)) ||
  env.SIDECHANNELS ||
  '';
const sidechannelExtras = sidechannelsRaw
  .split(',')
  .map((value) => value.trim())
  .filter((value) => value.length > 0 && value !== sidechannelEntry);

const sidechannelQuiet = parseBool(
  (flags['sidechannel-quiet'] && String(flags['sidechannel-quiet'])) || env.SIDECHANNEL_QUIET || '',
  false
);
const sidechannelDebug = parseBool(
  (flags['sidechannel-debug'] && String(flags['sidechannel-debug'])) || env.SIDECHANNEL_DEBUG || '',
  false
);
const sidechannelMaxBytes = Number.parseInt(
  (flags['sidechannel-max-bytes'] && String(flags['sidechannel-max-bytes'])) ||
    env.SIDECHANNEL_MAX_BYTES ||
    '',
  10
);
const sidechannelAllowRemoteOpen = parseBool(
  (flags['sidechannel-allow-remote-open'] && String(flags['sidechannel-allow-remote-open'])) ||
    env.SIDECHANNEL_ALLOW_REMOTE_OPEN ||
    '',
  true
);
const sidechannelAutoJoin = parseBool(
  (flags['sidechannel-auto-join'] && String(flags['sidechannel-auto-join'])) ||
    env.SIDECHANNEL_AUTO_JOIN ||
    '',
  false
);
const sidechannelWelcomeRequired = parseBool(
  (flags['sidechannel-welcome-required'] && String(flags['sidechannel-welcome-required'])) ||
    env.SIDECHANNEL_WELCOME_REQUIRED ||
    '',
  false
);
const directSessionDebug = parseBool(
  (flags['session-debug'] && String(flags['session-debug'])) || env.SESSION_DEBUG || '',
  false
);
const directSessionMaxFrameBytes = Number.parseInt(
  (flags['session-max-frame-bytes'] && String(flags['session-max-frame-bytes'])) ||
    env.SESSION_MAX_FRAME_BYTES ||
    '',
  10
);
const directSessionRateBytesPerSecond = Number.parseInt(
  (flags['session-rate-bytes-per-second'] && String(flags['session-rate-bytes-per-second'])) ||
    env.SESSION_RATE_BYTES_PER_SECOND ||
    '',
  10
);
const directSessionRateBurstBytes = Number.parseInt(
  (flags['session-rate-burst-bytes'] && String(flags['session-rate-burst-bytes'])) ||
    env.SESSION_RATE_BURST_BYTES ||
    '',
  10
);

const scBridgeEnabled = parseBool(
  (flags['sc-bridge'] && String(flags['sc-bridge'])) || env.SC_BRIDGE || '',
  false
);
const scBridgeHost =
  (flags['sc-bridge-host'] && String(flags['sc-bridge-host'])) ||
  env.SC_BRIDGE_HOST ||
  '127.0.0.1';
const scBridgePort = Number.parseInt(
  (flags['sc-bridge-port'] && String(flags['sc-bridge-port'])) || env.SC_BRIDGE_PORT || '',
  10
);
const scBridgeToken =
  (flags['sc-bridge-token'] && String(flags['sc-bridge-token'])) ||
  env.SC_BRIDGE_TOKEN ||
  '';
const scBridgeCliEnabled = parseBool(
  (flags['sc-bridge-cli'] && String(flags['sc-bridge-cli'])) || env.SC_BRIDGE_CLI || '',
  false
);
const scBridgeDebug = parseBool(
  (flags['sc-bridge-debug'] && String(flags['sc-bridge-debug'])) || env.SC_BRIDGE_DEBUG || '',
  false
);

const rpcEnabled = parseBool(
  (flags.rpc && String(flags.rpc)) || env.PEER_RPC || '',
  false
);
const rpcHost =
  (flags['rpc-host'] && String(flags['rpc-host'])) ||
  env.PEER_RPC_HOST ||
  '127.0.0.1';
const rpcPort = Number.parseInt(
  (flags['rpc-port'] && String(flags['rpc-port'])) || env.PEER_RPC_PORT || '5001',
  10
);
const rpcAllowOrigin =
  (flags['rpc-allow-origin'] && String(flags['rpc-allow-origin'])) ||
  env.PEER_RPC_ALLOW_ORIGIN ||
  '*';
const rpcMaxBodyBytes = Number.parseInt(
  (flags['rpc-max-body-bytes'] && String(flags['rpc-max-body-bytes'])) ||
    env.PEER_RPC_MAX_BODY_BYTES ||
    '1000000',
  10
);
const apiTxExposed = parseBool(
  (flags['api-tx-exposed'] && String(flags['api-tx-exposed'])) ||
    env.PEER_API_TX_EXPOSED ||
    '',
  false
);
const apiTxLocalApply = parseBool(
  (flags['api-tx-local-apply'] && String(flags['api-tx-local-apply'])) ||
    env.PEER_API_TX_LOCAL_APPLY ||
    '',
  false
);

if (scBridgeEnabled && !scBridgeToken) {
  throw new Error('SC-Bridge requires --sc-bridge-token (auth is mandatory).');
}
if (rpcEnabled && (!Number.isSafeInteger(rpcPort) || rpcPort < 1 || rpcPort > 65535)) {
  throw new Error('Invalid --rpc-port. Expected integer 1-65535.');
}
if (rpcEnabled && (!Number.isSafeInteger(rpcMaxBodyBytes) || rpcMaxBodyBytes < 1)) {
  throw new Error('Invalid --rpc-max-body-bytes. Expected a positive integer.');
}

const peerDhtBootstrap = parseCsvList(
  (flags['peer-dht-bootstrap'] && String(flags['peer-dht-bootstrap'])) ||
    (flags['dht-bootstrap'] && String(flags['dht-bootstrap'])) ||
    env.PEER_DHT_BOOTSTRAP ||
    env.DHT_BOOTSTRAP ||
    ''
);
const msbDhtBootstrap = parseCsvList(
  (flags['msb-dht-bootstrap'] && String(flags['msb-dht-bootstrap'])) ||
    env.MSB_DHT_BOOTSTRAP ||
    ''
);
const msbBootstrapOverride =
  (flags['msb-bootstrap'] && String(flags['msb-bootstrap'])) ||
  env.MSB_BOOTSTRAP ||
  null;
const msbChannelOverride =
  (flags['msb-channel'] && String(flags['msb-channel'])) ||
  env.MSB_CHANNEL ||
  null;

if (networkEnv === 'testnet1' && (!msbBootstrapOverride || !msbChannelOverride)) {
  throw new Error(
    'Testnet1 requires explicit --msb-bootstrap and --msb-channel (or MSB_BOOTSTRAP/MSB_CHANNEL) so beta launches never fall back to mainnet defaults.'
  );
}

const readHexFile = (filePath, byteLength) => {
  try {
    if (fs.existsSync(filePath)) {
      const hex = fs.readFileSync(filePath, 'utf8').trim().toLowerCase();
      if (/^[0-9a-f]+$/.test(hex) && hex.length === byteLength * 2) return hex;
    }
  } catch (_e) {}
  return null;
};

const ensureKeypairFile = async (keyPairPath) => {
  if (fs.existsSync(keyPairPath)) return;
  fs.mkdirSync(path.dirname(keyPairPath), { recursive: true });
  await ensureTextCodecs();
  const wallet = new PeerWallet();
  await wallet.ready;
  if (!wallet.secretKey) {
    await wallet.generateKeyPair();
  }
  wallet.exportToFile(keyPairPath, b4a.alloc(0));
};

const subnetBootstrapFile = path.join(peerStoresDirectory, peerStoreName, 'subnet-bootstrap.hex');
let subnetBootstrap = subnetBootstrapHex ? subnetBootstrapHex.trim().toLowerCase() : null;
if (subnetBootstrap) {
  if (!/^[0-9a-f]{64}$/.test(subnetBootstrap)) {
    throw new Error('Invalid --subnet-bootstrap. Provide 32-byte hex (64 chars).');
  }
} else {
  subnetBootstrap = readHexFile(subnetBootstrapFile, 32);
}

const msbConfig = createMsbConfig(msbEnvironment, {
  storeName: msbStoreName,
  storesDirectory: msbStoresDirectory,
  enableInteractiveMode: false,
  ...(msbBootstrapOverride ? { bootstrap: String(msbBootstrapOverride).trim().toLowerCase() } : {}),
  ...(msbChannelOverride ? { channel: String(msbChannelOverride) } : {}),
  ...(msbDhtBootstrap ? { dhtBootstrap: msbDhtBootstrap } : {}),
});

const msbBootstrapHex = b4a.toString(msbConfig.bootstrap, 'hex');
if (subnetBootstrap && subnetBootstrap === msbBootstrapHex) {
  throw new Error('Subnet bootstrap cannot equal MSB bootstrap.');
}

const peerConfig = createPeerConfig(peerEnvironment, {
  storesDirectory: peerStoresDirectory,
  storeName: peerStoreName,
  bootstrap: subnetBootstrap || null,
  channel: subnetChannel,
  enableInteractiveMode: peerInteractive,
  enableBackgroundTasks: peerBackgroundTasks,
  enableUpdater: peerUpdater,
  replicate: peerReplicate,
  replicateFlushTimeoutMs: peerReplicateFlushTimeoutMs,
  apiTxExposed,
  apiTxLocalApply,
  ...(peerDhtBootstrap ? { dhtBootstrap: peerDhtBootstrap } : {}),
});

await ensureKeypairFile(msbConfig.keyPairPath);
await ensureKeypairFile(peerConfig.keyPairPath);

console.log('=============== STARTING MSB ===============');
const msb = new MainSettlementBus(msbConfig);
await msb.ready();

console.log('=============== STARTING MAYHEM PEER ===============');
const peer = new Peer({
  config: peerConfig,
  msb,
  wallet: new MayhemWallet({ networkPrefix: msbConfig.addressPrefix }),
  protocol: MayhemProtocol,
  contract: MayhemContract,
});
await peer.ready();

let mayhemFeature = null;
{
  const admin = await peer.base.view.get('admin');
  mayhemFeature = new MayhemFeature(peer, {});
  await peer.protocol.instance.addFeature('mayhem', mayhemFeature);
  peer.mayhemFeature = mayhemFeature;
  if (admin && admin.value === peer.wallet.publicKey) {
    console.log('Mayhem Feature: ready (free admin-oracle lifecycle/evidence writer)');
  } else if (peer.base.writable) {
    console.log('Mayhem Feature: ready (free lifecycle/evidence writer)');
  } else {
    console.log('Mayhem Feature: ready (read-only until writer admission)');
  }
}

const effectiveSubnetBootstrapHex = peer.base?.key
  ? peer.base.key.toString('hex')
  : b4a.isBuffer(peer.config.bootstrap)
    ? peer.config.bootstrap.toString('hex')
    : String(peer.config.bootstrap ?? '').toLowerCase();

if (!subnetBootstrap) {
  fs.mkdirSync(path.dirname(subnetBootstrapFile), { recursive: true });
  fs.writeFileSync(subnetBootstrapFile, `${effectiveSubnetBootstrapHex}\n`);
}

const msbChannel = b4a.toString(msbConfig.channel, 'utf8');
const msbStorePath = path.join(msbStoresDirectory, msbStoreName);
const peerStorePath = path.join(peerStoresDirectory, peerStoreName);
const peerWriterKey = peer.writerLocalKey ?? peer.base?.local?.key?.toString('hex') ?? null;

console.log('');
console.log('==================== MAYHEM INTERCOM ====================');
console.log('Network:', networkEnv);
console.log('MSB address prefix:', msbConfig.addressPrefix);
console.log('MSB network id:', msbConfig.networkId);
console.log('MSB network bootstrap:', msbBootstrapHex);
console.log('MSB channel:', msbChannel);
console.log('MSB store:', msbStorePath);
console.log('Peer store:', peerStorePath);
if (Array.isArray(msbConfig?.dhtBootstrap) && msbConfig.dhtBootstrap.length > 0) {
  console.log('MSB DHT bootstrap nodes:', msbConfig.dhtBootstrap.join(', '));
}
if (Array.isArray(peerConfig?.dhtBootstrap) && peerConfig.dhtBootstrap.length > 0) {
  console.log('Peer DHT bootstrap nodes:', peerConfig.dhtBootstrap.join(', '));
}
console.log('Peer subnet bootstrap:', effectiveSubnetBootstrapHex);
console.log('Peer subnet channel:', subnetChannel);
console.log('Peer pubkey (hex):', peer.wallet.publicKey);
console.log('Peer trac address (bech32m):', peer.wallet.address ?? null);
console.log('Peer writer key (hex):', peerWriterKey);
console.log('Sidechannel entry:', sidechannelEntry);
console.log('Headless:', headless);
console.log('Peer interactive:', peerInteractive);
console.log('Peer replicate:', peerReplicate);
console.log('Peer replicate flush timeout ms:', peerReplicateFlushTimeoutMs);
if (sidechannelExtras.length > 0) {
  console.log('Sidechannel extras:', sidechannelExtras.join(', '));
}
if (scBridgeEnabled) {
  const portDisplay = Number.isSafeInteger(scBridgePort) ? scBridgePort : 49222;
  console.log('SC-Bridge:', `ws://${scBridgeHost}:${portDisplay}`);
}
if (rpcEnabled) {
  console.log('RPC:', `http://${rpcHost}:${rpcPort}/v1`);
}
console.log('================================================================');
console.log('');

let scBridge = null;
if (scBridgeEnabled) {
  scBridge = new ScBridge(peer, {
    host: scBridgeHost,
    port: Number.isSafeInteger(scBridgePort) ? scBridgePort : 49222,
    token: scBridgeToken,
    debug: scBridgeDebug,
    cliEnabled: scBridgeCliEnabled,
    requireAuth: true,
    info: {
      app: 'mayhem',
      network: networkEnv,
      msbAddressPrefix: msbConfig.addressPrefix,
      msbNetworkId: msbConfig.networkId,
      msbBootstrap: msbBootstrapHex,
      msbChannel,
      msbStore: msbStorePath,
      peerStore: peerStorePath,
      subnetBootstrap: effectiveSubnetBootstrapHex,
      subnetChannel,
      peerPubkey: peer.wallet.publicKey,
      peerTracAddress: peer.wallet.address ?? null,
      peerWriterKey,
      sidechannelEntry,
      sidechannelExtras: sidechannelExtras.slice(),
    },
  });
}

const directSession = new DirectSession(peer, {
  debug: directSessionDebug,
  maxFrameBytes: Number.isSafeInteger(directSessionMaxFrameBytes)
    ? directSessionMaxFrameBytes
    : undefined,
  rateBytesPerSecond: Number.isSafeInteger(directSessionRateBytesPerSecond)
    ? directSessionRateBytesPerSecond
    : undefined,
  rateBurstBytes: Number.isSafeInteger(directSessionRateBurstBytes)
    ? directSessionRateBurstBytes
    : undefined,
  onFrame: scBridgeEnabled
    ? (event) => scBridge.handleSessionFrame(event)
    : null,
});
peer.directSession = directSession;

const sidechannel = new Sidechannel(peer, {
  channels: [sidechannelEntry, ...sidechannelExtras],
  debug: sidechannelDebug,
  maxMessageBytes: Number.isSafeInteger(sidechannelMaxBytes) ? sidechannelMaxBytes : undefined,
  entryChannel: sidechannelEntry,
  allowRemoteOpen: sidechannelAllowRemoteOpen,
  autoJoinOnOpen: sidechannelAutoJoin,
  welcomeRequired: sidechannelWelcomeRequired,
  onMessage: (channel, payload, connection) => {
    if (mayhemFeature.isRelayMessage(payload)) {
      mayhemFeature.handleSidechannelMessage(channel, payload, connection).catch((error) => {
        console.error('Mayhem feature relay failed:', error?.message ?? error);
      });
      return;
    }
    if (scBridgeEnabled) {
      scBridge.handleSidechannelMessage(channel, payload, connection);
    } else if (!sidechannelQuiet) {
      const from = payload?.from ?? 'unknown';
      console.log(`[sidechannel:${channel}] ${from}:`, payload?.message ?? payload);
    }
  },
});
peer.sidechannel = sidechannel;

if (scBridge) {
  scBridge.attachSidechannel(sidechannel);
  scBridge.attachDirectSession(directSession);
  try {
    scBridge.start();
  } catch (err) {
    console.error('SC-Bridge failed to start:', err?.message ?? err);
  }
  peer.scBridge = scBridge;
}

let rpcServer = null;
if (rpcEnabled) {
  rpcServer = createRpcServer(peer, {
    maxBodyBytes: rpcMaxBodyBytes,
    allowOrigin: rpcAllowOrigin,
  });
  rpcServer.listen(rpcPort, rpcHost, () => {
    console.log('RPC: ready', `http://${rpcHost}:${rpcPort}/v1`);
  });
  peer.rpcServer = rpcServer;
}

try {
  directSession.start();
} catch (err) {
  console.error('Direct session failed to start:', err?.message ?? err);
}

sidechannel
  .start()
  .then(() => {
    console.log('Sidechannel: ready');
  })
  .catch((err) => {
    console.error('Sidechannel failed to start:', err?.message ?? err);
  });

if (headless) {
  console.log('Terminal: disabled (headless)');
} else {
  const terminal = new Terminal(peer);
  await terminal.start();
}

if (keepAlive) {
  const close = async () => {
    try {
      rpcServer?.close?.();
    } catch (_e) {}
    try {
      await peer.close?.();
    } catch (_e) {}
    try {
      await msb.close?.();
    } catch (_e) {}
    if (typeof Bare !== 'undefined' && Bare.exit) Bare.exit(0);
  };
  if (typeof Pear !== 'undefined' && Pear.teardown) Pear.teardown(close);
  await new Promise(() => {});
}

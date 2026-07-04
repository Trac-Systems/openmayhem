import fs from 'node:fs';
import path from 'node:path';
import { randomBytes } from 'node:crypto';

import b4a from 'b4a';
import { MainSettlementBus } from 'trac-msb/src/index.js';
import { Config } from 'trac-msb/src/config/config.js';
import { applyStateMessageFactory } from 'trac-msb/src/messages/state/applyStateMessageFactory.js';
import { safeEncodeApplyOperation } from 'trac-msb/src/utils/protobuf/operationHelpers.js';

import {
  boolArg,
  chmodQuiet,
  createLocalConfig,
  defaultChannel,
  defaultStateDir,
  ensureKeypairFile,
  ensurePrivateDir,
  networkFilePath,
  normalizeNetwork,
  parseArgs,
  parseCsv,
  printCopyPastePeerArgs,
  tick,
  trailingSlash,
} from './msb-local-common.mjs';

const args = parseArgs();
const stateDir = path.resolve(args['state-dir'] || defaultStateDir());
const force = boolArg(args.force, false);
const network = normalizeNetwork(args.network || process.env.MAYHEM_MSB_NETWORK || 'testnet1');
const channel = String(args.channel || process.env.MAYHEM_MSB_CHANNEL || defaultChannel(network));
const storeName = String(args['store-name'] || process.env.MAYHEM_MSB_ADMIN_STORE || 'admin');
const dhtBootstrap = parseCsv(args['dht-bootstrap'] || process.env.MAYHEM_MSB_DHT_BOOTSTRAP || '');
const netFile = networkFilePath(stateDir);

if (fs.existsSync(netFile) && !force) {
  throw new Error(`${netFile} already exists. Use --force only for a disposable local ledger reset.`);
}
if (force && fs.existsSync(stateDir)) {
  fs.rmSync(stateDir, { recursive: true, force: true });
}
ensurePrivateDir(stateDir);

let options = {
  network,
  stateDir,
  storeName,
  channel,
  bootstrap: randomBytes(32).toString('hex'),
  dhtBootstrap,
};
let config = createLocalConfig(options);
await ensureKeypairFile(config);

console.log('[msb:genesis] phase 1: opening with random bootstrap to derive the admin writer key');
let msb = new MainSettlementBus(config);
await msb.ready();
const writingKey = b4a.toString(msb.state.writingKey, 'hex');
const adminAddress = msb.wallet.address;
console.log('[msb:genesis] admin address:', adminAddress);
console.log('[msb:genesis] bootstrap/writer key:', writingKey);

console.log('[msb:genesis] phase 2: reopening with bootstrap = writer key');
await msb.close();
options = { ...options, bootstrap: writingKey };
config = new Config(
  {
    storeName,
    storesDirectory: trailingSlash(stateDir),
    channel,
    bootstrap: writingKey,
    enableInteractiveMode: false,
    enableWallet: true,
    ...(dhtBootstrap ? { dhtBootstrap } : {}),
  },
  config
);
msb = new MainSettlementBus(config);
await msb.ready();
await msb.state.append(null);
await tick();

console.log('[msb:genesis] appending add-admin operation');
const txValidity = await msb.state.getIndexerSequenceState();
const payload = await applyStateMessageFactory(msb.wallet, config).buildCompleteAddAdminMessage(
  msb.wallet.address,
  msb.state.writingKey,
  txValidity
);
await msb.state.append(safeEncodeApplyOperation(payload));
await tick();
await msb.state.base.forceFastForward();
await tick();

const adminEntry = await msb.state.getAdminEntry();
const networkRecord = {
  version: 1,
  network,
  bootstrap: writingKey,
  channel,
  address_prefix: config.addressPrefix,
  network_id: config.networkId,
  admin_address: adminAddress,
  admin_store_name: storeName,
  stores_directory: trailingSlash(stateDir),
  keypair_path: config.keyPairPath,
  dht_bootstrap: dhtBootstrap || config.dhtBootstrap,
  created_at: new Date().toISOString(),
};
fs.writeFileSync(netFile, `${JSON.stringify(networkRecord, null, 2)}\n`);
chmodQuiet(netFile, 0o600);

console.log('=== MAYHEM LOCAL MSB GENESIS ===');
console.log('network     :', network);
console.log('prefix      :', config.addressPrefix);
console.log('network id  :', config.networkId);
console.log('bootstrap   :', writingKey);
console.log('channel     :', channel);
console.log('admin       :', adminAddress);
console.log('admin entry :', adminEntry ? 'present' : 'missing');
console.log('state       :', stateDir);
printCopyPastePeerArgs(networkRecord, stateDir);

await msb.close();
process.exit(adminEntry ? 0 : 1);

import path from 'node:path';

import b4a from 'b4a';
import { MainSettlementBus } from 'trac-msb/src/index.js';

import {
  balanceDecimal,
  boolArg,
  createLocalConfig,
  defaultStateDir,
  normalizeNetwork,
  parseArgs,
  readLocalNetwork,
  sleep,
} from './msb-local-common.mjs';

const args = parseArgs();
const stateDir = path.resolve(args['state-dir'] || defaultStateDir());
const explicitBootstrap = args['msb-bootstrap'] || process.env.MSB_BOOTSTRAP;
const explicitChannel = args['msb-channel'] || process.env.MSB_CHANNEL;
const useLocal = boolArg(args.local, false);
const net = useLocal ? readLocalNetwork(stateDir) : null;
const network = normalizeNetwork(args.network || net?.network || process.env.MAYHEM_MSB_NETWORK || 'testnet1');
const bootstrap = String(explicitBootstrap || net?.bootstrap || '').trim().toLowerCase();
const channel = explicitChannel || net?.channel || undefined;
const address = String(args.address || args._address || process.argv.find((item) => item.startsWith('testtrac1') || item.startsWith('trac1')) || net?.admin_address || '').trim();
const timeoutSec = Number.parseInt(args.timeout || process.env.MAYHEM_MSB_BALANCE_TIMEOUT || '60', 10);
const json = boolArg(args.json, false);
const printJson = process.stdout.write.bind(process.stdout);

if (json) {
  console.log = (...items) => console.error(...items);
  process.stdout.write = process.stderr.write.bind(process.stderr);
}

if (!address) {
  throw new Error('Missing address. Pass --address testtrac1...');
}

const config = createLocalConfig({
  network,
  stateDir: path.join(stateDir, 'balance-readers'),
  storeName: String(args['store-name'] || `balance-reader-${network}`),
  channel,
  bootstrap,
  enableWallet: false,
  dhtBootstrap: net?.dht_bootstrap || undefined,
});
const bootstrapHex = b4a.toString(config.bootstrap, 'hex');
const channelDisplay = channel || b4a.toString(config.channel, 'utf8');

if (!json) {
  console.log('[msb:balance] read-only balance probe');
  console.log('[msb:balance] network   :', network);
  console.log('[msb:balance] bootstrap :', bootstrapHex);
  console.log('[msb:balance] channel   :', channelDisplay);
  console.log('[msb:balance] address   :', address);
}

const msb = new MainSettlementBus(config);
await msb.ready();

let entry = null;
let balance = '0';
for (let i = 0; i <= timeoutSec; i += 1) {
  entry = await msb.state.getNodeEntryUnsigned(address);
  balance = balanceDecimal(entry);
  if (entry) break;
  if (!json && i % 5 === 0) console.log(`[msb:balance] syncing state (${i}s)`);
  await sleep(1000);
}

const result = {
  ok: !!entry,
  network,
  address_prefix: config.addressPrefix,
  network_id: config.networkId,
  bootstrap: bootstrapHex,
  channel: channelDisplay,
  address,
  balance,
};

if (json) {
  printJson(`${JSON.stringify(result, null, 2)}\n`);
} else {
  console.log(`[msb:balance] TNK balance: ${balance}`);
  if (!entry) console.log('[msb:balance] address entry not visible before timeout');
}

try {
  await msb.close();
} catch (_error) {}
process.exit(entry ? 0 : 2);

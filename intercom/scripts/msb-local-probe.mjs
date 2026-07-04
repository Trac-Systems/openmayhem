import path from 'node:path';

import { MainSettlementBus } from 'trac-msb/src/index.js';

import {
  boolArg,
  createLocalConfig,
  defaultStateDir,
  parseArgs,
  printCopyPastePeerArgs,
  readLocalNetwork,
  sleep,
  trailingSlash,
} from './msb-local-common.mjs';

const args = parseArgs();
const stateDir = path.resolve(args['state-dir'] || defaultStateDir());
const net = readLocalNetwork(stateDir);
const timeoutSec = Number.parseInt(args.timeout || process.env.MAYHEM_MSB_PROBE_TIMEOUT || '30', 10);
const json = boolArg(args.json, false);

const config = createLocalConfig({
  network: net.network,
  stateDir: path.join(stateDir, 'readers'),
  storeName: args['store-name'] || 'client-probe',
  channel: net.channel,
  bootstrap: net.bootstrap,
  enableWallet: false,
  dhtBootstrap: net.dht_bootstrap || undefined,
});

if (!json) {
  console.log('[msb:probe] read-only client connecting to local MSB');
  console.log('[msb:probe] network   :', net.network);
  console.log('[msb:probe] bootstrap :', net.bootstrap);
  console.log('[msb:probe] channel   :', net.channel);
}

const msb = new MainSettlementBus(config);
await msb.ready();

let adminEntry = null;
for (let i = 0; i <= timeoutSec; i += 1) {
  adminEntry = await msb.state.getAdminEntry();
  if (adminEntry) break;
  if (!json && i % 5 === 0) console.log(`[msb:probe] waiting for admin entry (${i}s)`);
  await sleep(1000);
}

const result = {
  ok: !!adminEntry,
  network: net.network,
  address_prefix: config.addressPrefix,
  network_id: config.networkId,
  bootstrap: net.bootstrap,
  channel: net.channel,
  admin_address: adminEntry?.address || null,
  state_dir: trailingSlash(stateDir),
};

if (json) {
  console.log(JSON.stringify(result, null, 2));
} else if (adminEntry) {
  console.log('[msb:probe] read-only sync ok; admin entry:', adminEntry.address);
  printCopyPastePeerArgs(net, stateDir);
} else {
  console.error(`[msb:probe] no admin entry after ${timeoutSec}s; is msb-local-run.mjs --serve running?`);
}

try {
  await msb.close();
} catch (_error) {}
process.exit(adminEntry ? 0 : 2);

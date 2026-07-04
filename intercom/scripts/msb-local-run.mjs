import http from 'node:http';
import path from 'node:path';

import { MainSettlementBus } from 'trac-msb/src/index.js';
import { applyStateMessageFactory } from 'trac-msb/src/messages/state/applyStateMessageFactory.js';
import { safeEncodeApplyOperation } from 'trac-msb/src/utils/protobuf/operationHelpers.js';
import { decimalStringToBigInt, bigIntTo16ByteBuffer } from 'trac-msb/src/utils/amountSerialization.js';

import {
  balanceDecimal,
  boolArg,
  createLocalConfig,
  defaultStateDir,
  parseArgs,
  printCopyPastePeerArgs,
  readLocalNetwork,
  tick,
  trailingSlash,
} from './msb-local-common.mjs';

const args = parseArgs();
const stateDir = path.resolve(args['state-dir'] || defaultStateDir());
const net = readLocalNetwork(stateDir);
const serve = boolArg(args.serve, boolArg(process.env.MAYHEM_MSB_RUN_FOREVER, false));
const controlPort = Number.parseInt(args['control-port'] || process.env.MAYHEM_MSB_CONTROL_PORT || '61500', 10);

const config = createLocalConfig({
  network: net.network,
  stateDir,
  storeName: net.admin_store_name || 'admin',
  channel: net.channel,
  bootstrap: net.bootstrap,
  dhtBootstrap: net.dht_bootstrap || undefined,
});

const msb = new MainSettlementBus(config);
await msb.ready();

console.log('[msb:run] admin reopened');
console.log('[msb:run] network    :', net.network);
console.log('[msb:run] prefix     :', config.addressPrefix);
console.log('[msb:run] network id :', config.networkId);
console.log('[msb:run] bootstrap  :', net.bootstrap);
console.log('[msb:run] channel    :', net.channel);
console.log('[msb:run] admin      :', net.admin_address, '(wallet reload ok:', msb.wallet.address === net.admin_address, ')');
console.log('[msb:run] writable   :', msb.state.isWritable(), '| indexer:', msb.state.isIndexer());
printCopyPastePeerArgs(net, stateDir);

async function balanceFor(address) {
  const entry = await msb.state.getNodeEntryUnsigned(address);
  return balanceDecimal(entry);
}

async function fundAddress(address, amount) {
  const amountE18 = decimalStringToBigInt(String(amount));
  const txValidity = await msb.state.getIndexerSequenceState();
  const payload = await applyStateMessageFactory(msb.wallet, config).buildCompleteBalanceInitializationMessage(
    msb.wallet.address,
    address,
    bigIntTo16ByteBuffer(amountE18),
    txValidity
  );
  await msb.state.append(safeEncodeApplyOperation(payload));
  await tick();
  await msb.state.base.forceFastForward();
  await tick();
  return balanceFor(address);
}

if (serve) {
  if (!Number.isSafeInteger(controlPort) || controlPort < 1 || controlPort > 65535) {
    throw new Error('Invalid --control-port. Expected integer 1-65535.');
  }
  const send = (res, status, body) => {
    res.writeHead(status, { 'content-type': 'application/json' });
    res.end(`${JSON.stringify(body)}\n`);
  };
  const server = http.createServer((req, res) => {
    const url = new URL(req.url, 'http://127.0.0.1');
    if (req.method === 'GET' && url.pathname === '/health') {
      return send(res, 200, {
        ok: true,
        network: net.network,
        bootstrap: net.bootstrap,
        channel: net.channel,
        admin_address: net.admin_address,
        state_dir: trailingSlash(stateDir),
      });
    }
    if (req.method === 'GET' && url.pathname === '/balance') {
      const address = url.searchParams.get('address');
      if (!address) return send(res, 400, { ok: false, error: 'address required' });
      return balanceFor(address)
        .then((balance) => send(res, 200, { ok: true, address, balance }))
        .catch((error) => send(res, 500, { ok: false, error: String(error?.message || error) }));
    }
    if (req.method === 'POST' && url.pathname === '/fund') {
      let body = '';
      req.on('data', (chunk) => {
        body += chunk;
        if (body.length > 100_000) req.destroy();
      });
      req.on('end', async () => {
        try {
          const parsed = JSON.parse(body || '{}');
          if (typeof parsed.address !== 'string' || parsed.address.length === 0) {
            return send(res, 400, { ok: false, error: 'address required' });
          }
          if (parsed.amount === undefined) {
            return send(res, 400, { ok: false, error: 'amount required' });
          }
          const balance = await fundAddress(parsed.address, parsed.amount);
          return send(res, 200, { ok: true, address: parsed.address, amount: String(parsed.amount), balance });
        } catch (error) {
          return send(res, 500, { ok: false, error: String(error?.message || error) });
        }
      });
      return;
    }
    return send(res, 404, { ok: false, error: 'not found' });
  });
  server.listen(controlPort, '127.0.0.1', () => {
    console.log(`[msb:run] control API listening on http://127.0.0.1:${controlPort}`);
    console.log('[msb:run] endpoints: GET /health, GET /balance?address=..., POST /fund {"address":"...","amount":"1.5"}');
  });
  const close = async () => {
    try {
      server.close();
    } catch (_error) {}
    try {
      await msb.close();
    } catch (_error) {}
    process.exit(0);
  };
  process.on('SIGINT', close);
  process.on('SIGTERM', close);
} else {
  await msb.close();
  process.exit(0);
}

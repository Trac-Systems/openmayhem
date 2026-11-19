// Loopback-only Ethereum evidence fixture for the full payout shell worker.
// The fixture copies expected receipts at setup; changing the archived report
// later cannot change what the simulated chain finalized.
import http from 'node:http';
import { workerData } from 'node:worker_threads';
import { createRequire } from 'node:module';
import { POOL_SETTLEMENT_ABI } from '../../../contracts/scripts/tap-settlement-roller.mjs';

const { Interface } = createRequire(new URL('../../../contracts/package.json', import.meta.url))('ethers');
const abi = new Interface(POOL_SETTLEMENT_ABI);
const cp = workerData.checkpoint;
const blocks = {};
const receipts = {};
const q = n => '0x' + n.toString(16);
for (const [role, name, timestamp] of [['proposal', 'RootProposed', 1001], ['execution', 'RootPosted', 1015]]) {
  const args = [cp.epoch, cp.root, cp.cumulative_spent_wei, 1n];
  if (role === 'proposal') args.push(1005n);
  const blockNumber = q(cp[role + '_block_number']);
  const blockHash = cp[role + '_block_hash'];
  const transactionHash = cp[role + '_tx'];
  blocks[blockNumber] = { number: blockNumber, hash: blockHash, timestamp: q(timestamp) };
  receipts[transactionHash] = { transactionHash, blockNumber, blockHash, status: '0x1', logs: [{
    ...abi.encodeEventLog(name, args), transactionHash, blockNumber, blockHash, address: cp.pool_address,
  }] };
}
blocks.finalized = { number: q(cp.finalized_block_number), hash: '0x' + 'ab'.repeat(32) };
const server = http.createServer(async (req, res) => {
  res.setHeader('content-type', 'application/json');
  if (req.method === 'POST' && req.url === '/eth') {
    let body = '';
    for await (const part of req) body += part;
    const { method, params, id } = JSON.parse(body);
    const result = method === 'eth_chainId' ? '0x1' :
      method === 'eth_getBlockByNumber' ? blocks[params[0]] :
      method === 'eth_getTransactionReceipt' ? receipts[params[0]] : null;
    res.end(JSON.stringify({ jsonrpc: '2.0', id, result: result ?? null }));
    return;
  }
  const url = new URL(req.url, 'http://127.0.0.1');
  if (req.method === 'GET' && url.pathname === '/v1/state' && url.searchParams.get('confirmed') === 'true') {
    const key = url.searchParams.get('key');
    const pinned = url.searchParams.get('signed_length');
    if (pinned === '44' || (pinned === null && key === 'admin')) {
      res.end(JSON.stringify({ key, value: workerData.state[key] ?? null, confirmed: true, signed_length: 44 }));
      return;
    }
  }
  res.writeHead(400);
  res.end('{}');
});
server.listen(0, '127.0.0.1', () => {
  const done = new Int32Array(workerData.ready);
  Atomics.store(done, 0, server.address().port);
  Atomics.notify(done, 0);
});

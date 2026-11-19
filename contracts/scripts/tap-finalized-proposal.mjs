import fs from 'node:fs';
import { fileURLToPath } from 'node:url';
import { Interface } from 'ethers';
import { POOL_SETTLEMENT_ABI } from './tap-settlement-roller.mjs';
import { rpcUrlCandidates } from './tap-price-watcher.mjs';

const events = new Interface(POOL_SETTLEMENT_ABI);
const hash = /^0x[0-9a-f]{64}$/i;
const address = /^0x[0-9a-f]{40}$/i;

function requireEvidence(condition, reason) {
  if (!condition) throw new Error('TAP proposal verification: ' + reason);
}

function quantity(value) {
  requireEvidence(typeof value === 'string' && /^0x(?:0|[1-9a-f][0-9a-f]*)$/i.test(value),
    'invalid Ethereum quantity');
  const n = Number(BigInt(value));
  requireEvidence(Number.isSafeInteger(n), 'Ethereum quantity out of range');
  return n;
}

function sameHex(a, b) {
  return typeof a === 'string' && typeof b === 'string' && a.toLowerCase() === b.toLowerCase();
}

// Exactly six indexed reads from the same endpoint. Never scan historical logs
// or reconstruct a missing receipt by issuing another monetary operation.
export async function verifyFinalizedTapProposal({ report, chainId, pool, read }) {
  const cp = report?.tap_settlement_checkpoint;
  requireEvidence(cp?.root_confirmed === true && report.root_confirmed === true &&
    cp.op === 'settle_targeted_tap' && cp.rail === 'tap' && cp.chain_id === chainId &&
    chainId === 1 && address.test(pool) && sameHex(cp.pool_address, pool) &&
    Number.isSafeInteger(cp.epoch) && cp.epoch > 0 && cp.epoch === report.epoch &&
    hash.test(cp.root) && sameHex(cp.root, report.root) && /^(0|[1-9][0-9]*)$/.test(cp.cumulative_spent_wei),
    'invalid archived identity');
  requireEvidence(quantity(await read('eth_chainId', [])) === chainId, 'wrong Ethereum network');
  const lookup = async (role, eventName) => {
    const tx = cp[role + '_tx'];
    const number = cp[role + '_block_number'];
    const blockHash = cp[role + '_block_hash'];
    requireEvidence(hash.test(tx) && Number.isSafeInteger(number) && number > 0 &&
      hash.test(blockHash), 'invalid archived ' + role);
    const [receipt, block] = await Promise.all([
      read('eth_getTransactionReceipt', [tx]),
      read('eth_getBlockByNumber', ['0x' + number.toString(16), false]),
    ]);
    requireEvidence(receipt && block && quantity(receipt.status) === 1 &&
      sameHex(receipt.transactionHash, tx) && quantity(receipt.blockNumber) === number &&
      quantity(block.number) === number && sameHex(receipt.blockHash, blockHash) &&
      sameHex(block.hash, blockHash) && Array.isArray(receipt.logs), 'unconfirmed ' + role);
    const topic = events.getEvent(eventName).topicHash;
    const matches = receipt.logs.filter(log => sameHex(log.address, pool) &&
      sameHex(log.topics?.[0], topic) && sameHex(log.transactionHash, tx) &&
      sameHex(log.blockHash, blockHash) && quantity(log.blockNumber) === number &&
      log.removed !== true).map(log => events.decodeEventLog(eventName, log.data, log.topics))
      .filter(args => args.epoch === BigInt(cp.epoch) && sameHex(args.merkleRoot, cp.root) &&
        args.cumulativeSpent === BigInt(cp.cumulative_spent_wei));
    requireEvidence(matches.length === 1, 'missing or ambiguous ' + eventName);
    return { number, hash: blockHash.toLowerCase(), timestamp: quantity(block.timestamp), event: matches[0] };
  };
  const [proposal, execution, finalized] = await Promise.all([
    lookup('proposal', 'RootProposed'), lookup('execution', 'RootPosted'),
    read('eth_getBlockByNumber', ['finalized', false]),
  ]);
  requireEvidence(hash.test(finalized?.hash) && Number.isSafeInteger(cp.finalized_block_number) &&
    cp.confirmation_policy === 'finalized-tag' && cp.execution_status === 1 &&
    cp.confirmation_depth === cp.finalized_block_number - execution.number &&
    cp.confirmation_depth >= 12 && quantity(finalized.number) >= cp.finalized_block_number &&
    proposal.number <= execution.number && proposal.timestamp > 0 &&
    proposal.event.nonce === execution.event.nonce &&
    BigInt(proposal.timestamp) <= proposal.event.executeAfter &&
    BigInt(execution.timestamp) >= proposal.event.executeAfter,
    'events, finality or governance delay disagree');
  return { number: proposal.number, hash: proposal.hash, timestamp: proposal.timestamp };
}

async function verifyFile(report) {
  const env = process.env;
  const test = env.MAYHEM_PAYOUT_TEST_MODE === '1';
  const urls = test ? [env.MAYHEM_PAYOUT_TEST_ETH_RPC] : rpcUrlCandidates({
    rpcUrl: env.MAYHEM_TAP_ETH_RPC, fallbackRpcUrls: env.MAYHEM_TAP_ETH_RPC_FALLBACKS,
  });
  const chainId = test ? 1 : Number(env.MAYHEM_TAP_ETH_CHAIN_ID);
  const pool = test ? report.tap_settlement_checkpoint.pool_address : env.MAYHEM_TAP_POOL_ADDRESS;
  for (const candidate of urls) {
    let failure;
    try {
      const url = new URL(candidate);
      requireEvidence(test ? url.origin === 'http://127.0.0.1:' + url.port :
        url.protocol === 'https:' || (url.protocol === 'http:' && ['127.0.0.1', '[::1]', 'localhost'].includes(url.hostname)),
        'invalid configured RPC transport');
      let next = 0;
      const signal = AbortSignal.timeout(8_000);
      const read = async (method, params) => {
        const id = ++next;
        const res = await fetch(url, { method: 'POST', redirect: 'error', signal,
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ jsonrpc: '2.0', id, method, params }) });
        requireEvidence(res.ok, 'RPC returned an HTTP error');
        const value = await res.json();
        requireEvidence(value.jsonrpc === '2.0' && value.id === id &&
          value.result != null && value.error == null, 'RPC response unavailable');
        return value.result;
      };
      return await verifyFinalizedTapProposal({ report, chainId, pool, read });
    } catch (error) {
      // Configuration can include secret URLs. Only surface our own fixed diagnostics.
      failure = String(error.message).startsWith('TAP proposal verification:') ?
        error.message : 'TAP proposal verification: could not read complete Ethereum evidence';
    }
    console.error(failure);
  }
  throw new Error('No configured Ethereum endpoint verified the finalized TAP proposal');
}

if (process.argv[1] && fs.existsSync(process.argv[1]) && fs.realpathSync(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    if (process.argv.length !== 3) throw new Error('Expected the archived TAP settlement report path');
    const report = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
    console.log(JSON.stringify(await verifyFile(report)));
  } catch {
    console.error('Could not verify finalized TAP proposal from archived settlement report');
    process.exitCode = 1;
  }
}

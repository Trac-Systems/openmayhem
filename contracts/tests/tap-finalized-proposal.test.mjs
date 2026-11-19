import assert from 'node:assert/strict';
import test from 'node:test';
import { Interface } from 'ethers';
import { verifyFinalizedTapProposal } from '../scripts/tap-finalized-proposal.mjs';
import { POOL_SETTLEMENT_ABI } from '../scripts/tap-settlement-roller.mjs';

const abi = new Interface(POOL_SETTLEMENT_ABI);
const hex = (byte, count = 32) => '0x' + byte.repeat(count);

function fixture() {
  const pool = hex('aa', 20);
  const root = hex('ee');
  const checkpoint = { op: 'settle_targeted_tap', rail: 'tap', chain_id: 1, pool_address: pool,
    root_confirmed: true, root, epoch: 19, cumulative_spent_wei: '308',
    proposal_tx: hex('33'), proposal_block_number: 101, proposal_block_hash: hex('99'),
    execution_tx: hex('44'), execution_block_number: 106, execution_block_hash: hex('88'),
    finalized_block_number: 120, confirmation_depth: 14, execution_status: 1, confirmation_policy: 'finalized-tag' };
  const report = { tap_settlement_checkpoint: checkpoint, root_confirmed: true, epoch: 19, root };
  const blocks = { '0x65': { number: '0x65', hash: hex('99'), timestamp: '0x300' },
    '0x6a': { number: '0x6a', hash: hex('88'), timestamp: '0x390' },
    finalized: { number: '0x80', hash: hex('77') } };
  const receipts = {};
  for (const [role, blockNumber, event, args] of [
    ['proposal', '0x65', 'RootProposed', [19n, root, 308n, 5n, 800n]],
    ['execution', '0x6a', 'RootPosted', [19n, root, 308n, 5n]],
  ]) {
    const tx = checkpoint[role + '_tx'];
    const blockHash = blocks[blockNumber].hash;
    receipts[tx] = { transactionHash: tx, blockHash, blockNumber, status: '0x1',
      logs: [{ ...abi.encodeEventLog(event, args), address: pool, blockNumber, blockHash, transactionHash: tx }] };
  }
  let calls = 0;
  async function read(method, [key]) {
    assert.ok(++calls <= 6);
    if (method === 'eth_chainId') return '0x1';
    if (method === 'eth_getTransactionReceipt') return receipts[key];
    assert.equal(method, 'eth_getBlockByNumber');
    return blocks[key];
  }
  return { checkpoint, blocks, receipts, check: () => verifyFinalizedTapProposal({ pool, chainId: 1, report, read }) };
}

test('recovers the actual finalized proposal time with six indexed reads', async () => {
  const f = fixture();
  assert.deepEqual(await f.check(), { number: 101, hash: hex('99'), timestamp: 768 });
});

for (const [why, change] of [
  ['failed proposals', f => { f.receipts[f.checkpoint.proposal_tx].status = '0x0'; }],
  ['noncanonical block', f => { f.blocks['0x65'].hash = hex('cc'); }],
  ['another pool', f => { f.receipts[f.checkpoint.proposal_tx].logs[0].address = hex('bb', 20); }],
  ['removed execution log', f => { f.receipts[f.checkpoint.execution_tx].logs[0].removed = true; }],
  ['ambiguous proposal', f => { const logs = f.receipts[f.checkpoint.proposal_tx].logs; logs.push(logs[0]); }],
  ['wrong event nonce', f => { f.receipts[f.checkpoint.execution_tx].logs[0].topics[2] = hex('55'); }],
  ['insufficient finality', f => { f.blocks.finalized.number = '0x6a'; }],
  ['execution before it was allowed', f => { f.blocks['0x6a'].timestamp = '0x310'; }],
  ['unbound transaction', f => { f.receipts[f.checkpoint.execution_tx].logs[0].transactionHash = hex('aa'); }],
]) {
  test('rejects ' + why, async () => {
    const f = fixture();
    change(f);
    await assert.rejects(f.check(), /TAP proposal verification/);
  });
}

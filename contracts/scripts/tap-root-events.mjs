import fs from 'node:fs';
import path from 'node:path';

const PAGE_BLOCKS = 10_000;
const PAGES_PER_ATTEMPT = 4;
const HASH = /^0x[0-9a-f]{64}$/i;
const height = value => Number.isSafeInteger(value) && value >= 0;

async function blockAtOrBefore(provider, timestamp, head) {
  let low = 0, high = head.number;
  while (low < high) {
    const mid = Math.ceil((low + high) / 2);
    const block = await provider.getBlock(mid);
    if (!block || !height(block.timestamp)) throw new Error('TAP event anchor block is unavailable');
    if (block.timestamp <= timestamp) low = mid; else high = mid - 1;
  }
  return Math.max(0, low - 1);
}

/** Persist bounded event lookups, not the chain's event history. Cached event
 * blocks are hints: each use re-reads the exact log from the current chain. */
export async function tapRootEventReader({ pool, epoch, rateTimestamp, file } = {}) {
  if (!height(epoch) || epoch < 1) throw new Error('TAP event epoch is invalid');
  const provider = pool.runner.provider;
  const [network, address] = await Promise.all([provider.getNetwork(), pool.getAddress()]);
  const identity = { version: 1, chain_id: network.chainId.toString(), pool: address.toLowerCase(), epoch };
  let state;
  if (file && fs.existsSync(file)) {
    if (fs.statSync(file).size > 16_384) throw new Error('TAP event cursor is too large');
    state = JSON.parse(fs.readFileSync(file, 'utf8'));
    if (Object.entries(identity).some(([k,v]) => state[k] !== v) || !height(state.start_block)
        || !state.events || typeof state.events !== 'object' || Array.isArray(state.events)) throw new Error('TAP event cursor identity is invalid');
    for (const [name, value] of Object.entries(state.events)) {
      if (!['RootPosted','RootProposed'].includes(name) || !height(value.next_block)
          || value.next_block < state.start_block || (value.block !== undefined &&
            (!height(value.block) || value.block < state.start_block || !HASH.test(value.tx)))) {
        throw new Error('TAP event cursor position is invalid');
      }
    }
  } else {
    const [head, pending, currentEpoch] = await Promise.all([
      provider.getBlock('latest'), pool.pendingRoot(), pool.epoch(),
    ]);
    if (!head || !height(head.number)) throw new Error('TAP event head is unavailable');
    let start = Math.max(0, head.number - 64);
    const pendingEpoch = Number(pending.newEpoch ?? pending[1]);
    const executeAfter = Number(pending.executeAfter ?? pending[6]);
    if (pendingEpoch === epoch && executeAfter > 0) {
      const proposalTime = executeAfter - Number(await pool.governanceDelay());
      if (!height(proposalTime)) throw new Error('TAP proposal time is invalid');
      start = await blockAtOrBefore(provider, proposalTime, head);
    } else if (Number(currentEpoch) === epoch) {
      // Upgrade recovery for a root executed before a cursor existed. The
      // immutable rate predates its proposal; subsequent pages resume on disk.
      if (!height(rateTimestamp)) throw new Error('TAP recovery rate timestamp is invalid');
      start = await blockAtOrBefore(provider, rateTimestamp, head);
    }
    state = { ...identity, start_block: start, events: {} };
  }
  const save = () => {
    if (!file) return;
    fs.mkdirSync(path.dirname(file), { recursive: true, mode: 0o700 });
    const temporary = `${file}.${process.pid}.tmp`;
    const fd = fs.openSync(temporary, 'w', 0o600);
    try { fs.writeFileSync(fd, JSON.stringify(state)+'\n'); fs.fsyncSync(fd); }
    finally { fs.closeSync(fd); }
    fs.renameSync(temporary, file);
    const parent = fs.openSync(path.dirname(file), 'r');
    try { fs.fsyncSync(parent); } finally { fs.closeSync(parent); }
  };
  save(); // Retain the starting block before any transaction is broadcast.
  return async ({ name, root, cumulativeSpentWei, nonce, throughBlock }) => {
    if (!['RootPosted','RootProposed'].includes(name) || !height(throughBlock)) throw new Error('Invalid TAP event lookup');
    const expected = { root: root.toLowerCase(), spent: cumulativeSpentWei.toString() };
    if (state.intent && (state.intent.root !== expected.root || state.intent.spent !== expected.spent)) {
      throw new Error('TAP event cursor conflicts with the prepared root');
    }
    state.intent = expected;
    const entry = state.events[name] ??= { next_block: state.start_block };
    const matches = event => String(event.args?.merkleRoot).toLowerCase() === expected.root
      && String(event.args?.cumulativeSpent) === expected.spent
      && (nonce === undefined || String(event.args?.nonce) === String(nonce));
    const find = async (from, to) => {
      const logs = await pool.queryFilter(pool.filters[name](epoch), from, to);
      if (logs.some(event => !height(event.blockNumber) || event.blockNumber < from || event.blockNumber > to)) {
        throw new Error('TAP event response is outside the requested range');
      }
      const found = logs.filter(matches);
      if (found.length > 1) throw new Error('TAP root event evidence is ambiguous');
      return found[0];
    };
    if (entry.block !== undefined && entry.block <= throughBlock) {
      const known = await find(entry.block, entry.block);
      if (known && known.transactionHash === entry.tx) return known;
      // A pre-finality reorg invalidates the hint, never the signed intent.
      entry.next_block = Math.max(state.start_block, entry.block - 64);
      delete entry.block; delete entry.tx; save();
    }
    for (let page = 0; page < PAGES_PER_ATTEMPT && entry.next_block <= throughBlock; page++) {
      const end = Math.min(throughBlock, entry.next_block + PAGE_BLOCKS - 1);
      const event = await find(entry.next_block, end);
      if (event) {
        if (!height(event.blockNumber) || !HASH.test(event.transactionHash)) throw new Error('Invalid TAP root event identity');
        entry.block = event.blockNumber; entry.tx = event.transactionHash; save(); return event;
      }
      entry.next_block = end + 1; save();
    }
    const error = new Error(`TAP ${name} lookup is pending; retained cursor will resume`);
    error.code = 'TAP_EVENT_LOOKUP_PENDING';
    throw error;
  };
}

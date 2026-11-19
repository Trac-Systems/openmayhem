// The reconciled checkpoint authenticates the execution header hash. Prove the
// pool's account and both append-only mapping entries at that exact state root.
import { keccak256 as digest } from 'ethereum-cryptography/keccak';
import b4a from 'b4a';

const keccak256 = hex => '0x' + b4a.toString(digest(b4a.from(hex.slice(2), 'hex')), 'hex');

const hash = x => typeof x === 'string' && /^0x[0-9a-f]{64}$/.test(x);
const bytes = (x, limit) => typeof x === 'string' && x.length <= 2 + limit * 2 && /^0x(?:[0-9a-f]{2})+$/.test(x);
function need(ok) { if (!ok) throw new Error('Invalid finalized TAP epoch exclusion proof.'); }

// Decode the bounded RLP subset used for headers, accounts and hexary trie
// nodes. The reducer runs in Bare as well as Node. No RPC/Node codec imports.
function decodeRlp(encoded) {
  need(bytes(encoded, 2048));
  const raw = b4a.from(encoded.slice(2), 'hex');
  let pos = 0;
  function item(end, depth) {
    need(pos < end && depth <= 16);
    const tag = raw[pos++];
    if (tag < 128) return '0x' + tag.toString(16).padStart(2, '0');
    const list = tag >= 192;
    let length = tag - (list ? 192 : 128);
    if (length > 55) {
      const count = length - 55;
      need(count <= 2 && pos + count <= end && raw[pos] !== 0);
      length = 0;
      for (let i = 0; i < count; i++) length = length * 256 + raw[pos++];
      need(length > 55);
    }
    const stop = pos + length;
    need(stop <= end);
    if (list) {
      const parts = [];
      while (pos < stop) parts.push(item(stop, depth + 1));
      return parts;
    }
    need(length !== 1 || raw[pos] >= 128);
    const value = '0x' + b4a.toString(raw.subarray(pos, stop), 'hex');
    pos = stop;
    return value;
  }
  const result = item(raw.length, 0);
  need(pos === raw.length);
  return result;
}

function uintWord(value) {
  const n = BigInt(value);
  need(n >= 0n && n < (1n << 256n));
  return '0x' + n.toString(16).padStart(64, '0');
}

export function tapEpochSlot(epoch) {
  // MayhemInferencePool.merkleRootAtEpoch, pinned Solidity storage layout.
  return keccak256(uintWord(epoch) + uintWord(19).slice(2));
}

// Both tries use Keccak-hashed 32-byte paths. A valid mismatch at a leaf or
// extension, or an empty child at a branch, proves non-membership. Truncation
// never does. Embedded children are authenticated directly by their parent.
function trieValue(root, key, nodes) {
  need(hash(root) && hash(key) && Array.isArray(nodes) && nodes.length <= 65);
  if (nodes.length === 0) { need(root === keccak256('0x80')); return null; }
  const path = key.slice(2);
  let ref = root, offset = 0, used = 0;
  const finish = value => { need(used === nodes.length); return value; };
  for (let depth = 0; depth <= 64; depth++) {
    let node = ref;
    if (!Array.isArray(ref)) {
      const encoded = nodes[used++];
      need(hash(ref) && bytes(encoded, 1024) && keccak256(encoded) === ref);
      node = decodeRlp(encoded);
    }
    need(Array.isArray(node));
    if (node.length === 17) {
      if (offset === 64) return finish(node[16] === '0x' ? null : node[16]);
      ref = node[parseInt(path[offset++], 16)];
      if (ref === '0x') return finish(null);
      continue;
    }
    need(node.length === 2 && bytes(node[0], 33));
    const flag = parseInt(node[0][2], 16);
    need(flag <= 3 && (flag % 2 === 1 || node[0][3] === '0'));
    const segment = node[0].slice(flag % 2 ? 3 : 4);
    const leaf = flag >= 2;
    need((segment.length > 0 || leaf) && offset + segment.length <= 64);
    if (!path.startsWith(segment, offset)) return finish(null);
    offset += segment.length;
    if (leaf) return finish(offset === 64 ? node[1] : null);
    ref = node[1];
  }
  throw new Error('Invalid TAP proof path length.');
}

export function tapClaimedSlot(account) {
  need(typeof account === 'string' && /^0x[0-9a-f]{40}$/.test(account));
  return keccak256(uintWord(account) + uintWord(22).slice(2));
}

// Public pool storage layout: current root, epoch, cumulativeSpent.
export const tapPoolHeadSlots = [18, 20, 21].map(uintWord);
export const tapPendingSlots = [4, 5, 6, 7, 8].map(uintWord);
// EIP-1186 repeats branch nodes across wallet and slot paths. Intern the
// identical bytes across all blocks, using base64 on the Core wire.
function mapPaths(proof, map) {
  const result = { ...proof };
  for (const key of ['account','posted','skipped','parent_account','proposal_account','before_proposal_account','before_proposal_pending']) {
    if (result[key] != null) result[key] = map(result[key]);
  }
  for (const key of ['head','parent_head','proposal_pending','parent_pending']) {
    if (result[key] !== undefined) {
      need(Array.isArray(result[key]) && result[key].length <= 5);
      result[key] = result[key].map(map);
    }
  }
  for (const key of ['claimed','parent_claimed']) {
    if (result[key] !== undefined) {
      need(result[key] && typeof result[key] === 'object' && Object.keys(result[key]).length <= 64);
      result[key] = Object.fromEntries(Object.entries(result[key]).map(([account, path]) => [account, map(path)]));
    }
  }
  return result;
}
export function packTapProof(proof) {
  const index = new Map(), nodes = [];
  const result = mapPaths(proof, path => {
    need(Array.isArray(path) && path.length <= 65);
    return path.map(node => {
      need(bytes(node, 1024));
      if (!index.has(node)) {
        index.set(node, nodes.length);
        nodes.push(b4a.toString(b4a.from(node.slice(2), 'hex'), 'base64'));
      }
      return index.get(node);
    });
  });
  return { ...result, nodes };
}
function unpackTapProof(proof) {
  need(Array.isArray(proof?.nodes) && proof.nodes.length > 0 && proof.nodes.length <= 4096);
  const nodes = proof.nodes.map(value => {
    need(typeof value === 'string' && value.length > 0 && value.length <= 1368);
    const data = b4a.from(value, 'base64');
    need(data.length <= 1024 && b4a.toString(data, 'base64') === value);
    return '0x' + b4a.toString(data, 'hex');
  });
  return mapPaths(proof, path => {
    need(Array.isArray(path) && path.length <= 65);
    return path.map(i => { need(Number.isInteger(i) && i >= 0 && i < nodes.length); return nodes[i]; });
  });
}
function storageRoot(header, pool, proof) {
  const account = decodeRlp(trieValue(header[3], keccak256(pool), proof));
  need(Array.isArray(account) && account.length === 4 && hash(account[2]));
  return account[2];
}
function verifySlots(root, slots, paths, expected) {
  need(Array.isArray(paths) && paths.length === slots.length);
  for (const [i, slot] of slots.entries()) {
    const value = trieValue(root, keccak256(slot), paths[i]);
    need((value === null ? 0n : BigInt(decodeRlp(value))) === BigInt(expected[i]));
  }
}
export function verifySkippedTapEpoch(cp, oldEpoch, proof, ceilings, opening) {
  need(oldEpoch < cp.epoch);
  verifyPoolTransition(cp, oldEpoch, proof, ceilings, opening);
}
export function verifyWinningTapEpoch(cp, old, proof, ceilings, opening, winningEntries) {
  need(old.epoch === cp.epoch && old.root !== cp.root && proof.skipped === null);
  verifyPoolTransition(cp, old.epoch, proof, ceilings, opening, winningEntries);
}
export function verifyPostedTapEpoch(cp, old, proof, ceilings, opening, entries) {
  need(old.epoch === cp.epoch && old.root === cp.root && proof.skipped === null);
  verifyPoolTransition(cp, old.epoch, proof, ceilings, opening, entries);
}
function verifyClaims(root, paths, ceilings) {
  need(paths && typeof paths === 'object' && !Array.isArray(paths) &&
    Object.keys(paths).length === ceilings.size && ceilings.size <= 64);
  for (const [payee, ceiling] of ceilings) {
    const stored = trieValue(root, keccak256(tapClaimedSlot(payee)), paths[payee]);
    need((stored === null ? 0n : BigInt(decodeRlp(stored))) <= ceiling);
  }
}
function verifyPoolTransition(cp, oldEpoch, proof, ceilings, opening, winningEntries) {
  proof = unpackTapProof(proof);
  need(cp.chain_id === 1 && cp.confirmation_policy === 'finalized-tag' && cp.root_confirmed === true &&
    cp.execution_status === 1 && cp.confirmation_depth >= 12 &&
    cp.finalized_block_number - cp.execution_block_number === cp.confirmation_depth &&
    Number.isSafeInteger(oldEpoch) && oldEpoch > 0 && oldEpoch <= cp.epoch &&
    bytes(proof?.header, 2048) && keccak256(proof.header) === cp.execution_block_hash);
  const header = decodeRlp(proof.header);
  need(Array.isArray(header) && header.length >= 15 && hash(header[3]) &&
    BigInt(header[8]) === BigInt(cp.execution_block_number));
  const root = storageRoot(header, cp.pool_address, proof.account);
  verifySlots(root, tapPoolHeadSlots, proof.head, [cp.root, cp.epoch, cp.cumulative_spent_wei]);
  // The public pool's immutable delay precludes two root executions in one
  // block. Its parent head must be the exact opening: an unknown intervening
  // root can still pay forever, even with a zero claimed[] at this block.
  need(bytes(proof.parent_header, 2048) && keccak256(proof.parent_header) === header[0]);
  const parent = decodeRlp(proof.parent_header);
  need(Array.isArray(parent) && parent.length >= 15 && BigInt(parent[8]) + 1n === BigInt(header[8]));
  const prior = storageRoot(parent, cp.pool_address, proof.parent_account);
  verifySlots(prior, tapPoolHeadSlots, proof.parent_head, [opening.root ?? 0, opening.epoch ?? 0, opening.cumulative_spent_wei]);
  const entry = epoch => keccak256(tapEpochSlot(epoch));
  const posted = trieValue(root, entry(cp.epoch), proof.posted);
  // Storage integers are RLP-encoded minimal big-endian values, including
  // bytes32 roots whose first bytes are zero.
  need(posted !== null && uintWord(decodeRlp(posted)) === cp.root);
  if (winningEntries) {
    // Pin the price clock to the actual block that created this pending
    // proposal. The ordinary admin checkpoint authenticates both block hashes.
    need(bytes(proof.proposal_header, 2048) && keccak256(proof.proposal_header) === cp.proposal_block_hash);
    const proposal = decodeRlp(proof.proposal_header);
    need(Array.isArray(proposal) && proposal.length >= 15 && BigInt(proposal[8]) === BigInt(cp.proposal_block_number) &&
      BigInt(proposal[11]) === BigInt(cp.proposal_block_timestamp) && BigInt(proposal[8]) < BigInt(header[8]) &&
      bytes(proof.before_proposal_header, 2048) && keccak256(proof.before_proposal_header) === proposal[0]);
    const before = decodeRlp(proof.before_proposal_header);
    need(Array.isArray(before) && before.length >= 15 && BigInt(before[8]) + 1n === BigInt(proposal[8]));
    verifySlots(storageRoot(before, cp.pool_address, proof.before_proposal_account), [tapPendingSlots[0]],
      [proof.before_proposal_pending], [0]);
    const pending = [cp.root, cp.epoch, cp.cumulative_spent_wei, opening.epoch ?? 0, opening.cumulative_spent_wei];
    verifySlots(storageRoot(proposal, cp.pool_address, proof.proposal_account), tapPendingSlots, proof.proposal_pending, pending);
    verifySlots(prior, tapPendingSlots, proof.parent_pending, pending);
    verifyClaims(prior, proof.parent_claimed, ceilings);
    const limits = new Map([...ceilings.keys()].map(payee => [payee, winningEntries.get(payee) ?? 0n]));
    verifyClaims(root, proof.claimed, limits);
  } else {
    need(trieValue(root, entry(oldEpoch), proof.skipped) === null);
    verifyClaims(root, proof.claimed, ceilings);
  }
}

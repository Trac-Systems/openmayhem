// Off-chain Merkle distributor helper - matches the on-chain KnowledgePool verification exactly:
//   - leaf = keccak256(bytes.concat(keccak256(abi.encode(account, amount))))  (OZ StandardMerkleTree
//     double-hash encoding - second-preimage safe),
//   - internal nodes = COMMUTATIVE keccak256 (sorted pair), the OZ MerkleProof default.
// This is the shape the Phase-5 settlement roller will use to build the cumulative {key->owed} root.
import { ethers } from 'ethers';

const abi = ethers.AbiCoder.defaultAbiCoder();

/** leaf hash for a (account, cumulativeAmount) distribution entry. */
export function leafHash(account, amount) {
  const inner = ethers.keccak256(abi.encode(['address', 'uint256'], [account, amount]));
  return ethers.keccak256(inner);
}

/** commutative pair hash (sorts the two 32-byte words, like OZ Hashes.commutativeKeccak256). */
function hashPair(a, b) {
  const [x, y] = a.toLowerCase() <= b.toLowerCase() ? [a, b] : [b, a];
  return ethers.keccak256(ethers.concat([x, y]));
}

/** Build the layered tree (odd tail carried up - no duplication). */
export function buildTree(leaves) {
  if (!leaves.length) throw new Error('merkle: no leaves');
  const layers = [leaves.slice()];
  while (layers[layers.length - 1].length > 1) {
    const prev = layers[layers.length - 1];
    const next = [];
    for (let i = 0; i < prev.length; i += 2) {
      next.push(i + 1 < prev.length ? hashPair(prev[i], prev[i + 1]) : prev[i]);
    }
    layers.push(next);
  }
  return layers;
}

export function rootOf(layers) { return layers[layers.length - 1][0]; }

export function proofOf(layers, index) {
  const p = [];
  let idx = index;
  for (let l = 0; l < layers.length - 1; l++) {
    const layer = layers[l];
    const sib = idx ^ 1;
    if (sib < layer.length) p.push(layer[sib]);
    idx >>= 1;
  }
  return p;
}

/**
 * Build a distribution from entries [{account, amount(bigint)}].
 * @returns {{root:string, proofFor:(acct:string)=>string[], leafFor:(acct:string)=>string}}
 */
export function distribution(entries) {
  const leaves = entries.map((e) => leafHash(e.account, e.amount));
  const layers = buildTree(leaves);
  const idx = new Map(entries.map((e, i) => [e.account.toLowerCase(), i]));
  return {
    root: rootOf(layers),
    proofFor: (account) => proofOf(layers, idx.get(account.toLowerCase())),
    leafFor: (account) => leaves[idx.get(account.toLowerCase())],
  };
}

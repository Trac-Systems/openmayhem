// Off-chain Merkle distributor helper - matches the on-chain MayhemInferencePool verification exactly:
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

/** Build OpenZeppelin's sorted complete-tree heap layout. */
export function buildTree(leaves) {
  if (!leaves.length) throw new Error('merkle: no leaves');
  const sorted = leaves.slice().sort((left, right) => (
    left.toLowerCase().localeCompare(right.toLowerCase())
  ));
  const tree = new Array(2 * sorted.length - 1);
  for (const [index, leaf] of sorted.entries()) {
    tree[tree.length - 1 - index] = leaf;
  }
  for (let index = tree.length - 1 - sorted.length; index >= 0; index -= 1) {
    tree[index] = hashPair(tree[2 * index + 1], tree[2 * index + 2]);
  }
  return tree;
}

export function rootOf(tree) { return tree[0]; }

export function proofOf(tree, index) {
  const proof = [];
  let idx = index;
  while (idx > 0) {
    const sibling = idx - (-1) ** (idx % 2);
    proof.push(tree[sibling]);
    idx = Math.floor((idx - 1) / 2);
  }
  return proof;
}

/**
 * Build a distribution from entries [{account, amount(bigint)}].
 * @returns {{root:string, proofFor:(acct:string)=>string[], leafFor:(acct:string)=>string}}
 */
export function distribution(entries) {
  const indexedLeaves = entries.map((entry) => ({
    account: entry.account.toLowerCase(),
    leaf: leafHash(entry.account, entry.amount),
  }));
  if (new Set(indexedLeaves.map((entry) => entry.account)).size !== entries.length) {
    throw new Error('merkle: duplicate account');
  }
  const tree = buildTree(indexedLeaves.map((entry) => entry.leaf));
  const sortedLeaves = indexedLeaves.slice().sort((left, right) => (
    left.leaf.toLowerCase().localeCompare(right.leaf.toLowerCase())
  ));
  const treeIndex = new Map(sortedLeaves.map((entry, index) => (
    [entry.account, tree.length - index - 1]
  )));
  const leafByAccount = new Map(indexedLeaves.map((entry) => [entry.account, entry.leaf]));
  return {
    root: rootOf(tree),
    proofFor: (account) => {
      const index = treeIndex.get(account.toLowerCase());
      if (index === undefined) throw new Error('merkle: account not found');
      return proofOf(tree, index);
    },
    leafFor: (account) => {
      const leaf = leafByAccount.get(account.toLowerCase());
      if (leaf === undefined) throw new Error('merkle: account not found');
      return leaf;
    },
  };
}

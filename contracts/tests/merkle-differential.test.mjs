import assert from 'node:assert/strict';
import test from 'node:test';

import { StandardMerkleTree } from '@openzeppelin/merkle-tree';
import { ethers } from 'ethers';

import { distribution } from '../scripts/merkle.mjs';

const LEAF_ENCODING = ['address', 'uint256'];

function entries(count, offset = 0) {
  return Array.from({ length: count }, (_, index) => ({
    account: ethers.getAddress(ethers.zeroPadValue(ethers.toBeHex(offset + index + 1), 20)),
    amount: BigInt((offset + index + 1) * 7919),
  }));
}

function assertMatchesOpenZeppelin(input) {
  const custom = distribution(input);
  const values = input.map(({ account, amount }) => [account, amount.toString()]);
  const standard = StandardMerkleTree.of(values, LEAF_ENCODING);

  assert.equal(custom.root, standard.root);
  for (const { account, amount } of input) {
    const proof = custom.proofFor(account);
    assert.equal(
      StandardMerkleTree.verify(
        custom.root,
        LEAF_ENCODING,
        [account, amount.toString()],
        proof,
      ),
      true,
    );
    assert.deepEqual(proof, standard.getProof([account, amount.toString()]));
  }
}

test('custom distribution exactly matches OpenZeppelin for singleton and odd leaf counts', () => {
  for (const count of [1, 3, 5, 7, 9, 17]) {
    assertMatchesOpenZeppelin(entries(count));
  }
});

test('custom distribution is order-independent and matches OpenZeppelin across varied sizes', () => {
  for (let count = 2; count <= 64; count += 1) {
    const input = entries(count, count * 100);
    const reordered = input.slice().sort((left, right) => (
      left.account.toLowerCase() < right.account.toLowerCase() ? 1 : -1
    ));
    assertMatchesOpenZeppelin(reordered);
    assert.equal(distribution(input).root, distribution(reordered).root);
  }
});

test('distribution rejects duplicate accounts and unknown proof targets', () => {
  const input = entries(2);
  assert.throws(
    () => distribution([input[0], { ...input[0], amount: input[0].amount + 1n }]),
    /duplicate account/,
  );
  assert.throws(
    () => distribution(input).proofFor('0xffffffffffffffffffffffffffffffffffffffff'),
    /account not found/,
  );
});

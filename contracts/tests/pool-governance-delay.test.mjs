import assert from 'node:assert/strict';
import test from 'node:test';

import Ganache from 'ganache';
import { ethers } from 'ethers';

import { compileAll } from '../scripts/compile.mjs';
import { distribution } from '../scripts/merkle.mjs';
import {
  signMaxEpochDeltaProposal,
  signRescueProposal,
  signRootProposal,
} from '../scripts/pool-governance.mjs';

const GOVERNANCE_DELAY = 3600n;
const MAX_EPOCH_DELTA = ethers.parseUnits('10000', 18);

async function setup() {
  const ganache = Ganache.provider({
    logging: { quiet: true },
    chain: { chainId: 1 },
    wallet: { totalAccounts: 3 },
  });
  const provider = new ethers.BrowserProvider(ganache);
  const owner = await provider.getSigner(0);
  const executor = await provider.getSigner(1);
  const governance = ethers.Wallet.createRandom();
  const artifacts = compileAll();
  const tokenFactory = new ethers.ContractFactory(
    artifacts.MockTTAP.abi,
    artifacts.MockTTAP.bytecode,
    owner,
  );
  const token = await tokenFactory.deploy();
  await token.waitForDeployment();
  const poolFactory = new ethers.ContractFactory(
    artifacts.MayhemInferencePool.abi,
    artifacts.MayhemInferencePool.bytecode,
    owner,
  );
  return { ganache, provider, owner, executor, governance, token, poolFactory };
}

test('public-chain pool rejects a zero delay and zero epoch cap', async () => {
  const { owner, governance, token, poolFactory } = await setup();
  await assert.rejects(async () => {
    const pool = await poolFactory.deploy(
      await token.getAddress(),
      await owner.getAddress(),
      governance.address,
      0n,
      MAX_EPOCH_DELTA,
    );
    await pool.waitForDeployment();
  });
  await assert.rejects(async () => {
    const pool = await poolFactory.deploy(
      await token.getAddress(),
      await owner.getAddress(),
      governance.address,
      GOVERNANCE_DELAY,
      0n,
    );
    await pool.waitForDeployment();
  });
});

test('owner rotation preserves two-key governance and cannot renounce a live escrow', async () => {
  const { owner, executor, governance, token, poolFactory } = await setup();
  const pool = await poolFactory.deploy(
    await token.getAddress(),
    await owner.getAddress(),
    governance.address,
    GOVERNANCE_DELAY,
    MAX_EPOCH_DELTA,
  );
  await pool.waitForDeployment();

  await assert.rejects(
    pool.transferOwnership.staticCall(governance.address),
    /owner=governance signer/,
  );
  await assert.rejects(pool.renounceOwnership.staticCall(), /owner required/);

  const nextOwner = await executor.getAddress();
  await (await pool.transferOwnership(nextOwner)).wait();
  await (await pool.connect(executor).acceptOwnership()).wait();
  assert.equal(await pool.owner(), nextOwner);
  assert.notEqual(await pool.owner(), await pool.governanceSigner());
});

test('public-chain root is cross-signed and cannot execute before its immutable delay', async () => {
  const { ganache, owner, executor, governance, token, poolFactory } = await setup();
  const pool = await poolFactory.deploy(
    await token.getAddress(),
    await owner.getAddress(),
    governance.address,
    GOVERNANCE_DELAY,
    MAX_EPOCH_DELTA,
  );
  await pool.waitForDeployment();

  const entry = {
    account: await executor.getAddress(),
    amount: 0n,
  };
  const root = distribution([entry]).root;
  const signature = await signRootProposal({
    signer: governance,
    pool,
    merkleRoot: root,
    newEpoch: 1,
    newCumulativeSpent: 0n,
  });
  await (await pool.proposeRoot(root, 1, 0, signature)).wait();
  await assert.rejects(pool.connect(executor).executeRoot.staticCall(), /governance delay/);

  await ganache.request({ method: 'evm_increaseTime', params: [Number(GOVERNANCE_DELAY)] });
  await ganache.request({ method: 'evm_mine', params: [] });
  await (await pool.connect(executor).executeRoot()).wait();

  assert.equal(await pool.epoch(), 1n);
  assert.equal(await pool.merkleRootAtEpoch(1), root);
});

test('public-chain epoch-cap change cannot execute before its immutable delay', async () => {
  const { ganache, owner, executor, governance, token, poolFactory } = await setup();
  const pool = await poolFactory.deploy(
    await token.getAddress(),
    await owner.getAddress(),
    governance.address,
    GOVERNANCE_DELAY,
    MAX_EPOCH_DELTA,
  );
  await pool.waitForDeployment();

  const newMax = ethers.parseUnits('9000', 18);
  const signature = await signMaxEpochDeltaProposal({
    signer: governance,
    pool,
    newMaxEpochDelta: newMax,
  });
  await (await pool.proposeMaxEpochDelta(newMax, signature)).wait();
  await assert.rejects(
    pool.connect(executor).executeMaxEpochDelta.staticCall(),
    /governance delay/,
  );

  await ganache.request({ method: 'evm_increaseTime', params: [Number(GOVERNANCE_DELAY)] });
  await ganache.request({ method: 'evm_mine', params: [] });
  await (await pool.connect(executor).executeMaxEpochDelta()).wait();

  assert.equal(await pool.maxEpochDelta(), newMax);
});

test('public-chain surplus rescue cannot execute before its immutable delay', async () => {
  const { ganache, owner, executor, governance, token, poolFactory } = await setup();
  const pool = await poolFactory.deploy(
    await token.getAddress(),
    await owner.getAddress(),
    governance.address,
    GOVERNANCE_DELAY,
    MAX_EPOCH_DELTA,
  );
  await pool.waitForDeployment();

  const amount = ethers.parseUnits('1', 18);
  await (await token.mint(await pool.getAddress(), amount)).wait();
  const recipient = await executor.getAddress();
  const signature = await signRescueProposal({
    signer: governance,
    pool,
    to: recipient,
    amount,
  });
  await (await pool.proposeRescue(recipient, amount, signature)).wait();
  await assert.rejects(pool.connect(executor).executeRescue.staticCall(), /governance delay/);

  await ganache.request({ method: 'evm_increaseTime', params: [Number(GOVERNANCE_DELAY)] });
  await ganache.request({ method: 'evm_mine', params: [] });
  await (await pool.connect(executor).executeRescue()).wait();

  assert.equal(await token.balanceOf(recipient), amount);
  assert.equal(await pool.rescuableSurplus(), 0n);
});

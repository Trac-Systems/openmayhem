// Deploy MockTTAP + MayhemInferencePool locally. As a CLI it connects to MAYHEM_TAP_ETH_RPC and writes
// .mayhem-local/contracts/eth-addresses.json for the oracle
// runners. As a module it exports deployPool(signer) so the in-process oracle e2e can deploy + wire
// the same contracts without a standing node. No external network.
import { ethers } from 'ethers';
import { writeFileSync, mkdirSync } from 'node:fs';
import { dirname } from 'node:path';
import { compileAll } from './compile.mjs';
import { ADDRESSES_FILE } from './paths.mjs';
import {
  TAP_DEPLOYER_SIGNER_ENV,
  walletFromEnv,
} from './signer-env.mjs';

/** Deploy ONLY the MayhemInferencePool bound to an EXISTING token (no mock). Public deployments should call this
 *  with canonical TAP. `maxEpochDelta` is the C1 per-epoch spend cap (0 = disabled). */
export async function deployPoolWithToken(signer, tokenAddr, ownerAddr, maxEpochDelta = 0n, art = compileAll()) {
  const owner = ownerAddr || (await signer.getAddress());
  const pool = await new ethers.ContractFactory(art.MayhemInferencePool.abi, art.MayhemInferencePool.bytecode, signer).deploy(tokenAddr, owner, maxEpochDelta);
  await pool.waitForDeployment();
  return { pool, poolAddr: await pool.getAddress(), art };
}

/** Deploy both contracts with `signer`; pool owner defaults to the signer. `maxEpochDelta` is the C1
 *  per-epoch spend cap (0 = disabled, the local default; set a real ceiling for public networks). Local/test only:
 *  deploys a MockTTAP. Public deployments must bind deployPoolWithToken() to canonical TAP. */
export async function deployPool(signer, ownerAddr, maxEpochDelta = 0n) {
  const art = compileAll();
  const token = await new ethers.ContractFactory(art.MockTTAP.abi, art.MockTTAP.bytecode, signer).deploy();
  await token.waitForDeployment();
  const tokenAddr = await token.getAddress();
  const { pool, poolAddr } = await deployPoolWithToken(signer, tokenAddr, ownerAddr, maxEpochDelta, art);
  return { token, pool, tokenAddr, poolAddr, art };
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const rpc = process.env.MAYHEM_TAP_ETH_RPC || 'http://127.0.0.1:61000';
  const provider = new ethers.JsonRpcProvider(rpc);
  try {
    const { envName, wallet } = walletFromEnv(provider, {
      names: [TAP_DEPLOYER_SIGNER_ENV],
      label: 'TAP deployer private key',
    });
    const signer = new ethers.NonceManager(wallet);
    // C1: a real per-epoch spend cap can be set via MAYHEM_TAP_MAX_EPOCH_DELTA (wei); 0 locally.
    const maxEpochDelta = process.env.MAYHEM_TAP_MAX_EPOCH_DELTA ? BigInt(process.env.MAYHEM_TAP_MAX_EPOCH_DELTA) : 0n;
    const { tokenAddr, poolAddr } = await deployPool(signer, undefined, maxEpochDelta);
    const net = await provider.getNetwork();
    const out = { pool: poolAddr, token: tokenAddr, deployer: await signer.getAddress(), signerEnv: envName, chainId: Number(net.chainId), rpc, maxEpochDelta: maxEpochDelta.toString() };
    mkdirSync(dirname(ADDRESSES_FILE), { recursive: true });
    writeFileSync(ADDRESSES_FILE, JSON.stringify(out, null, 2) + '\n');
    console.log('deployed ->', ADDRESSES_FILE, '\n', out);
  } finally {
    if (provider.destroy) provider.destroy();
  }
}

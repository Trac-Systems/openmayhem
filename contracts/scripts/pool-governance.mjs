import { ethers } from 'ethers';

export const POOL_GOVERNANCE_DOMAIN_NAME = 'MayhemInferencePool';
export const POOL_GOVERNANCE_DOMAIN_VERSION = '1';

export const ROOT_PROPOSAL_TYPES = {
  RootProposal: [
    { name: 'merkleRoot', type: 'bytes32' },
    { name: 'newEpoch', type: 'uint256' },
    { name: 'newCumulativeSpent', type: 'uint256' },
    { name: 'previousEpoch', type: 'uint256' },
    { name: 'previousCumulativeSpent', type: 'uint256' },
    { name: 'nonce', type: 'uint256' },
  ],
};

export const MAX_EPOCH_DELTA_PROPOSAL_TYPES = {
  MaxEpochDeltaProposal: [
    { name: 'newMaxEpochDelta', type: 'uint256' },
    { name: 'nonce', type: 'uint256' },
  ],
};

export const RESCUE_PROPOSAL_TYPES = {
  RescueProposal: [
    { name: 'to', type: 'address' },
    { name: 'amount', type: 'uint256' },
    { name: 'nonce', type: 'uint256' },
  ],
};

function nonNegativeBigInt(value, label) {
  try {
    const parsed = BigInt(value);
    if (parsed < 0n) throw new Error();
    return parsed;
  } catch (_error) {
    throw new Error(`${label} must be a non-negative integer`);
  }
}

export function poolGovernanceDomain({ chainId, poolAddress } = {}) {
  const normalizedChainId = Number(chainId);
  if (!Number.isSafeInteger(normalizedChainId) || normalizedChainId <= 0) {
    throw new Error('chainId must be a positive safe integer');
  }
  return {
    name: POOL_GOVERNANCE_DOMAIN_NAME,
    version: POOL_GOVERNANCE_DOMAIN_VERSION,
    chainId: normalizedChainId,
    verifyingContract: ethers.getAddress(poolAddress),
  };
}

export async function poolGovernanceContext(pool) {
  if (!pool?.runner?.provider) throw new Error('Pool contract has no provider');
  const [network, poolAddress] = await Promise.all([
    pool.runner.provider.getNetwork(),
    pool.getAddress(),
  ]);
  return {
    domain: poolGovernanceDomain({ chainId: network.chainId, poolAddress }),
    nonce: nonNegativeBigInt(await pool.governanceNonce(), 'governance nonce'),
  };
}

export async function signRootProposal({
  signer,
  pool,
  merkleRoot,
  newEpoch,
  newCumulativeSpent,
  previousEpoch,
  previousCumulativeSpent,
  nonce,
} = {}) {
  if (!signer?.signTypedData) throw new Error('Missing governance signer');
  const context = await poolGovernanceContext(pool);
  const value = {
    merkleRoot,
    newEpoch: nonNegativeBigInt(newEpoch, 'new epoch'),
    newCumulativeSpent: nonNegativeBigInt(newCumulativeSpent, 'new cumulative spent'),
    previousEpoch: nonNegativeBigInt(
      previousEpoch ?? await pool.epoch(),
      'previous epoch'
    ),
    previousCumulativeSpent: nonNegativeBigInt(
      previousCumulativeSpent ?? await pool.cumulativeSpent(),
      'previous cumulative spent'
    ),
    nonce: nonNegativeBigInt(nonce ?? context.nonce, 'governance nonce'),
  };
  return signer.signTypedData(context.domain, ROOT_PROPOSAL_TYPES, value);
}

export async function signMaxEpochDeltaProposal({ signer, pool, newMaxEpochDelta, nonce } = {}) {
  if (!signer?.signTypedData) throw new Error('Missing governance signer');
  const context = await poolGovernanceContext(pool);
  const value = {
    newMaxEpochDelta: nonNegativeBigInt(newMaxEpochDelta, 'new max epoch delta'),
    nonce: nonNegativeBigInt(nonce ?? context.nonce, 'governance nonce'),
  };
  return signer.signTypedData(context.domain, MAX_EPOCH_DELTA_PROPOSAL_TYPES, value);
}

export async function signRescueProposal({ signer, pool, to, amount, nonce } = {}) {
  if (!signer?.signTypedData) throw new Error('Missing governance signer');
  const context = await poolGovernanceContext(pool);
  const value = {
    to: ethers.getAddress(to),
    amount: nonNegativeBigInt(amount, 'rescue amount'),
    nonce: nonNegativeBigInt(nonce ?? context.nonce, 'governance nonce'),
  };
  return signer.signTypedData(context.domain, RESCUE_PROPOSAL_TYPES, value);
}

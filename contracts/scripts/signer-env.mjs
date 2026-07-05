import { ethers } from 'ethers';

export const TAP_ROLLER_SIGNER_ENV = 'MAYHEM_TAP_ROLLER_PRIVATE_KEY';
export const TAP_DEPLOYER_SIGNER_ENV = 'MAYHEM_TAP_DEPLOYER_PRIVATE_KEY';

function normalizePrivateKey(raw, label) {
  const key = String(raw ?? '').trim();
  if (!/^0x[0-9a-fA-F]{64}$/.test(key)) {
    throw new Error(`${label} must be a 0x-prefixed 32-byte private key`);
  }
  return key;
}

export function privateKeyFromEnv(env, names, label = 'private key') {
  for (const name of names) {
    const raw = env?.[name];
    if (raw !== undefined && String(raw).trim() !== '') {
      return { envName: name, privateKey: normalizePrivateKey(raw, name) };
    }
  }
  throw new Error(
    `Missing ${label}. Set ${names.join(' or ')} in the environment; never pass signing keys on argv or store them in the repo.`
  );
}

export function walletFromEnv(provider, {
  env = process.env,
  names,
  label = 'Ethereum signer private key',
} = {}) {
  const resolved = privateKeyFromEnv(env, names, label);
  return {
    envName: resolved.envName,
    wallet: new ethers.Wallet(resolved.privateKey, provider),
  };
}

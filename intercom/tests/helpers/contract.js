import b4a from 'b4a';
import { blake3 } from '@tracsystems/blake3';
import PeerWallet from 'trac-wallet';
import {
  consentMessage,
  depositTnkIntentMessage,
  probeResultMessage,
  providerKybMessage,
  providerLifecycleIntentMessage,
} from '../../contract/contract.js';

export const ZERO_HEX = '0'.repeat(64);

export class MemoryStorage {
  constructor(initial = {}) {
    this.values = new Map(Object.entries(initial));
  }

  static fromSnapshotBytes(bytes) {
    return new MemoryStorage(Object.fromEntries(JSON.parse(bytes)));
  }

  async get(key) {
    return this.values.has(key) ? { value: this.values.get(key) } : null;
  }

  async put(key, value) {
    this.values.set(key, value);
  }

  async del(key) {
    this.values.delete(key);
  }

  snapshotBytes() {
    return JSON.stringify(Array.from(this.values.entries()).sort(([a], [b]) => a.localeCompare(b)));
  }
}

export const makeTxKey = (n) => n.toString(16).padStart(64, '0');

export const makeOperation = (type, value, sender, txNo, writer = ZERO_HEX) => ({
  type: 'tx',
  key: makeTxKey(txNo),
  value: {
    dispatch: { type, value },
    ipk: sender,
    wp: writer,
  },
});

export const execute = (contract, storage, type, value, sender, txNo, writer = ZERO_HEX) =>
  contract.execute(makeOperation(type, value, sender, txNo, writer), storage);

export const makeFeatureOperation = (featureType, key, value, sender) => ({
  type: 'feature',
  key: `${featureType.replace(/_feature$/, '')}_${key}`,
  value: {
    dispatch: {
      type: featureType,
      key,
      value,
      address: sender,
    },
  },
});

export const executeFeature = (contract, storage, featureType, key, value, sender) =>
  contract.execute(makeFeatureOperation(featureType, key, value, sender), storage);

export async function epochApplyFeatureKey(contract, value) {
  const key = await contract.epochApplyFeatureKey(value);
  if (key instanceof Error) throw key;
  return key;
}

export async function executeEpochApplyFeature(contract, storage, value, sender) {
  contract._mayhemLastFeatureResult = undefined;
  const result = await executeFeature(
    contract,
    storage,
    'mayhem_feature',
    await epochApplyFeatureKey(contract, value),
    value,
    sender
  );
  return result ?? contract._mayhemLastFeatureResult;
}

export async function depositFeatureKey(contract, value) {
  const key = await contract.depositFeatureKey(value);
  if (key instanceof Error) throw key;
  return key;
}

export async function executeDepositFeature(contract, storage, value, sender) {
  contract._mayhemLastFeatureResult = undefined;
  const result = await executeFeature(
    contract,
    storage,
    'mayhem_feature',
    await depositFeatureKey(contract, value),
    value,
    sender
  );
  return result ?? contract._mayhemLastFeatureResult;
}

export async function rateFeatureKey(contract, value) {
  const key = await contract.rateFeatureKey(value);
  if (key instanceof Error) throw key;
  return key;
}

export async function executeRateFeature(contract, storage, value, sender) {
  contract._mayhemLastFeatureResult = undefined;
  const result = await executeFeature(
    contract,
    storage,
    'mayhem_feature',
    await rateFeatureKey(contract, value),
    value,
    sender
  );
  return result ?? contract._mayhemLastFeatureResult;
}

export async function seedCurrentAdminPrice(
  storage,
  {
    enclaveId,
    modelId,
    admin,
    txNo = 1,
    ver = 1,
    inPer1kMu = 10,
    outPer1kMu = 30,
    perReqMu = 0,
    minSessionMu = 0,
    effectiveAt = 0,
  }
) {
  const record = {
    enclave_id: enclaveId,
    model_id: modelId,
    denom: 'mu_usd',
    ver,
    in_per_1k_mu: inPer1kMu,
    out_per_1k_mu: outPer1kMu,
    per_req_mu: perReqMu,
    min_session_mu: minSessionMu,
    effective_at: effectiveAt,
    effective_from: makeTxKey(txNo),
    updated_at: makeTxKey(txNo),
    set_by: admin,
    set_by_role: 'admin',
  };
  await storage.put(`price/${enclaveId}`, {
    enclave_id: enclaveId,
    model_id: modelId,
    denom: 'mu_usd',
    current: record,
    pending: null,
  });
  await storage.put(`price/${enclaveId}/v/${ver}`, record);
  return record;
}

export async function makeIdentity() {
  const wallet = new PeerWallet();
  await wallet.ready;
  await wallet.generateKeyPair();
  return {
    wallet,
    publicKey: b4a.toString(wallet.publicKey, 'hex'),
  };
}

export const makeVerifier = (wallet) => ({
  verify(signature, message, publicKey) {
    return wallet.verify(
      b4a.from(signature, 'hex'),
      b4a.from(String(message)),
      b4a.from(publicKey, 'hex')
    );
  },
});

export const signConsent = (wallet, ver, hash, signingVersion) =>
  b4a.toString(wallet.sign(b4a.from(consentMessage(ver, hash, signingVersion))), 'hex');

export const signDepositTnkIntent = (wallet, intent) =>
  b4a.toString(wallet.sign(b4a.from(depositTnkIntentMessage(intent))), 'hex');

export const signProviderLifecycleIntent = (wallet, intent, signingVersion) =>
  b4a.toString(wallet.sign(b4a.from(providerLifecycleIntentMessage(intent, signingVersion))), 'hex');

export const signProbeResult = (wallet, value, auditor) =>
  b4a.toString(wallet.sign(b4a.from(probeResultMessage(value, auditor))), 'hex');

export const signProviderKyb = (wallet, value) =>
  b4a.toString(wallet.sign(b4a.from(providerKybMessage(value))), 'hex');

export const providerLifecycleFeatureKey = async (intent, signingVersion) => {
  const digest = await blake3(b4a.from(providerLifecycleIntentMessage(intent, signingVersion)));
  return `intent/provider/${intent.provider}/${intent.op}/${b4a.toString(digest, 'hex')}`;
};

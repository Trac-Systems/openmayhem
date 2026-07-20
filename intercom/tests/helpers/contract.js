import b4a from 'b4a';
import { blake3 } from '@tracsystems/blake3';
import { keccak256 } from 'ethereum-cryptography/keccak';
import { secp256k1 } from 'ethereum-cryptography/secp256k1';
import PeerWallet from 'trac-wallet';
import {
  consentMessage,
  depositTnkIntentMessage,
  probeResultMessage,
  providerKybMessage,
  providerLifecycleIntentMessage,
  spendReservationMessage,
  spendVoucherMessage,
  tapAccountBindingMessage,
  targetedSpendReservationMessage,
} from '../../contract/contract.js';

export const ZERO_HEX = '0'.repeat(64);
export const textRateMap = (inPer1kAu = 20, outPer1kAu = 60) => [
  { unit: 'input_token', per_unit_au: String(inPer1kAu), granularity: 1000 },
  { unit: 'output_token', per_unit_au: String(outPer1kAu), granularity: 1000 },
];
const auString = (value) => String(value);

const ctxBracketForTokens = (tokens) => {
  if (tokens <= 8_192) return 'le8k';
  if (tokens <= 32_768) return 'le32k';
  if (tokens <= 131_072) return 'le128k';
  if (tokens <= 262_144) return 'le256k';
  return 'gt256k';
};

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

export async function seedSpendHold(storage, { user, rail = 'fiat', epoch, au }) {
  await storage.put(`hold/${rail}/${user}/${epoch}`, {
    user,
    rail,
    denom: 'au_usd',
    epoch,
    reserved_au: auString(au),
    balance_au_at_last_reserve: null,
    sessions: [],
    updated_at: makeTxKey(99),
  });
}

export async function seedSpendHoldsForApply(storage, value) {
  const totals = new Map();
  for (const debit of value.debits ?? []) {
    const key = `${debit.rail}:${debit.user}:${value.epoch}`;
    const current = totals.get(key) ?? 0n;
    totals.set(key, current + BigInt(String(debit.au)));
  }
  for (const [key, au] of totals) {
    const [rail, user, epoch] = key.split(':');
    await seedSpendHold(storage, { user, rail, epoch: Number(epoch), au });
  }
}

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

export const executeFeature = async (contract, storage, featureType, key, value, sender) => {
  contract._mayhemLastFeatureResult = undefined;
  await contract.execute(makeFeatureOperation(featureType, key, value, sender), storage);
  return contract._mayhemLastFeatureResult;
};

export async function epochApplyFeatureKey(contract, value) {
  const key = await contract.epochApplyFeatureKey(value);
  if (key instanceof Error) throw key;
  return key;
}

export async function executeEpochApplyFeature(contract, storage, value, sender) {
  const previousStorage = contract.storage;
  const previousAddress = contract.address;
  const previousValue = contract.value;
  const previousTx = contract.tx;
  contract.storage = storage;
  contract.address = sender;
  contract.value = value;
  contract.tx = await epochApplyFeatureKey(contract, value);
  try {
    // Legacy-shaped vectors exercise the accounting core used by apply_targeted_epoch.
    return await contract.targetedEpochApply(value, [], []);
  } finally {
    contract.storage = previousStorage;
    contract.address = previousAddress;
    contract.value = previousValue;
    contract.tx = previousTx;
  }
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

export async function executeTapAccountBindingFeature(contract, storage, value, sender) {
  contract._mayhemLastFeatureResult = undefined;
  const key = await contract.tapAccountBindingFeatureKey(value);
  if (key instanceof Error) throw key;
  const result = await executeFeature(contract, storage, 'mayhem_feature', key, value, sender);
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

export async function tnkSettlementFeatureKey(contract, value) {
  const key = await contract.tnkSettlementFeatureKey(value);
  if (key instanceof Error) throw key;
  return key;
}

export async function executeTnkSettlementFeature(contract, storage, value, sender) {
  contract._mayhemLastFeatureResult = undefined;
  const result = await executeFeature(
    contract,
    storage,
    'mayhem_feature',
    await tnkSettlementFeatureKey(contract, value),
    value,
    sender
  );
  return result ?? contract._mayhemLastFeatureResult;
}

export async function fiatSettlementFeatureKey(contract, value) {
  const key = await contract.fiatSettlementFeatureKey(value);
  if (key instanceof Error) throw key;
  return key;
}

export async function executeFiatSettlementFeature(contract, storage, value, sender) {
  contract._mayhemLastFeatureResult = undefined;
  const result = await executeFeature(
    contract,
    storage,
    'mayhem_feature',
    await fiatSettlementFeatureKey(contract, value),
    value,
    sender
  );
  return result ?? contract._mayhemLastFeatureResult;
}

export async function reputationAnchorFeatureKey(contract, value) {
  const key = await contract.reputationAnchorFeatureKey(value);
  if (key instanceof Error) throw key;
  return key;
}

export async function executeReputationAnchorFeature(contract, storage, value, sender) {
  contract._mayhemLastFeatureResult = undefined;
  const result = await executeFeature(
    contract,
    storage,
    'mayhem_feature',
    await reputationAnchorFeatureKey(contract, value),
    value,
    sender
  );
  return result ?? contract._mayhemLastFeatureResult;
}

export async function spendReservationFeatureKey(contract, value, storage = null) {
  const previousStorage = contract.storage;
  if (storage) contract.storage = storage;
  try {
    const key = value.op === 'spend_reserve_targeted'
      ? await contract.targetedSpendReservationFeatureKey(value)
      : await contract.spendReservationFeatureKey(value);
    if (key instanceof Error) throw key;
    return key;
  } finally {
    if (storage) contract.storage = previousStorage;
  }
}

export async function executeSpendReservationFeature(contract, storage, value, sender) {
  contract._mayhemLastFeatureResult = undefined;
  const result = await executeFeature(
    contract,
    storage,
    'mayhem_feature',
    await spendReservationFeatureKey(contract, value, storage),
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
    inPer1kAu = 10,
    outPer1kAu = 30,
    perReqAu = 0,
    minSessionAu = 0,
    effectiveAt = 0,
    ctxBracket,
    ctxBracketTableVer = 1,
    rateMap,
  }
) {
  let resolvedCtxBracket = ctxBracket;
  if (resolvedCtxBracket === undefined) {
    const enclave = (await storage.get(`enclave/${enclaveId}`))?.value;
    if (!enclave || enclave.model_class === 'text-generation') {
      const cap = Number(enclave?.caps?.ctx_max ?? enclave?.caps?.ctx ?? 8_192);
      resolvedCtxBracket = ctxBracketForTokens(Number.isFinite(cap) && cap > 0 ? cap : 8_192);
    } else {
      resolvedCtxBracket = null;
    }
  }
  const record = {
    enclave_id: enclaveId,
    model_id: modelId,
    denom: 'au_usd',
    ver,
    rate_map: rateMap ?? textRateMap(inPer1kAu, outPer1kAu),
    per_req_au: auString(perReqAu),
    min_session_au: auString(minSessionAu),
    effective_at: effectiveAt,
    effective_from: makeTxKey(txNo),
    updated_at: makeTxKey(txNo),
    set_by: admin,
    set_by_role: 'admin',
  };
  if (resolvedCtxBracket) {
    record.ctx_bracket = resolvedCtxBracket;
    record.ctx_bracket_table_ver = ctxBracketTableVer;
  }
  const scheduleKey = resolvedCtxBracket
    ? `price/${enclaveId}/${resolvedCtxBracket}`
    : `price/${enclaveId}`;
  await storage.put(scheduleKey, {
    enclave_id: enclaveId,
    model_id: modelId,
    denom: 'au_usd',
    ...(resolvedCtxBracket
      ? { ctx_bracket: resolvedCtxBracket, ctx_bracket_table_ver: ctxBracketTableVer }
      : {}),
    current: record,
    pending: null,
  });
  await storage.put(`${scheduleKey}/v/${ver}`, record);
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

export function makeEthereumIdentity() {
  const privateKey = secp256k1.utils.randomPrivateKey();
  const publicKey = secp256k1.getPublicKey(privateKey, false);
  return {
    privateKey,
    address: `0x${b4a.toString(keccak256(publicKey.subarray(1)).subarray(12), 'hex')}`,
  };
}

export const makeVerifier = (wallet) => ({
  verify(signature, message, publicKey) {
    return wallet.verify(
      b4a.from(signature, 'hex'),
      b4a.isBuffer(message) ? message : b4a.from(String(message)),
      b4a.from(publicKey, 'hex')
    );
  },
});

export const signConsent = (wallet, ver, hash, signingVersion) =>
  b4a.toString(wallet.sign(b4a.from(consentMessage(ver, hash, signingVersion))), 'hex');

export const signDepositTnkIntent = (wallet, intent) =>
  b4a.toString(wallet.sign(b4a.from(depositTnkIntentMessage(intent))), 'hex');

export const signTapAccountBinding = (wallet, ethereum, value) => {
  const message = tapAccountBindingMessage(value);
  const body = b4a.from(message, 'utf8');
  const prefix = b4a.from(`\x19Ethereum Signed Message:\n${body.length}`, 'utf8');
  const signature = secp256k1.sign(keccak256(b4a.concat([prefix, body])), ethereum.privateKey, {
    lowS: true,
  });
  const ethereumSignature = b4a.alloc(65);
  ethereumSignature.set(signature.toCompactRawBytes(), 0);
  ethereumSignature[64] = 27 + signature.recovery;
  return {
    ...value,
    user_sig: b4a.toString(wallet.sign(b4a.from(message)), 'hex'),
    ethereum_sig: `0x${b4a.toString(ethereumSignature, 'hex')}`,
  };
};

export const signSpendVoucher = (wallet, body, signingVersion) =>
  b4a.toString(wallet.sign(b4a.from(spendVoucherMessage(body, signingVersion))), 'hex');

export const signSpendReservation = (wallet, value) =>
  b4a.toString(wallet.sign(b4a.from(spendReservationMessage(value))), 'hex');

export const signTargetedSpendReservation = (wallet, value) =>
  b4a.toString(wallet.sign(b4a.from(targetedSpendReservationMessage(value))), 'hex');

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

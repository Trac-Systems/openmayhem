import b4a from "b4a";
import { fastestToJsonSchema } from "./utils/schemaToJson.js";
import { createHash } from "../src/utils/types.js";

const asHex32 = (value, field) => {
  const hex = String(value ?? "").trim().toLowerCase();
  if (!/^[0-9a-f]{64}$/.test(hex)) throw new Error(`Invalid ${field}. Expected 32-byte hex (64 chars).`);
  return hex;
};

const isObject = (v) => v !== null && typeof v === "object" && !Array.isArray(v);

const requireApi = (peer) => {
  const api = peer?.protocol.instance?.api;
  if (!api) throw new Error("Protocol API not initialized.");
  return api;
};

export async function getStatus(peer) {
  const subnetBootstrapHex = b4a.isBuffer(peer.bootstrap)
    ? b4a.toString(peer.bootstrap, "hex")
    : peer.bootstrap != null
      ? String(peer.bootstrap)
      : null;

  const peerMsbAddress = peer.msbClient.pubKeyHexToAddress(peer.wallet.publicKey);

  const admin = peer.base?.view ? await peer.base.view.get("admin") : null;
  const chatStatus = peer.base?.view ? await peer.base.view.get("chat_status") : null;

  return {
    peer: {
      pubKeyHex: peer.wallet?.publicKey ?? null,
      writerKeyHex: peer.writerLocalKey ?? null,
      msbAddress: peerMsbAddress,
      baseWritable: !!peer.base?.writable,
      isIndexer: !!peer.base?.isIndexer,
      isWriter: !!peer.base?.writable,
      subnetBootstrapHex,
      subnetChannelUtf8: peer.channel ? b4a.toString(peer.channel, "utf8") : null,
      subnetSignedLength: peer.base?.view?.core?.signedLength ?? null,
      subnetUnsignedLength: peer.base?.view?.core?.length ?? null,
      admin: admin?.value ?? null,
      chatStatus: chatStatus?.value ?? null,
    },
    msb: {
      ready: true,
      bootstrapHex: peer.msbClient.bootstrapHex,
      networkId: peer.msbClient.networkId,
      signedLength: peer.msbClient.getSignedLength()
    },
  };
}

const inferPrototypeOps = (contract) => {
  const proto = Object.getPrototypeOf(contract);
  const baseProto = Object.getPrototypeOf(proto);
  const baseNames = new Set(Object.getOwnPropertyNames(baseProto ?? {}));
  const names = Object.getOwnPropertyNames(proto);
  const ops = [];
  for (const name of names) {
    if (name === "constructor") continue;
    if (name.startsWith("_")) continue;
    if (baseNames.has(name)) continue;
    const desc = Object.getOwnPropertyDescriptor(proto, name);
    if (!desc || typeof desc.value !== "function") continue;
    ops.push(name);
  }
  return ops;
};

const convertContractOpSchema = (fv) => {
  if (!isObject(fv)) return { value: {} };
  const out = {};
  if (fv.key !== undefined) out.key = fastestToJsonSchema(fv.key);
  if (fv.value !== undefined) out.value = fastestToJsonSchema(fv.value);
  if (Object.keys(out).length === 0) out.value = fastestToJsonSchema(fv);
  return out;
};

export async function getContractSchema(peer) {
  const contract = peer?.contract?.instance;
  if (!contract) throw new Error("Contract instance not initialized.");

  const registrations = contract.metadata ?? {};
  const regSchemas = registrations.schemas ?? {};
  const regFunctions = registrations.functions ?? {};
  const regFeatures = registrations.features ?? {};

  const schemaNames = Object.keys(regSchemas);
  const functionNames = Object.keys(regFunctions);
  const featureNames = Object.keys(regFeatures);

  const hasAnyExplicit = schemaNames.length > 0 || functionNames.length > 0 || featureNames.length > 0;

  const inferred = hasAnyExplicit ? [] : inferPrototypeOps(contract);
  const txTypes = [...new Set([...schemaNames, ...functionNames, ...featureNames, ...inferred])].sort();

  const ops = {};
  for (const type of txTypes) {
    if (regSchemas[type] !== undefined) ops[type] = convertContractOpSchema(regSchemas[type]);
    else ops[type] = { value: {} };
  }

  return {
    schemaVersion: 1,
    schemaFormat: "json-schema",
    contract: {
      contractClass: contract.constructor?.name ?? null,
      protocolClass: peer.protocol.instance?.constructor?.name ?? null,
      txTypes,
      ops,
    },
    api: peer.protocol.instance?.getApiSchema ? peer.protocol.instance.getApiSchema() : { methods: {} },
  };
}

export async function contractGenerateNonce(peer) {
  const api = requireApi(peer);
  return api.generateNonce();
}

export async function contractPrepareTx(peer, { prepared_command, address, nonce } = {}) {
  const api = requireApi(peer);
  if (!isObject(prepared_command)) throw new Error("prepared_command must be an object.");
  const addr = asHex32(address, "address");
  const n = asHex32(nonce, "nonce");

  if (peer?.protocol.instance?.safeJsonStringify == null) {
    throw new Error("safeJsonStringify is not available on protocol instance.");
  }

  const json = peer.protocol.instance.safeJsonStringify(prepared_command);
  if (json == null) throw new Error("Failed to stringify prepared_command.");

  const command_hash = await createHash(json);
  const tx = await api.generateTx(addr, command_hash, n);
  return { tx, command_hash };
}

export async function contractTx(peer, { tx, prepared_command, address, signature, nonce, sim = false } = {}) {
  const api = requireApi(peer);
  if (!isObject(prepared_command)) throw new Error("prepared_command must be an object.");
  const res = await api.tx(tx, prepared_command, address, signature, nonce, sim === true);
  return { result: res };
}

export async function contractFeature(peer, { feature = "mayhem", key, value } = {}) {
  if (!peer.base?.writable) throw new Error("Peer subnet is not writable.");
  const featureName = String(feature ?? "").trim();
  const recordKey = String(key ?? "").trim();
  if (!featureName) throw new Error("Missing feature.");
  if (!recordKey) throw new Error("Missing key.");
  if (recordKey.length > 256) throw new Error("Invalid key. Expected at most 256 characters.");
  if (!isObject(value)) throw new Error("Invalid value. Expected an object.");
  const registered = peer.protocol?.instance?.features?.[featureName];
  if (!registered || typeof registered.append !== "function") {
    throw new Error(`Feature ${featureName} is not available.`);
  }
  await registered.append(recordKey, value);
  return { ok: true, feature: featureName, key: recordKey };
}

const stateView = (peer, confirmed) => {
  if (!peer.base?.view) throw new Error("Peer view not ready.");
  if (!confirmed) return { view: peer.base.view, close: async () => {} };
  const view = peer.base.view.checkout(peer.base.view.core.signedLength);
  return { view, close: async () => view.close() };
};

export async function getState(peer, key, { confirmed = true } = {}) {
  const k = String(key ?? "");
  if (!k) throw new Error("Missing key.");
  const session = stateView(peer, confirmed);
  try {
    const res = await session.view.get(k);
    return res?.value ?? null;
  } finally {
    await session.close();
  }
}

export async function getStatePrefix(peer, prefix, { confirmed = true, limit = 500 } = {}) {
  const p = String(prefix ?? "");
  if (!p) throw new Error("Missing prefix.");
  if (p.length > 256) throw new Error("Prefix is too long.");
  const n = Number(limit);
  if (!Number.isInteger(n) || n < 1 || n > 1000) {
    throw new Error("Invalid limit. Expected an integer from 1 to 1000.");
  }

  const session = stateView(peer, confirmed);
  const entries = [];
  try {
    const stream = session.view.createReadStream({
      gte: p,
      lt: `${p}\xff`,
      limit: n,
    });
    for await (const entry of stream) {
      entries.push({ key: entry.key, value: entry.value });
    }
  } finally {
    await session.close();
  }
  return entries;
}

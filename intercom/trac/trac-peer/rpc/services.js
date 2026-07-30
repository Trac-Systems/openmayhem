import b4a from "b4a";
import { fastestToJsonSchema } from "./utils/schemaToJson.js";

const isObject = (v) => v !== null && typeof v === "object" && !Array.isArray(v);

const requireApi = (peer) => {
  const api = peer?.protocol.instance?.api;
  if (!api) throw new Error("Protocol API not initialized.");
  return api;
};

export async function getStatus(peer) {
  const subnetBootstrapHex = b4a.isBuffer(peer.config.bootstrap)
    ? b4a.toString(peer.config.bootstrap, "hex")
    : peer.config.bootstrap != null
      ? String(peer.config.bootstrap)
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

export async function contractTxContext(peer) {
  const api = requireApi(peer);

  const networkId = peer.msbClient.networkId;
  const mbs = peer.msbClient.bootstrapHex;
  const txv = await peer.msbClient.getTxvHex();

  const bs =
    b4a.isBuffer(peer.config.bootstrap)
      ? b4a.toString(peer.config.bootstrap, "hex")
      : peer.config.bootstrap != null
        ? String(peer.config.bootstrap)
        : null;

  const iw = peer.writerLocalKey ?? api.getPeerWriterKey?.();
  if (!iw || !/^[0-9a-f]{64}$/i.test(String(iw))) throw new Error("Peer writer key is not available.");
  if (!bs || !/^[0-9a-f]{64}$/i.test(String(bs))) throw new Error("Peer subnet bootstrap is not available.");

  return {
    msb: {
      networkId,
      txv,
      iw: String(iw).toLowerCase(),
      bs: String(bs).toLowerCase(),
      mbs: String(mbs).toLowerCase(),
      operationType: 12,
    },
  };
}

export async function contractTx(peer, { tx, prepared_command, address, signature, nonce, sim = false } = {}) {
  const api = requireApi(peer);
  if (!isObject(prepared_command)) throw new Error("prepared_command must be an object.");
  const res = await api.tx(tx, prepared_command, address, signature, nonce, sim === true);
  return { result: res };
}

export async function getState(peer, key, { confirmed = true } = {}) {
  const k = String(key ?? "");
  if (!k) throw new Error("Missing key.");
  if (!peer.base?.view) throw new Error("Peer view not ready.");
  if (confirmed) {
    const viewSession = peer.base.view.checkout(peer.base.view.core.signedLength);
    try {
      const res = await viewSession.get(k);
      return res?.value ?? null;
    } finally {
      await viewSession.close();
    }
  }
  const res = await peer.base.view.get(k);
  return res?.value ?? null;
}

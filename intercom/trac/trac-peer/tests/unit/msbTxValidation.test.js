import test from "brittle";
import path from "path";
import os from "os";
import fs from "fs/promises";
import b4a from "b4a";
import PeerWallet from "trac-wallet";

import { Peer, Contract, Protocol, createConfig, ENV } from "../../src/index.js";
import Wallet from "../../src/wallet.js";
import { createHash, jsonStringify } from "../../src/utils/types.js";
import { mkdtempPortable, rmrfPortable } from "../helpers/tmpdir.js";

class JsonProtocol extends Protocol {
  mapTxCommand(command) {
    if (typeof command !== "string" || command.trim() === "") return null;
    const raw = command.trim();
    if (!raw.startsWith("{")) return null;
    try {
      const parsed = JSON.parse(raw);
      if (parsed && typeof parsed.type === "string" && parsed.value !== undefined) {
        return { type: parsed.type, value: parsed.value };
      }
    } catch (_e) {}
    return null;
  }
}

class OkContract extends Contract {
  constructor(protocol, config) {
    super(protocol, config);
    this.addFunction("catch");
  }

  async catch() {
    return "ok";
  }
}

async function prepareWallet(storesDirectory, storeName) {
  const wallet = new Wallet();
  await wallet.generateKeyPair();
  const keypairPath = path.join(storesDirectory, storeName, "db", "keypair.json");
  await fs.mkdir(path.dirname(keypairPath), { recursive: true });
  await wallet.exportToFile(keypairPath, b4a.alloc(0));
  return wallet;
}

async function closePeer(peer) {
  if (!peer) return;
  try {
    await peer.close();
  } catch (_e) {}
  try {
    await peer.store.close();
  } catch (_e) {}
}

function makeMsbStub({
  getRegisteredBootstrapEntry = async (_bootstrapHex) => null,
  getNodeEntryUnsigned = async (_address) => null,
  getNodeEntry = async (_address) => null,
  getIndexerSequenceState = async () => b4a.alloc(32).fill(1),
  get = async (_key) => null,
} = {}) {
  const bootstrap = b4a.alloc(32).fill(7);
  const dummyAddress = PeerWallet.encodeBech32mSafe("trac", b4a.alloc(32).fill(2));
  const addressLength = dummyAddress.length;
  const fee = b4a.alloc(16);
  fee[15] = 1;

  return {
    config: { bootstrap, addressPrefix: "trac", addressLength, networkId: 918 },
    wallet: { address: dummyAddress },
    network: {},
    state: {
      base: {
        view: {
          core: { signedLength: 0, length: 0, once() {} },
          checkout() {
            return { async get() { return null; }, async close() {} };
          },
        },
      },
      getFee() {
        return fee;
      },
      async getRegisteredBootstrapEntry(bootstrapHex) {
        return await getRegisteredBootstrapEntry(bootstrapHex);
      },
      async getNodeEntryUnsigned(address) {
        return await getNodeEntryUnsigned(address);
      },
      async getNodeEntry(address) {
        return await getNodeEntry(address);
      },
      async getIndexerSequenceState() {
        return await getIndexerSequenceState();
      },
      async get(key) {
        return await get(key);
      },
    },
    async ready() {},
  };
}

test("msb preflight: sim fails when subnet bootstrap not deployed", async (t) => {
  const tmpRoot = await mkdtempPortable(path.join(os.tmpdir(), "trac-peer-msb-preflight-"));
  const storesDirectory = tmpRoot.endsWith(path.sep) ? tmpRoot : tmpRoot + path.sep;

  const msb = makeMsbStub({
    getRegisteredBootstrapEntry: async () => null,
    getNodeEntryUnsigned: async () => ({ balance: b4a.alloc(16).fill(0xff) }),
    getNodeEntry: async () => ({ balance: b4a.alloc(16).fill(0xff) }),
  });

  const wallet = await prepareWallet(storesDirectory, "peer");
  const config = createConfig(ENV.DEVELOPMENT, {
    storesDirectory,
    storeName: "peer",
    bootstrap: b4a.alloc(32).fill(9),
  });
  const peer = new Peer({ config, msb, wallet, protocol: JsonProtocol, contract: OkContract });

  try {
    await peer.ready();
    t.exception(
      () => peer.protocol.instance.broadcastTransaction({ type: "catch", value: {} }, true),
      /Invalid MSB tx:.*not registered as deployment entry/i
    );
  } finally {
    await closePeer(peer);
    await rmrfPortable(tmpRoot);
  }
});

test("msb preflight: sim fails when requester has insufficient fee balance", async (t) => {
  const tmpRoot = await mkdtempPortable(path.join(os.tmpdir(), "trac-peer-msb-preflight-"));
  const storesDirectory = tmpRoot.endsWith(path.sep) ? tmpRoot : tmpRoot + path.sep;

  const msb = makeMsbStub({
    getRegisteredBootstrapEntry: async () => {
      // minimal non-null value; subsequent state.get(txHash) will fail with a clearer message if used,
      // but we won't reach it because balance check fails first.
      return b4a.alloc(32 + PeerWallet.encodeBech32mSafe("trac", b4a.alloc(32).fill(2)).length);
    },
    getNodeEntryUnsigned: async () => ({ balance: b4a.alloc(16) }),
    getNodeEntry: async () => ({ balance: b4a.alloc(16) }),
  });

  const wallet = await prepareWallet(storesDirectory, "peer");
  const config = createConfig(ENV.DEVELOPMENT, {
    storesDirectory,
    storeName: "peer",
    bootstrap: b4a.alloc(32).fill(9),
  });
  const peer = new Peer({ config, msb, wallet, protocol: JsonProtocol, contract: OkContract });

  try {
    await peer.ready();
    t.exception(
      () => peer.protocol.instance.broadcastTransaction({ type: "catch", value: {} }, true),
      /Invalid MSB tx:.*Insufficient balance to cover transaction fee/i
    );
  } finally {
    await closePeer(peer);
    await rmrfPortable(tmpRoot);
  }
});

test("msb preflight: sim fails when txv changes between generation and validation (expired)", async (t) => {
  const tmpRoot = await mkdtempPortable(path.join(os.tmpdir(), "trac-peer-msb-preflight-"));
  const storesDirectory = tmpRoot.endsWith(path.sep) ? tmpRoot : tmpRoot + path.sep;

  const txv1 = b4a.alloc(32).fill(1);
  const txv2 = b4a.alloc(32).fill(2);
  let calls = 0;

  const msb = makeMsbStub({
    getIndexerSequenceState: async () => (calls++ === 0 ? txv1 : txv2),
    getRegisteredBootstrapEntry: async () => null, // we should fail earlier on txv mismatch
    getNodeEntryUnsigned: async () => ({ balance: b4a.alloc(16).fill(0xff) }),
    getNodeEntry: async () => ({ balance: b4a.alloc(16).fill(0xff) }),
  });

  const wallet = await prepareWallet(storesDirectory, "peer");
  const config = createConfig(ENV.DEVELOPMENT, {
    storesDirectory,
    storeName: "peer",
    bootstrap: b4a.alloc(32).fill(9),
  });
  const peer = new Peer({ config, msb, wallet, protocol: JsonProtocol, contract: OkContract });

  try {
    await peer.ready();
    t.exception(
      () => peer.protocol.instance.broadcastTransaction({ type: "catch", value: {} }, true),
      /Invalid MSB tx:.*Transaction has expired/i
    );
  } finally {
    await closePeer(peer);
    await rmrfPortable(tmpRoot);
  }
});

test("msb preflight: sim fails when tx already exists (duplicate)", async (t) => {
  const tmpRoot = await mkdtempPortable(path.join(os.tmpdir(), "trac-peer-msb-preflight-"));
  const storesDirectory = tmpRoot.endsWith(path.sep) ? tmpRoot : tmpRoot + path.sep;

  const msb = makeMsbStub({
    get: async () => b4a.alloc(1), // any tx hash appears as already present
    getRegisteredBootstrapEntry: async () => null, // uniqueness check runs before bootstrap checks
    getNodeEntryUnsigned: async () => ({ balance: b4a.alloc(16).fill(0xff) }),
    getNodeEntry: async () => ({ balance: b4a.alloc(16).fill(0xff) }),
  });

  const wallet = await prepareWallet(storesDirectory, "peer");
  const config = createConfig(ENV.DEVELOPMENT, {
    storesDirectory,
    storeName: "peer",
    bootstrap: b4a.alloc(32).fill(9),
  });
  const peer = new Peer({ config, msb, wallet, protocol: JsonProtocol, contract: OkContract });

  try {
    await peer.ready();
    t.exception(
      () => peer.protocol.instance.broadcastTransaction({ type: "catch", value: {} }, true),
      /Invalid MSB tx:.*already exists in the state/i
    );
  } finally {
    await closePeer(peer);
    await rmrfPortable(tmpRoot);
  }
});

test("msb preflight: sim fails on MSB-level signature validation (direct protocol call)", async (t) => {
  const tmpRoot = await mkdtempPortable(path.join(os.tmpdir(), "trac-peer-msb-preflight-"));
  const storesDirectory = tmpRoot.endsWith(path.sep) ? tmpRoot : tmpRoot + path.sep;

  const funded = { balance: b4a.alloc(16).fill(0xff) };
  const msb = makeMsbStub({ getNodeEntryUnsigned: async () => funded, getNodeEntry: async () => funded });

  const peerWallet = await prepareWallet(storesDirectory, "peer");
  const externalWallet = new Wallet();
  await externalWallet.generateKeyPair();

  const config = createConfig(ENV.DEVELOPMENT, {
    storesDirectory,
    storeName: "peer",
    bootstrap: b4a.alloc(32).fill(9),
  });
  const peer = new Peer({ config, msb, wallet: peerWallet, protocol: JsonProtocol, contract: OkContract });

  try {
    await peer.ready();
    const obj = { type: "catch", value: {} };
    const command_hash = await createHash(jsonStringify(obj));
    const nonce = PeerWallet.generateNonce().toString("hex");
    const txvHex = await peer.msbClient.getTxvHex();
    const msbBootstrapHex = peer.msbClient.bootstrapHex;
    const subnetBootstrapHex = b4a.isBuffer(peer.config.bootstrap)
      ? peer.config.bootstrap.toString("hex")
      : String(peer.config.bootstrap);

    const txHex = await peer.protocol.instance.generateTx(
      peer.msbClient.networkId,
      txvHex,
      peer.writerLocalKey,
      command_hash,
      subnetBootstrapHex,
      msbBootstrapHex,
      nonce
    );
    const badSig = "01".repeat(64);
    const surrogate = { tx: txHex, nonce, signature: badSig, address: externalWallet.publicKey };

    t.exception(
      () => peer.protocol.instance.broadcastTransaction({ type: "catch", value: {} }, true, surrogate),
      /Invalid MSB tx:.*Invalid signature in payload/i
    );
  } finally {
    await closePeer(peer);
    await rmrfPortable(tmpRoot);
  }
});

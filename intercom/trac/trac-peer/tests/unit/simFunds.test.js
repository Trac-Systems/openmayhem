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

class CatchContract extends Contract {
  constructor(protocol, config) {
    super(protocol, config);
    this.addFunction("catch");
  }

  async catch() {
    return "ok";
  }
}

function makeMsbStubUnfunded() {
  const bootstrap = b4a.alloc(32).fill(7);
  const txv = b4a.alloc(32).fill(1);
  const fee = b4a.alloc(16);
  fee[15] = 1;
  const addressLength = PeerWallet.encodeBech32mSafe("trac", b4a.alloc(32).fill(2)).length;
  return {
    config: { bootstrap, addressPrefix: "trac", addressLength, networkId: 918 },
    wallet: { address: "trac1test" },
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
      getSignedLength() {
        return 0;
      },
      getFee() {
        return fee;
      },
      async get(_key) {
        return null;
      },
      async getNodeEntryUnsigned(_address) {
        return null;
      },
      async getIndexerSequenceState() {
        return txv;
      },
    },
    async ready() {},
  };
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

test("rpc sim: fails when requester has no MSB entry/balance", async (t) => {
  const tmpRoot = await mkdtempPortable(path.join(os.tmpdir(), "trac-peer-sim-funds-"));
  const storesDirectory = tmpRoot.endsWith(path.sep) ? tmpRoot : tmpRoot + path.sep;

  const peerWallet = await prepareWallet(storesDirectory, "peer");
  const externalWallet = new Wallet();
  await externalWallet.generateKeyPair();

  const config = createConfig(ENV.DEVELOPMENT, {
    storesDirectory,
    storeName: "peer",
    apiTxExposed: true,
  });

  const peer = new Peer({
    config,
    msb: makeMsbStubUnfunded(),
    wallet: peerWallet,
    protocol: Protocol,
    contract: CatchContract,
  });

  try {
    await peer.ready();
    const api = peer.protocol.instance.api;

    const prepared_command = { type: "catch", value: {} };
    const nonce = api.generateNonce();
    const command_hash = await createHash(jsonStringify(prepared_command));
    const tx = await api.generateTx(externalWallet.publicKey, command_hash, nonce);
    const signature = externalWallet.sign(b4a.from(tx, "hex"));

    await t.exception(
      () => api.tx(tx, prepared_command, externalWallet.publicKey, signature, nonce, true),
      /Invalid MSB tx:.*Requester address not found in state/i
    );
  } finally {
    await closePeer(peer);
    await rmrfPortable(tmpRoot);
  }
});

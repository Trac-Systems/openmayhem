import test from "brittle";
import path from "path";
import os from "os";
import fs from "fs/promises";
import b4a from "b4a";

import { Peer, Contract, Protocol, createConfig, ENV } from "../../src/index.js";
import Wallet from "../../src/wallet.js";
import { mkdtempPortable, rmrfPortable } from "../helpers/tmpdir.js";

class JsonProtocol extends Protocol {
  mapTxCommand(command) {
    if (typeof command !== "string" || command.trim() === "") return null;
    const raw = command.trim();
    if (raw.startsWith("{")) {
      try {
        const parsed = JSON.parse(raw);
        if (parsed && typeof parsed.type === "string" && parsed.value !== undefined) {
          return { type: parsed.type, value: parsed.value };
        }
      } catch (_e) {}
    }
    return null;
  }
}

class CatchContract extends Contract {
  constructor(protocol, config) {
    super(protocol, config);
    this.addFunction("catch");
  }

  async catch() {
    await this.put(`app/pokedex/${this.address}`, { ok: true, tx: this.tx });
    return "ok";
  }
}

function makeMsbStub() {
  const bootstrap = b4a.alloc(32).fill(7);
  const txv = b4a.alloc(32).fill(1);
  return {
    config: { bootstrap, addressPrefix: "trac", networkId: 918 },
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

test("cli tx: /tx --command JSON can simulate catch", async (t) => {
  const tmpRoot = await mkdtempPortable(path.join(os.tmpdir(), "trac-peer-cli-tx-"));
  const storesDirectory = tmpRoot.endsWith(path.sep) ? tmpRoot : tmpRoot + path.sep;

  const msb = makeMsbStub();
  const wallet = await prepareWallet(storesDirectory, "peer");

  const config = createConfig(ENV.DEVELOPMENT, {
    storesDirectory,
    storeName: "peer",
    bootstrap: b4a.alloc(32).fill(9),
  });
  const peer = new Peer({
    config,
    msb,
    wallet,
    protocol: JsonProtocol,
    contract: CatchContract,
  });

  try {
    await peer.ready();
    const res = await peer.protocol.instance.tx({ command: '{"type":"catch","value":{}}' }, true);
    t.is(res, "ok");
  } finally {
    await closePeer(peer);
    await rmrfPortable(tmpRoot);
  }
});

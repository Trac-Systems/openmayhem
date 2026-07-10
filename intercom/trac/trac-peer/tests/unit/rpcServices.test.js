import test from "brittle";
import b4a from "b4a";

import { getStatePrefix, getStatus } from "../../rpc/services.js";

function makeStream(entries) {
  return {
    async *[Symbol.asyncIterator]() {
      for (const entry of entries) yield entry;
    },
  };
}

function makeView(entries) {
  return {
    core: { signedLength: 7 },
    createReadStream({ gte, lt, limit }) {
      return makeStream(
        entries
          .filter((entry) => entry.key >= gte && entry.key < lt)
          .slice(0, limit)
      );
    },
    checkout() {
      return {
        ...this,
        async close() {},
      };
    },
  };
}

test("rpc services: getStatePrefix returns bounded prefix records", async (t) => {
  const peer = {
    base: {
      view: makeView([
        { key: "enclave/a", value: { status: "active" } },
        { key: "enclave/b", value: { status: "retired" } },
        { key: "room/a", value: { status: "open" } },
      ]),
    },
  };

  const records = await getStatePrefix(peer, "enclave/", {
    confirmed: true,
    limit: 1,
  });

  t.alike(records, [
    { key: "enclave/a", value: { status: "active" } },
  ]);
});

test("rpc services: status exposes the MSB boot-proof fields", async (t) => {
  const peer = {
    config: {
      bootstrap: b4a.from("aa".repeat(32), "hex"),
      channelName: "mayhem-router-subnet",
      dhtBootstrap: ["node1.hyperdht.org:49737"],
    },
    wallet: { publicKey: "11".repeat(32) },
    writerLocalKey: "22".repeat(32),
    base: {
      writable: false,
      isIndexer: true,
      view: {
        core: { signedLength: 7, length: 8 },
        async get() { return null; },
      },
    },
    msbClient: {
      bootstrapHex: "bb".repeat(32),
      channelUtf8: "0000trac0network0msb0mainnet0000",
      networkId: 918,
      getSignedLength: () => 54_349,
      getConnectedValidatorsCount: () => 3,
      pubKeyHexToAddress: () => "trac1status",
      dhtBootstrap: ["node2.hyperdht.org:49737"],
    },
  };

  const status = await getStatus(peer);
  t.alike(status.msb, {
    ready: true,
    bootstrapHex: "bb".repeat(32),
    channel: "0000trac0network0msb0mainnet0000",
    networkId: 918,
    signedLength: 54_349,
    connectedValidators: 3,
    dhtBootstrap: ["node2.hyperdht.org:49737"],
  });
  t.is(status.peer.subnetBootstrapHex, "aa".repeat(32));
  t.is(status.peer.subnetChannelUtf8, "mayhem-router-subnet");
  t.alike(status.peer.dhtBootstrap, ["node1.hyperdht.org:49737"]);
});

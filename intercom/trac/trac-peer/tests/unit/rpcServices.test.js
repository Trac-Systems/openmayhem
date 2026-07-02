import test from "brittle";

import { getStatePrefix } from "../../rpc/services.js";

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

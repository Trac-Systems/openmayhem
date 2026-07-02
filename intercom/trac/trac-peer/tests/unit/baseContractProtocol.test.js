import test from "brittle";

import Contract from "../../src/artifacts/contract.js";
import Protocol from "../../src/artifacts/protocol.js";

const makeProtocolStubForContract = () => {
  const compile = (_schema) => () => true;
  return { peer: { check: { validator: { compile } } } };
};

test("base Contract: addFunction/addSchema/addFeature populate metadata", async (t) => {
  const protocol = makeProtocolStubForContract();
  const contract = new Contract(protocol, {});

  contract.addFunction("f1");
  t.is(contract.funcs.f1, true);
  t.is(contract.metadata.functions.f1.type, "f1");

  const schema = { value: { $$type: "object", a: { type: "string", min: 1 } } };
  contract.addSchema("s1", schema);
  t.is(contract.check.hasSchema("s1"), true);
  t.is(typeof contract.check.schemata.s1, "function");
  t.is(contract.metadata.schemas.s1.value.a.min, 1);

  function myFeature() {}
  contract.addFeature("feat1", myFeature);
  t.is(typeof contract.features.feat1, "function");
  t.is(contract.metadata.features.feat1.type, "feat1");
  t.is(contract.metadata.features.feat1.name, "myFeature");
});

test("base Contract: addSchema stores a clone (mutating input doesn't affect metadata)", async (t) => {
  const protocol = makeProtocolStubForContract();
  const contract = new Contract(protocol, {});

  const schema = { value: { $$type: "object", a: { type: "string", min: 1 } } };
  contract.addSchema("op", schema);

  schema.value = { $$type: "object", a: { type: "string", min: 999 } };
  t.is(contract.metadata.schemas.op.value.a.min, 1);
});

test("base Protocol: getApiSchema exposes tx + extendApi methods", async (t) => {
  const peer = {};
  const protocol = new Protocol(peer, null, {});

  const baseSchema = protocol.getApiSchema();
  t.is(typeof baseSchema?.methods?.tx, "object");
  t.is(baseSchema.methods.tx.params.length, 6);

  class ExtendedProtocol extends Protocol {
    async extendApi() {
      const _this = this;
      this.api.getListingsLength = async function (signed = true) {
        void signed;
        void _this;
        return 0;
      };
    }
  }

  const extended = new ExtendedProtocol(peer, null, {});
  await extended.extendApi();
  const extendedSchema = extended.getApiSchema();
  t.is(typeof extendedSchema?.methods?.getListingsLength, "object");
  t.ok(Array.isArray(extendedSchema.methods.getListingsLength.params));
});

test("base Protocol: tx throws if mapTxCommand returns null", async (t) => {
  const peer = {};
  const protocol = new Protocol(peer, null, {});
  await t.exception(() => protocol.tx({ command: "unknown command" }, true), /command not found/i);
});

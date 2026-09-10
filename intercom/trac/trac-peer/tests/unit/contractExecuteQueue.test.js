import test from "brittle";

import Contract from "../../src/artifacts/contract.js";

const makeProtocolStubForContract = () => {
  const compile = (_schema) => () => true;
  return { peer: { check: { validator: { compile } } } };
};

class MemoryStorage {
  constructor(initial = {}) {
    this.values = new Map(Object.entries(initial));
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

  keys() {
    return [...this.values.keys()].sort();
  }

  snapshotBytes() {
    return JSON.stringify(
      [...this.values.entries()].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    );
  }
}

const txKey = (n) => n.toString(16).padStart(64, "0");
const address = (n) => n.toString(16).repeat(64).slice(0, 64);

const operation = (type, value, sender, txNo, writer = "0".repeat(64)) => ({
  type: "tx",
  key: txKey(txNo),
  value: {
    dispatch: { type, value },
    ipk: sender,
    wp: writer,
  },
});

const deferred = () => {
  let resolve = null;
  const promise = new Promise((settle) => { resolve = settle; });
  return { promise, resolve };
};

// Give a pending execution enough turns of the event loop to reach its await.
const settle = async () => {
  for (let index = 0; index < 8; index += 1) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
};

const probeContract = (release, ContractClass = Contract) => {
  const contract = new ContractClass(makeProtocolStubForContract(), {});
  contract.addFunction("probe");
  contract.probe = async function probe() {
    const entered = {
      tag: this.value.tag,
      tx: this.tx,
      address: this.address,
      validator_address: this.validator_address,
      type: this.op.type,
      is_feature: this.is_feature,
      is_message: this.is_message,
    };
    if (entered.tag === "held") await release.promise;
    const resumed = {
      tag: this.value.tag,
      tx: this.tx,
      address: this.address,
      validator_address: this.validator_address,
      type: this.op.type,
      is_feature: this.is_feature,
      is_message: this.is_message,
    };
    await this.put(`probe/${resumed.tag}`, resumed);
    const stored = await this.get(`probe/${resumed.tag}`);
    return { ok: true, entered, resumed, stored };
  };
  return contract;
};

test("contract execute: overlapping executions each keep their own operation, sender and storage", async (t) => {
  const release = deferred();
  const contract = probeContract(release);
  const heldStorage = new MemoryStorage();
  const otherStorage = new MemoryStorage();

  const heldRun = contract.execute(
    operation("probe", { tag: "held" }, address(1), 701, address(9)),
    heldStorage
  );
  await settle();
  const otherRun = contract.execute(
    operation("probe", { tag: "other" }, address(2), 702, address(8)),
    otherStorage
  );
  await settle();
  release.resolve();
  const [heldResult, otherResult] = await Promise.all([heldRun, otherRun]);

  const heldExpected = {
    tag: "held",
    tx: txKey(701),
    address: address(1),
    validator_address: address(9),
    type: "probe",
    is_feature: false,
    is_message: false,
  };
  const otherExpected = {
    tag: "other",
    tx: txKey(702),
    address: address(2),
    validator_address: address(8),
    type: "probe",
    is_feature: false,
    is_message: false,
  };

  t.alike(heldResult.entered, heldExpected);
  t.alike(heldResult.resumed, heldExpected);
  t.alike(heldResult.stored, heldExpected);
  t.alike(otherResult.entered, otherExpected);
  t.alike(otherResult.resumed, otherExpected);
  t.alike(otherResult.stored, otherExpected);
  t.alike(heldStorage.keys(), ["probe/held"]);
  t.alike(otherStorage.keys(), ["probe/other"]);

  // The same two calls, run one after the other, produce the same results and
  // the same storage bytes.
  const sequentialRelease = deferred();
  sequentialRelease.resolve();
  const sequentialContract = probeContract(sequentialRelease);
  const sequentialHeldStorage = new MemoryStorage();
  const sequentialOtherStorage = new MemoryStorage();
  const sequentialHeld = await sequentialContract.execute(
    operation("probe", { tag: "held" }, address(1), 701, address(9)),
    sequentialHeldStorage
  );
  const sequentialOther = await sequentialContract.execute(
    operation("probe", { tag: "other" }, address(2), 702, address(8)),
    sequentialOtherStorage
  );

  t.alike(heldResult, sequentialHeld);
  t.alike(otherResult, sequentialOther);
  t.is(heldStorage.snapshotBytes(), sequentialHeldStorage.snapshotBytes());
  t.is(otherStorage.snapshotBytes(), sequentialOtherStorage.snapshotBytes());
  t.is(contract.storage, null);
  t.is(contract.address, null);
  t.is(contract.execute_queue, null);
});

test("contract execute: a throwing handler leaves the queue usable", async (t) => {
  const contract = new Contract(makeProtocolStubForContract(), {});
  contract.addFunction("boom");
  contract.boom = async function boom() {
    await new Promise((resolve) => setTimeout(resolve, 0));
    throw new Error("handler failed");
  };
  contract.addFunction("after");
  contract.after = async function after() {
    await this.put("after/ran", { tx: this.tx, address: this.address });
    return { ok: true, tx: this.tx, address: this.address };
  };

  const storage = new MemoryStorage();
  await t.exception(
    () => contract.execute(operation("boom", { tag: "boom" }, address(3), 801), storage),
    /Error in contract/
  );
  t.is(contract.execute_queue, null);

  const result = await contract.execute(
    operation("after", { tag: "after" }, address(4), 802),
    storage
  );
  t.alike(result, { ok: true, tx: txKey(802), address: address(4) });
  t.alike(storage.keys(), ["after/ran"]);

  // Two more overlapping calls still take turns after the failure.
  const release = deferred();
  const slowStorage = new MemoryStorage();
  contract.addFunction("slow");
  contract.slow = async function slow() {
    const seen = { tx: this.tx, address: this.address };
    if (this.value.tag === "first") await release.promise;
    return { ok: true, seen, still: { tx: this.tx, address: this.address } };
  };
  const first = contract.execute(
    operation("slow", { tag: "first" }, address(5), 803),
    slowStorage
  );
  await settle();
  const second = contract.execute(
    operation("slow", { tag: "second" }, address(6), 804),
    slowStorage
  );
  await settle();
  release.resolve();
  const [firstResult, secondResult] = await Promise.all([first, second]);
  t.alike(firstResult.seen, firstResult.still);
  t.alike(secondResult.seen, secondResult.still);
  t.is(firstResult.seen.address, address(5));
  t.is(secondResult.seen.address, address(6));
});

test("contract execute: a sub-class override calling super.execute() still runs", async (t) => {
  const calls = [];

  class SubContract extends Contract {
    async execute(op, storage) {
      calls.push(op.value.dispatch.value.tag);
      this.last_type = op.type;
      try {
        return await super.execute(op, storage);
      } finally {
        this.last_type = null;
      }
    }
  }

  const release = deferred();
  const contract = probeContract(release, SubContract);
  const heldStorage = new MemoryStorage();
  const otherStorage = new MemoryStorage();

  const heldRun = contract.execute(
    operation("probe", { tag: "held" }, address(1), 901, address(9)),
    heldStorage
  );
  await settle();
  const otherRun = contract.execute(
    operation("probe", { tag: "other" }, address(2), 902, address(8)),
    otherStorage
  );
  await settle();
  release.resolve();
  const [heldResult, otherResult] = await Promise.all([heldRun, otherRun]);

  t.alike(calls, ["held", "other"]);
  t.is(heldResult.ok, true);
  t.is(otherResult.ok, true);
  t.alike(heldResult.entered, heldResult.resumed);
  t.alike(otherResult.entered, otherResult.resumed);
  t.is(heldResult.resumed.address, address(1));
  t.is(otherResult.resumed.address, address(2));
  t.is(heldResult.resumed.tx, txKey(901));
  t.is(otherResult.resumed.tx, txKey(902));
  t.alike(heldStorage.keys(), ["probe/held"]);
  t.alike(otherStorage.keys(), ["probe/other"]);
  t.is(contract.last_type, null);
  t.is(contract.execute_queue, null);
});

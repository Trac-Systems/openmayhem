import test from "brittle";
import b4a from "b4a";

import Wallet from "../../src/wallet.js";

import { AddAdminOperation, AddAdminCheck } from "../../src/operations/addAdmin/index.js";
import { UpdateAdminOperation, UpdateAdminCheck } from "../../src/operations/updateAdmin/index.js";
import { AddWriterOperation, AddWriterCheck } from "../../src/operations/addWriter/index.js";
import { AddIndexerOperation, AddIndexerCheck } from "../../src/operations/addIndexer/index.js";
import { RemoveWriterOperation, RemoveWriterCheck } from "../../src/operations/removeWriter/index.js";
import { SetAutoAddWritersOperation, SetAutoAddWritersCheck } from "../../src/operations/setAutoAddWriters/index.js";
import { AutoAddWritersOperation, AutoAddWritersCheck } from "../../src/operations/autoAddWriter/index.js";
import { SetChatStatusOperation, SetChatStatusCheck } from "../../src/operations/setChatStatus/index.js";
import { EnableTransactionsOperation, EnableTransactionsCheck } from "../../src/operations/enableTransactions/index.js";
import { EnableWhitelistOperation, EnableWhitelistCheck } from "../../src/operations/enableWhitelist/index.js";
import { SetWhitelistStatusOperation, SetWhitelistStatusCheck } from "../../src/operations/setWhitelistStatus/index.js";
import { SetModOperation, SetModCheck } from "../../src/operations/setMod/index.js";
import { MuteStatusOperation, MuteStatusCheck } from "../../src/operations/muteStatus/index.js";
import { SetNickOperation, SetNickCheck } from "../../src/operations/setNick/index.js";
import { MsgOperation, MsgCheck } from "../../src/operations/msg/index.js";
import { DeleteMessageOperation, DeleteMessageCheck } from "../../src/operations/deleteMessage/index.js";
import { PinMessageOperation, PinMessageCheck } from "../../src/operations/pinMessage/index.js";
import { UnpinMessageOperation, UnpinMessageCheck } from "../../src/operations/unpinMessage/index.js";
import { FeatureOperation, FeatureCheck } from "../../src/operations/feature/index.js";

const makeHex32 = (fillByte) => b4a.toString(b4a.alloc(32).fill(fillByte), "hex");

function makeBatch(initial = {}) {
  const map = new Map(Object.entries(initial));
  return {
    async get(key) {
      if (!map.has(key)) return null;
      return { value: map.get(key) };
    },
    async put(key, value) {
      map.set(key, value);
    },
    async del(key) {
      map.delete(key);
    },
    _map: map,
  };
}

function makeBase() {
  const calls = { addWriter: [], removeWriter: [] };
  return {
    async addWriter(keyBuf, opts) {
      calls.addWriter.push({ key: b4a.toString(keyBuf, "hex"), opts });
    },
    async removeWriter(keyBuf) {
      calls.removeWriter.push(b4a.toString(keyBuf, "hex"));
    },
    view: { core: { signedLength: 0 } },
    _calls: calls,
  };
}

function makeNode(op, { fromKeyHex = null } = {}) {
  return {
    value: op,
    from: { key: fromKeyHex ? b4a.from(fromKeyHex, "hex") : b4a.alloc(32).fill(0) },
  };
}

function makeProtocolStub({ msgMaxBytes = 1_000_000, txMaxBytes = 1_000_000, featMaxBytes = 1_000_000 } = {}) {
  return {
    msgMaxBytes: () => msgMaxBytes,
    txMaxBytes: () => txMaxBytes,
    featMaxBytes: () => featMaxBytes,
    getError: () => null,
  };
}

test("operations: addAdmin sets initial admin on bootstrap node", async (t) => {
  const subnetBootstrapHex = makeHex32(1);
  const adminPk = makeHex32(2);

  const batch = makeBatch();
  const base = makeBase();
  const node = makeNode({ type: "addAdmin", key: adminPk }, { fromKeyHex: subnetBootstrapHex });
  const config = { bootstrap: b4a.from(subnetBootstrapHex, "hex") };

  const op = new AddAdminOperation(new AddAdminCheck(), { config });
  await op.handle(node.value, batch, base, node);

  t.is((await batch.get("admin"))?.value ?? null, adminPk);
});

test("operations: updateAdmin transfers admin (admin-signed)", async (t) => {
  const adminWallet = new Wallet();
  await adminWallet.generateKeyPair();
  const newAdminPk = makeHex32(7);

  const batch = makeBatch({ admin: adminWallet.publicKey });
  const base = makeBase();
  const nonce = makeHex32(9);
  const value = { dispatch: { admin: newAdminPk, type: "updateAdmin", address: adminWallet.publicKey } };
  const strValue = JSON.stringify(value);
  const hash = adminWallet.sign(`${strValue}${nonce}`);
  const opObj = { type: "updateAdmin", value, hash, nonce };
  const node = makeNode(opObj);

  const op = new UpdateAdminOperation(new UpdateAdminCheck(), {
    wallet: adminWallet,
    protocolInstance: makeProtocolStub(),
    contractInstance: { execute: async () => null },
  });
  await op.handle(opObj, batch, base, node);

  t.is((await batch.get("admin"))?.value ?? null, newAdminPk);
  t.is((await batch.get(`sh/${hash}`))?.value ?? null, "");
});

test("operations: addWriter adds an autobase writer (admin-signed)", async (t) => {
  const adminWallet = new Wallet();
  await adminWallet.generateKeyPair();
  const writerKeyHex = makeHex32(3);
  const nonce = makeHex32(4);
  const msg = { type: "addWriter", key: writerKeyHex };
  const hash = adminWallet.sign(`${JSON.stringify(msg)}${nonce}`);

  const batch = makeBatch({ admin: adminWallet.publicKey });
  const base = makeBase();
  const opObj = { type: "addWriter", key: writerKeyHex, value: { msg }, hash, nonce };
  const node = makeNode(opObj);

  const op = new AddWriterOperation(new AddWriterCheck(), {
    wallet: adminWallet,
    protocolInstance: makeProtocolStub(),
    contractInstance: { execute: async () => null },
  });
  await op.handle(opObj, batch, base, node);

  t.is(base._calls.addWriter.length, 1);
  t.is(base._calls.addWriter[0].key, writerKeyHex);
  t.is(base._calls.addWriter[0].opts?.isIndexer ?? null, false);
  t.is((await batch.get(`sh/${hash}`))?.value ?? null, "");
});

test("operations: addIndexer adds an autobase indexer (admin-signed)", async (t) => {
  const adminWallet = new Wallet();
  await adminWallet.generateKeyPair();
  const writerKeyHex = makeHex32(5);
  const nonce = makeHex32(6);
  const msg = { type: "addIndexer", key: writerKeyHex };
  const hash = adminWallet.sign(`${JSON.stringify(msg)}${nonce}`);

  const batch = makeBatch({ admin: adminWallet.publicKey });
  const base = makeBase();
  const opObj = { type: "addIndexer", key: writerKeyHex, value: { msg }, hash, nonce };
  const node = makeNode(opObj);

  const op = new AddIndexerOperation(new AddIndexerCheck(), {
    wallet: adminWallet,
    protocolInstance: makeProtocolStub(),
    contractInstance: { execute: async () => null },
  });
  await op.handle(opObj, batch, base, node);

  t.is(base._calls.addWriter.length, 1);
  t.is(base._calls.addWriter[0].key, writerKeyHex);
  t.is(base._calls.addWriter[0].opts?.isIndexer ?? null, true);
});

test("operations: removeWriter removes an autobase writer/indexer (admin-signed)", async (t) => {
  const adminWallet = new Wallet();
  await adminWallet.generateKeyPair();
  const writerKeyHex = makeHex32(8);
  const nonce = makeHex32(10);
  const msg = { type: "removeWriter", key: writerKeyHex };
  const hash = adminWallet.sign(`${JSON.stringify(msg)}${nonce}`);

  const batch = makeBatch({ admin: adminWallet.publicKey });
  const base = makeBase();
  const opObj = { type: "removeWriter", key: writerKeyHex, value: { msg }, hash, nonce };
  const node = makeNode(opObj);

  const op = new RemoveWriterOperation(new RemoveWriterCheck(), {
    wallet: adminWallet,
    protocolInstance: makeProtocolStub(),
    contractInstance: { execute: async () => null },
  });
  await op.handle(opObj, batch, base, node);

  t.is(base._calls.removeWriter.length, 1);
  t.is(base._calls.removeWriter[0], writerKeyHex);
});

test("operations: setAutoAddWriters toggles auto_add_writers (admin-signed)", async (t) => {
  const adminWallet = new Wallet();
  await adminWallet.generateKeyPair();
  const nonce = makeHex32(11);
  const msg = { type: "setAutoAddWriters", key: "on" };
  const hash = adminWallet.sign(`${JSON.stringify(msg)}${nonce}`);

  const batch = makeBatch({ admin: adminWallet.publicKey });
  const base = makeBase();
  const opObj = { type: "setAutoAddWriters", key: "on", value: { msg }, hash, nonce };
  const node = makeNode(opObj);

  const op = new SetAutoAddWritersOperation(new SetAutoAddWritersCheck(), {
    wallet: adminWallet,
    protocolInstance: makeProtocolStub(),
    contractInstance: { execute: async () => null },
  });
  await op.handle(opObj, batch, base, node);

  t.is((await batch.get("auto_add_writers"))?.value ?? null, "on");
});

test("operations: autoAddWriter adds writer when enabled and not banned", async (t) => {
  const writerKeyHex = makeHex32(12);

  const batch = makeBatch({ auto_add_writers: "on" });
  const base = makeBase();
  const opObj = { type: "autoAddWriter", key: writerKeyHex };
  const node = makeNode(opObj);

  const op = new AutoAddWritersOperation(new AutoAddWritersCheck(), {
    wallet: new Wallet(),
    protocolInstance: makeProtocolStub(),
    contractInstance: { execute: async () => null },
  });
  await op.handle(opObj, batch, base, node);

  t.is(base._calls.addWriter.length, 1);
  t.is(base._calls.addWriter[0].key, writerKeyHex);
});

test("operations: setChatStatus toggles chat_status (admin-signed)", async (t) => {
  const adminWallet = new Wallet();
  await adminWallet.generateKeyPair();
  const nonce = makeHex32(13);
  const msg = { type: "setChatStatus", key: "on" };
  const hash = adminWallet.sign(`${JSON.stringify(msg)}${nonce}`);

  const batch = makeBatch({ admin: adminWallet.publicKey });
  const base = makeBase();
  const opObj = { type: "setChatStatus", key: "on", value: { msg }, hash, nonce };
  const node = makeNode(opObj);

  const op = new SetChatStatusOperation(new SetChatStatusCheck(), {
    wallet: adminWallet,
    protocolInstance: makeProtocolStub(),
    contractInstance: { execute: async () => null },
  });
  await op.handle(opObj, batch, base, node);

  t.is((await batch.get("chat_status"))?.value ?? null, "on");
});

test("operations: enableTransactions sets txen (admin-signed)", async (t) => {
  const adminWallet = new Wallet();
  await adminWallet.generateKeyPair();
  const nonce = makeHex32(14);
  const value = { dispatch: { enabled: true, type: "enableTransactions", address: adminWallet.publicKey } };
  const strValue = JSON.stringify(value);
  const hash = adminWallet.sign(`${strValue}${nonce}`);

  const batch = makeBatch({ admin: adminWallet.publicKey });
  const base = makeBase();
  const opObj = { type: "enableTransactions", value, hash, nonce };
  const node = makeNode(opObj);

  const op = new EnableTransactionsOperation(new EnableTransactionsCheck(), { wallet: adminWallet });
  await op.handle(opObj, batch, base, node);

  t.is((await batch.get("txen"))?.value ?? null, true);
});

test("operations: enableWhitelist sets wlst (admin-signed)", async (t) => {
  const adminWallet = new Wallet();
  await adminWallet.generateKeyPair();
  const nonce = makeHex32(15);
  const value = { dispatch: { enabled: true, type: "enableWhitelist", address: adminWallet.publicKey } };
  const strValue = JSON.stringify(value);
  const hash = adminWallet.sign(`${strValue}${nonce}`);

  const batch = makeBatch({ admin: adminWallet.publicKey });
  const base = makeBase();
  const opObj = { type: "enableWhitelist", value, hash, nonce };
  const node = makeNode(opObj);

  const op = new EnableWhitelistOperation(new EnableWhitelistCheck(), { wallet: adminWallet });
  await op.handle(opObj, batch, base, node);

  t.is((await batch.get("wlst"))?.value ?? null, true);
});

test("operations: setWhitelistStatus sets wl/<user> (admin-signed)", async (t) => {
  const adminWallet = new Wallet();
  await adminWallet.generateKeyPair();
  const userPk = makeHex32(16);
  const nonce = makeHex32(17);
  const value = { dispatch: { status: true, type: "setWhitelistStatus", address: adminWallet.publicKey, user: userPk } };
  const strValue = JSON.stringify(value);
  const hash = adminWallet.sign(`${strValue}${nonce}`);

  const batch = makeBatch({ admin: adminWallet.publicKey });
  const base = makeBase();
  const opObj = { type: "setWhitelistStatus", value, hash, nonce };
  const node = makeNode(opObj);

  const op = new SetWhitelistStatusOperation(new SetWhitelistStatusCheck(), { wallet: adminWallet });
  await op.handle(opObj, batch, base, node);

  t.is((await batch.get(`wl/${userPk}`))?.value ?? null, true);
});

test("operations: setMod sets mod/<user> (admin-signed)", async (t) => {
  const adminWallet = new Wallet();
  await adminWallet.generateKeyPair();
  const userPk = makeHex32(18);
  const nonce = makeHex32(19);
  const value = { dispatch: { mod: true, type: "setMod", address: adminWallet.publicKey, user: userPk } };
  const strValue = JSON.stringify(value);
  const hash = adminWallet.sign(`${strValue}${nonce}`);

  const batch = makeBatch({ admin: adminWallet.publicKey });
  const base = makeBase();
  const opObj = { type: "setMod", value, hash, nonce };
  const node = makeNode(opObj);

  const op = new SetModOperation(new SetModCheck(), { wallet: adminWallet });
  await op.handle(opObj, batch, base, node);

  t.is((await batch.get(`mod/${userPk}`))?.value ?? null, true);
});

test("operations: muteStatus sets mtd/<user> (admin-signed)", async (t) => {
  const adminWallet = new Wallet();
  await adminWallet.generateKeyPair();
  const userPk = makeHex32(20);
  const nonce = makeHex32(21);
  const value = { dispatch: { muted: true, type: "muteStatus", address: adminWallet.publicKey, user: userPk } };
  const strValue = JSON.stringify(value);
  const hash = adminWallet.sign(`${strValue}${nonce}`);

  const batch = makeBatch({ admin: adminWallet.publicKey });
  const base = makeBase();
  const opObj = { type: "muteStatus", value, hash, nonce };
  const node = makeNode(opObj);

  const op = new MuteStatusOperation(new MuteStatusCheck(), {
    wallet: adminWallet,
    protocolInstance: makeProtocolStub(),
    contractInstance: { execute: async () => null },
  });
  await op.handle(opObj, batch, base, node);

  t.is((await batch.get(`mtd/${userPk}`))?.value ?? null, true);
});

test("operations: setNick stores nick/<addr> and kcin/<nick> (user-signed)", async (t) => {
  const userWallet = new Wallet();
  await userWallet.generateKeyPair();
  const nonce = makeHex32(22);
  const value = {
    dispatch: { nick: "alice", type: "setNick", address: userWallet.publicKey, initiator: userWallet.publicKey },
  };
  const strValue = JSON.stringify(value);
  const hash = userWallet.sign(`${strValue}${nonce}`);

  const batch = makeBatch({ chat_status: "on", admin: makeHex32(23) });
  const base = makeBase();
  const opObj = { type: "setNick", value, hash, nonce };
  const node = makeNode(opObj);

  const op = new SetNickOperation(new SetNickCheck(), {
    wallet: userWallet,
    protocolInstance: makeProtocolStub(),
    contractInstance: { execute: async () => null },
  });
  await op.handle(opObj, batch, base, node);

  t.is((await batch.get(`nick/${userWallet.publicKey}`))?.value ?? null, "alice");
  t.is((await batch.get(`kcin/alice`))?.value ?? null, userWallet.publicKey);
});

test("operations: msg indexes message when chat is on and signature valid", async (t) => {
  const userWallet = new Wallet();
  await userWallet.generateKeyPair();
  const adminPk = makeHex32(24);
  const nonce = makeHex32(25);
  const value = {
    dispatch: {
      attachments: [],
      msg: "hi",
      type: "msg",
      address: userWallet.publicKey,
      deleted_by: null,
      reply_to: null,
      pinned: false,
      pin_id: null,
    },
  };
  const strValue = JSON.stringify(value);
  const hash = userWallet.sign(`${strValue}${nonce}`);
  const opObj = { type: "msg", value, hash, nonce };
  const node = makeNode(opObj);

  const batch = makeBatch({ admin: adminPk, chat_status: "on" });
  const base = makeBase();

  const op = new MsgOperation(new MsgCheck(), {
    wallet: userWallet,
    protocolInstance: makeProtocolStub({ msgMaxBytes: 8192 }),
    contractInstance: { execute: async () => null },
  });
  await op.handle(opObj, batch, base, node);

  t.is((await batch.get("msgl"))?.value ?? null, 1);
  t.is((await batch.get(`msg/0`))?.value?.msg ?? null, "hi");
  t.is((await batch.get(`umsg/${userWallet.publicKey}/0`))?.value ?? null, "msg/0");
});

test("operations: deleteMessage blanks message and indexes deletion (admin-signed)", async (t) => {
  const adminWallet = new Wallet();
  await adminWallet.generateKeyPair();
  const userPk = makeHex32(26);
  const nonce = makeHex32(27);
  const value = { dispatch: { id: 0, type: "deleteMessage", address: adminWallet.publicKey } };
  const strValue = JSON.stringify(value);
  const hash = adminWallet.sign(`${strValue}${nonce}`);
  const opObj = { type: "deleteMessage", value, hash, nonce };
  const node = makeNode(opObj);

  const batch = makeBatch({
    admin: adminWallet.publicKey,
    "msg/0": {
      attachments: [],
      msg: "hello",
      type: "msg",
      address: userPk,
      deleted_by: null,
      reply_to: null,
      pinned: false,
      pin_id: null,
    },
  });
  const base = makeBase();

  const op = new DeleteMessageOperation(new DeleteMessageCheck(), { wallet: adminWallet });
  await op.handle(opObj, batch, base, node);

  const msg0 = (await batch.get("msg/0"))?.value ?? null;
  t.ok(msg0 && typeof msg0 === "object");
  t.ok("msg" in msg0);
  t.is(msg0.msg, null);
  t.is(msg0?.deleted_by ?? null, adminWallet.publicKey);
  t.is((await batch.get("delml"))?.value ?? null, 1);
  t.is((await batch.get("delm/0"))?.value ?? null, 0);
});

test("operations: pinMessage sets pinned state and pni index (admin-signed)", async (t) => {
  const adminWallet = new Wallet();
  await adminWallet.generateKeyPair();
  const nonce = makeHex32(28);
  const value = { dispatch: { id: 0, pinned: true, type: "pinMessage", address: adminWallet.publicKey } };
  const strValue = JSON.stringify(value);
  const hash = adminWallet.sign(`${strValue}${nonce}`);
  const opObj = { type: "pinMessage", value, hash, nonce };
  const node = makeNode(opObj);

  const batch = makeBatch({
    admin: adminWallet.publicKey,
    "msg/0": {
      attachments: [],
      msg: "hello",
      type: "msg",
      address: makeHex32(29),
      deleted_by: null,
      reply_to: null,
      pinned: false,
      pin_id: null,
    },
  });
  const base = makeBase();

  const op = new PinMessageOperation(new PinMessageCheck(), { wallet: adminWallet });
  await op.handle(opObj, batch, base, node);

  const msg0 = (await batch.get("msg/0"))?.value ?? null;
  t.is(msg0?.pinned ?? null, true);
  t.is(msg0?.pin_id ?? null, 0);
  t.is((await batch.get("pnl"))?.value ?? null, 1);
  t.is((await batch.get("pni/0"))?.value?.msg ?? null, 0);
});

test("operations: unpinMessage clears pin state (admin-signed)", async (t) => {
  const adminWallet = new Wallet();
  await adminWallet.generateKeyPair();
  const nonce = makeHex32(30);
  const value = { dispatch: { id: 0, type: "unpinMessage", address: adminWallet.publicKey } };
  const strValue = JSON.stringify(value);
  const hash = adminWallet.sign(`${strValue}${nonce}`);
  const opObj = { type: "unpinMessage", value, hash, nonce };
  const node = makeNode(opObj);

  const batch = makeBatch({
    admin: adminWallet.publicKey,
    "msg/0": {
      attachments: [],
      msg: "hello",
      type: "msg",
      address: makeHex32(31),
      deleted_by: null,
      reply_to: null,
      pinned: true,
      pin_id: 0,
    },
    "pni/0": { msg: 0, pinned: true },
  });
  const base = makeBase();

  const op = new UnpinMessageOperation(new UnpinMessageCheck(), { wallet: adminWallet });
  await op.handle(opObj, batch, base, node);

  const msg0 = (await batch.get("msg/0"))?.value ?? null;
  t.is(msg0?.pinned ?? null, false);
  t.is(msg0?.pin_id ?? null, null);
  t.is((await batch.get("pni/0"))?.value?.pinned ?? null, false);
});

test("operations: feature executes contract when admin-signed", async (t) => {
  const adminWallet = new Wallet();
  await adminWallet.generateKeyPair();
  const nonce = makeHex32(32);
  const dispatchValue = { a: 1 };
  const strDispatchValue = JSON.stringify(dispatchValue);
  const signature = adminWallet.sign(`${strDispatchValue}${nonce}`);

  const batch = makeBatch({ admin: adminWallet.publicKey });
  const base = makeBase();

  const contractInstance = {
    execute: async (_op, b) => {
      await b.put("feature_called", true);
      return null;
    },
  };

  const opObj = {
    type: "feature",
    key: "my_feature",
    value: { dispatch: { value: dispatchValue, nonce, hash: signature } },
  };
  const node = makeNode(opObj);

  const op = new FeatureOperation(new FeatureCheck(), {
    wallet: adminWallet,
    protocolInstance: makeProtocolStub({ featMaxBytes: 4096 }),
    contractInstance,
  });
  await op.handle(opObj, batch, base, node);

  t.is((await batch.get("feature_called"))?.value ?? null, true);
  t.is((await batch.get(`sh/${signature}`))?.value ?? null, "");
});

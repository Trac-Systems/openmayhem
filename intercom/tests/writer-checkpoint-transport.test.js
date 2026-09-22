import assert from 'node:assert/strict';
import test from 'node:test';
import b4a from 'b4a';
import Wallet from 'trac-peer/src/wallet.js';
import PeerWallet from 'trac-wallet';
import { TxOperation } from 'trac-peer/src/operations/tx/index.js';
import TransactionPool from 'trac-peer/src/transaction/transactionPool.js';
import { safeEncodeApplyOperation } from 'trac-msb/src/utils/protobuf/operationHelpers.js';
import { FEE } from 'trac-msb/src/core/state/utils/transaction.js';
import MayhemContract, { CONTRACT_VERSION, validateMayhemOperationContractVersion } from '../contract/contract.js';
import MayhemProtocol from '../contract/protocol.js';
import { WriterCheckpointTransport } from '../src/writer-checkpoint-transport.js';
import { MemoryStorage, executeFeature } from './helpers/contract.js';

async function harness() {
  const wallet = new Wallet();
  await wallet.ready;
  await wallet.generateKeyPair();
  const validator = new Wallet();
  await validator.ready;
  await validator.generateKeyPair();
  const storage = new MemoryStorage({ admin: wallet.publicKey });
  const confirmations = new Map();
  const toAddress = (key) => PeerWallet.encodeBech32mSafe('trac', b4a.from(key, 'hex'));
  const fromAddress = (address) => b4a.toString(PeerWallet.decodeBech32mSafe(address), 'hex');
  let broadcasts = 0;
  const peer = { wallet, writerLocalKey: '42'.repeat(32),
    config: { bootstrap: '43'.repeat(32), maxMsbSignedLength: 1e9,
      maxMsbSignedLengthFutureDelta: 100000, maxMsbApplyOperationBytes: 4096 },
    txPool: new TransactionPool({ txPoolMaxSize: 64 }),
    base: { writable: true, view: {
      core: { signedLength: 50 },
      get: (key) => storage.get(key),
      checkout: () => {
        const snapshot = MemoryStorage.fromSnapshotBytes(storage.snapshotBytes());
        return { close: async () => {}, get: async (key) => {
          const entry = await snapshot.get(key);
          return entry ? { seq: 1, key, value: entry.value } : null;
        } };
      },
    } },
    msbClient: { networkId: 918, bootstrapHex: '44'.repeat(32),
      getTxvHex: async () => '45'.repeat(32), getFee: () => FEE,
      pubKeyHexToAddress: toAddress, addressToPubKeyHex: fromAddress,
      getSignedLength: () => 100, waitForSignedLengthAtLeast: async () => {},
      getSignedAtLength: async (key) => confirmations.get(key) ?? null,
      validateTransaction: async () => true,
      broadcastTransaction: async (payload) => {
        broadcasts++;
        const txo = Object.fromEntries(Object.entries(payload.txo).map(([key, value]) => [key, b4a.from(value, 'hex')]));
        txo.va = b4a.from(toAddress(validator.publicKey));
        const encoded = safeEncodeApplyOperation({ type: 12, address: b4a.from(payload.address), txo });
        assert.ok(encoded.length > 0);
        confirmations.set(payload.txo.tx, { value: encoded });
        throw new Error('response lost after signed MSB application');
      },
    },
  };
  const protocol = new MayhemProtocol(peer, peer.base, peer.config);
  peer.protocol = { instance: protocol };
  const contract = new MayhemContract(protocol, {});
  peer.contract = { instance: contract };
  const txOperation = new TxOperation({ validateNode: () => true, validate: () => true }, {
    wallet, protocolInstance: protocol, contractInstance: contract,
    msbClient: peer.msbClient, config: peer.config,
  });
  peer.base.append = (op) => txOperation.handle(op, storage, peer.base, {});
  const transport = new WriterCheckpointTransport({ peer,
    feature: { submit: async (key, value) => executeFeature(contract, storage,
      'mayhem_feature', key, value, wallet.publicKey) },
    releaseIdentity: { contractVersion: CONTRACT_VERSION, contractCodeSha256: '46'.repeat(32) },
  });
  return { peer, protocol, contract, storage, transport, confirmations, get broadcasts() { return broadcasts; } };
}

test('exact signed MSB bytes recover through the real subnet TX verifier after a lost ACK', async () => {
  const ctx = await harness();
  const snapshot = await ctx.transport.prepareSnapshot(100, 3000);
  const paid = await ctx.transport.preparePayment(snapshot);
  assert.equal(await ctx.storage.get('checkpoint/current'), null, 'simulation must not write canonical state');
  assert.equal(ctx.contract.storage, null, 'simulation must leave live consensus context untouched');
  const persisted = JSON.parse(JSON.stringify(paid));
  await assert.rejects(ctx.transport.broadcast(persisted), /response lost/);
  const inspected = await ctx.transport.inspect(persisted, '30000000000000000');
  assert.equal(inspected.confirmed, true);
  assert.equal(await ctx.transport.reconcileSubnet(persisted, inspected.proof), null);
  const checkpoint = await ctx.transport.reconcileSubnet(persisted, inspected.proof);
  assert.equal(checkpoint.tx, persisted.surrogate.tx);
  assert.equal(checkpoint.snapshot_hash, snapshot.snapshot_hash);
  assert.equal(ctx.broadcasts, 1);
  assert.equal((await ctx.storage.get(`tx/${checkpoint.tx}`)).value, 0);
  assert.equal((await ctx.storage.get('txi/0')).value.err, null);
});

test('durable preparation rejects changed dispatch, nonce, signature and payload before broadcast', async () => {
  const ctx = await harness();
  const paid = await ctx.transport.preparePayment(await ctx.transport.prepareSnapshot(100, 3000));
  for (const change of [
    (p) => { p.dispatch.value.slot++; },
    (p) => { p.surrogate.nonce = '99'.repeat(32); },
    (p) => { p.surrogate.signature = '00'.repeat(64); },
    (p) => { p.payload.txo.iw = '98'.repeat(32); },
  ]) {
    const corrupted = JSON.parse(JSON.stringify(paid));
    change(corrupted);
    await assert.rejects(ctx.transport.broadcast(corrupted), /Persisted/);
  }
  assert.equal(ctx.broadcasts, 0);
});

test('MSB proof with another dispatch cannot count as a paid checkpoint', async () => {
  const ctx = await harness();
  const paid = await ctx.transport.preparePayment(await ctx.transport.prepareSnapshot(100, 3000));
  await assert.rejects(ctx.transport.broadcast(paid), /response lost/);
  const wrong = JSON.parse(JSON.stringify(paid));
  wrong.dispatch.value.slot = 101;
  // A genuine signed proof for a different transaction does not prove this slot.
  const next = await ctx.protocol.preparePaidTransaction(wrong.dispatch);
  ctx.confirmations.set(next.surrogate.tx, ctx.confirmations.get(paid.surrogate.tx));
  await assert.rejects(ctx.transport.inspect(next, '30000000000000000'), /differs from the exact/);
  assert.equal(await ctx.storage.get('checkpoint/current'), null);
});

async function historicalPreparation(ctx) {
  const snapshot = await ctx.transport.prepareSnapshot(100, 3000);
  // Model the exact persisted v24 snapshot with both canonical digest layers.
  snapshot.state.contract_version = 24;
  snapshot.state_hash = await ctx.contract.opaqueHash('mayhem-checkpoint-state-v1', snapshot.state);
  const { snapshot_hash, ...body } = snapshot;
  snapshot.snapshot_hash = await ctx.contract.opaqueHash('mayhem-checkpoint-snapshot-v1', body);
  await ctx.storage.put('checkpoint/prepared/100', snapshot);
  await ctx.storage.put('checkpoint/preparing', { slot: 100, snapshot_hash: snapshot.snapshot_hash });
  const versioned = ctx.protocol.versionedTransactionObject;
  ctx.protocol.versionedTransactionObject = (value) => ({ ...value,
    value: { ...value.value, contract_version: 24 } });
  const paid = await ctx.protocol.preparePaidTransaction({ type: 'stateCheckpoint', value: {
    op: 'state_checkpoint', slot: 100, snapshot_hash: snapshot.snapshot_hash,
  } });
  ctx.protocol.versionedTransactionObject = versioned;
  return JSON.parse(JSON.stringify(paid));
}

const paidOperation = (paid, proof = { signed_length: 100, validator: 'ab'.repeat(32) }) => ({
  type: 'tx', key: paid.surrogate.tx, value: { dispatch: paid.dispatch,
    ipk: paid.surrogate.address, wp: proof.validator, msbsl: proof.signed_length },
});

test('v24 durable checkpoint survives v26 restart and exact paid replay only once', async () => {
  const ctx = await harness();
  const paid = await historicalPreparation(ctx);
  await assert.rejects(ctx.transport.broadcast(paid), /response lost/);
  const storage = MemoryStorage.fromSnapshotBytes(ctx.storage.snapshotBytes());
  const contract = new MayhemContract(ctx.protocol, {});
  ctx.peer.contract.instance = contract;
  const apply = new TxOperation({ validateNode: () => true, validate: () => true }, {
    wallet: ctx.peer.wallet, protocolInstance: ctx.protocol, contractInstance: contract,
    msbClient: ctx.peer.msbClient, config: ctx.peer.config,
  });
  ctx.peer.base.append = (op) => apply.handle(op, storage, ctx.peer.base, {});
  const transport = new WriterCheckpointTransport({ peer: ctx.peer,
    releaseIdentity: { contractVersion: CONTRACT_VERSION } });
  transport.state = async (key) => (await storage.get(key))?.value ?? null;
  const inspected = await transport.inspect(paid, '30000000000000000');
  await transport.reconcileSubnet(paid, inspected.proof);
  const checkpoint = await transport.reconcileSubnet(paid, inspected.proof);
  await ctx.peer.base.append(paidOperation(paid, inspected.proof));
  assert.equal(checkpoint.tx, paid.surrogate.tx);
  assert.equal((await storage.get('txl')).value, 1);
  assert.equal((await storage.get('txi/0')).value.err, null);
  assert.equal(ctx.broadcasts, 1);
});

test('fresh v24 dispatch cannot fabricate historical preparation', async () => {
  const ctx = await harness();
  const paid = await historicalPreparation(ctx);
  assert.throws(() => validateMayhemOperationContractVersion(paidOperation(paid)), /expected CONTRACT_VERSION 26, got 24/);
  await ctx.storage.del('checkpoint/prepared/100');
  await assert.rejects(ctx.contract.execute(paidOperation(paid), ctx.storage), /expected CONTRACT_VERSION 26, got 24/);
  await assert.rejects(ctx.transport.broadcast(paid), /no matching historical canonical preparation/);
  assert.equal(ctx.broadcasts, 0);
});

test('historical checkpoint recovery rejects tampered evidence and unrelated old operations', async () => {
  for (const mutate of [
    (s) => { s.state.contract_version = 25; },
    (s) => { s.state.catalog_hash = 'aa'.repeat(32); },
    (s) => { s.prepared_by = 'bb'.repeat(32); },
    (s) => { s.slot++; },
  ]) {
    const ctx = await harness();
    const paid = await historicalPreparation(ctx);
    const snapshot = (await ctx.storage.get('checkpoint/prepared/100')).value;
    mutate(snapshot);
    await ctx.storage.put('checkpoint/prepared/100', snapshot);
    await assert.rejects(ctx.contract.execute(paidOperation(paid), ctx.storage), /expected CONTRACT_VERSION 26, got 24/);
    await assert.rejects(ctx.transport.broadcast(paid), /no matching historical canonical preparation/);
  }
  const ctx = await harness();
  const paid = await historicalPreparation(ctx);
  const unrelated = paidOperation(paid);
  unrelated.value.dispatch = { type: 'setRules', value: { op: 'set_rules', ver: 1,
    hash: 'aa'.repeat(32), contract_version: 24 } };
  await assert.rejects(ctx.contract.execute(unrelated, ctx.storage), /expected CONTRACT_VERSION 26, got 24/);
  const mutated = structuredClone(paid);
  mutated.dispatch.value.snapshot_hash = '99'.repeat(32);
  await assert.rejects(ctx.transport.broadcast(mutated), /bytes or signature changed/);
  assert.deepEqual(ctx.protocol.txHashDispatchCandidates(paid.dispatch), [paid.dispatch]);
});

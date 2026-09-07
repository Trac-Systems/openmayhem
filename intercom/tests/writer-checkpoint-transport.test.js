import assert from 'node:assert/strict';
import test from 'node:test';
import b4a from 'b4a';
import Wallet from 'trac-peer/src/wallet.js';
import PeerWallet from 'trac-wallet';
import { TxOperation } from 'trac-peer/src/operations/tx/index.js';
import TransactionPool from 'trac-peer/src/transaction/transactionPool.js';
import { safeEncodeApplyOperation } from 'trac-msb/src/utils/protobuf/operationHelpers.js';
import { FEE } from 'trac-msb/src/core/state/utils/transaction.js';
import MayhemContract, { CONTRACT_VERSION } from '../contract/contract.js';
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

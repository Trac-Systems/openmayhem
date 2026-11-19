import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import test from 'node:test';
import b4a from 'b4a';
import { blake3 } from '@tracsystems/blake3';
import Wallet from 'trac-peer/src/wallet.js';
import PeerWallet from 'trac-wallet';
import { TxOperation } from 'trac-peer/src/operations/tx/index.js';
import TransactionPool from 'trac-peer/src/transaction/transactionPool.js';
import { safeEncodeApplyOperation } from 'trac-msb/src/utils/protobuf/operationHelpers.js';
import { FEE } from 'trac-msb/src/core/state/utils/transaction.js';
import MayhemContract, { CONTRACT_VERSION, matchesCommittedC30Checkpoint, validateMayhemOperationContractVersion } from '../contract/contract.js';
import MayhemProtocol from '../contract/protocol.js';
import { WriterCheckpointTransport } from '../src/writer-checkpoint-transport.js';
import { WriterCheckpointWorker } from '../src/writer-checkpoint-worker.js';
import ContractV31 from '../contract/v31.js';
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
  assert.equal(paid.surrogate.ch, b4a.toString(
    await blake3(b4a.from(JSON.stringify(paid.dispatch), 'utf8')), 'hex'));
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

async function historicalPreparation(ctx, version = 24) {
  const snapshot = await ctx.transport.prepareSnapshot(100, 3000);
  // Model the persisted snapshot with both canonical digest layers.
  snapshot.state.contract_version = version;
  snapshot.state_hash = await ctx.contract.opaqueHash('mayhem-checkpoint-state-v1', snapshot.state);
  const { snapshot_hash, ...body } = snapshot;
  snapshot.snapshot_hash = await ctx.contract.opaqueHash('mayhem-checkpoint-snapshot-v1', body);
  await ctx.storage.put('checkpoint/prepared/100', snapshot);
  await ctx.storage.put('checkpoint/preparing', { slot: 100, snapshot_hash: snapshot.snapshot_hash });
  const versioned = ctx.protocol.versionedTransactionObject;
  ctx.protocol.versionedTransactionObject = (value) => ({ ...value,
    value: { ...value.value, contract_version: version } });
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

async function previousWorker(version = 31) {
  const ctx = await harness();
  if (version === 30) {
    const predecessor = { type: 'paid_state_checkpoint', schema_version: 1, slot: 99,
      tx: 'f9'.repeat(32), paid_by: ctx.peer.wallet.publicKey, observed_at: 2970 };
    await ctx.storage.put('checkpoint/current', predecessor);
    await ctx.storage.put('checkpoint/slot/99', predecessor);
  }
  const prepared = await historicalPreparation(ctx, version);
  const active = { slot: 100, prepared, phase: 'BROADCAST_OR_UNCERTAIN', broadcast_attempts: 1,
    snapshot_hash: prepared.dispatch.value.snapshot_hash, expected_fee_au: '30000000000000000', proof: null };
  const journal = { state: { active, next_slot: 100, due_through: 100 }, completed: [],
    save(state) { this.state = structuredClone(state); },
    complete(entry) { this.completed.push(structuredClone(entry)); } };
  const member = b4a.alloc(32, 9);
  const nextTxv = b4a.toString(await blake3(member), 'hex');
  const reads = [];
  const base = {
    view: { core: { key: b4a.alloc(32, 7), signedLength: 100 }, checkout(length) {
      return { get(key, options) {
        assert.deepEqual(options, { wait: false, update: false });
        reads.push([key, length]);
        return ctx.confirmations.get(key) ?? null;
      }, close: async () => {} };
    } },
    system: { core: { key: b4a.alloc(32, 4), signedLength: 95 }, checkout(length) {
      assert.equal(length, 95);
      return { indexers: [{ key: member }], views: [{ key: base.view.core.key, length: 90 }], close: async () => {} };
    } },
  };
  ctx.transport.msb = { state: { base } };
  let appends = 0;
  const append = ctx.peer.base.append;
  ctx.peer.base.append = (...args) => { appends++; return append(...args); };
  const step = async () => {
    const worker = new WriterCheckpointWorker({ journal, transport: ctx.transport,
      now: () => journal.state.next_slot * 30000, monotonicNow: () => 0, log: () => {} });
    await worker.step();
  };
  return { ...ctx, prepared, active, journal, nextTxv, base, reads, step,
    get broadcasts() { return ctx.broadcasts; }, get appends() { return appends; } };
}

async function fictionalCommittedCheckpoint() {
  const ctx = await previousWorker(30);
  const op = paidOperation(ctx.prepared);
  const snapshot = (await ctx.storage.get('checkpoint/prepared/100')).value;
  const preimage = ['openmayhem:paid-checkpoint-recovery:v1', 30, snapshot.slot,
    ctx.prepared.surrogate.address, snapshot.previous_tx, snapshot.snapshot_hash, op.key];
  const commitment = createHash('sha256').update(JSON.stringify(preimage), 'utf8').digest('hex');
  return { ctx, op, commitment };
}

test('C32 reconciles a committed original across lost ACK then advances the next ordinary slot exactly once', async () => {
  const {ctx, op, commitment} = await fictionalCommittedCheckpoint();
  const original = JSON.stringify(ctx.prepared);
  assert.equal(await matchesCommittedC30Checkpoint(op, ctx.storage, commitment), true);
  // Caller-supplied options cannot select a different production commitment.
  assert.equal(await ctx.contract.isPreparedCheckpointReplay(op, ctx.storage), false);
  await assert.rejects(ctx.transport.broadcast(ctx.prepared), /no matching historical canonical preparation/);
  await assert.rejects(ctx.contract.execute({ ...op, commitment }, ctx.storage), /expected CONTRACT_VERSION/);
  // The audited tuple is private. Exercise the same signed-proof/apply pipeline
  // with a synthetic frozen digest injected only into this isolated test instance.
  ctx.contract.isPreparedCheckpointReplay = (operation, storage) =>
    matchesCommittedC30Checkpoint(operation, storage, commitment);
  ctx.transport.funding = async () => ({ balance_au: '99999999999999999999', fee_au: ctx.active.expected_fee_au });
  const send = ctx.peer.msbClient.broadcastTransaction;
  ctx.peer.msbClient.broadcastTransaction = async payload => {
    assert.deepEqual(payload, ctx.prepared.payload);
    return send(payload);
  };
  await ctx.step();
  assert.equal(ctx.broadcasts, 1);
  assert.equal(ctx.journal.state.active.phase, 'BROADCAST_OR_UNCERTAIN');
  await ctx.step();
  assert.equal(ctx.journal.state.active.phase, 'MSB_CONFIRMED');
  await ctx.step();
  assert.equal(ctx.journal.completed.length, 1);
  assert.equal(ctx.journal.completed[0].checkpoint.tx, ctx.prepared.surrogate.tx);
  assert.equal(JSON.stringify(ctx.journal.completed[0].prepared), original);
  assert.equal(ctx.journal.completed[0].checkpoint.previous_tx, 'f9'.repeat(32));
  assert.equal(ctx.broadcasts, 1);
  assert.equal(ctx.appends, 1);
  assert.equal((await ctx.storage.get('txi/0')).value.err, null);
  assert.equal((await ctx.storage.get('txl')).value, 1);
  await ctx.transport.broadcast(ctx.prepared);
  assert.equal(ctx.broadcasts, 1);
  ctx.peer.msbClient.broadcastTransaction = send;
  ctx.journal.state.due_through = 101;
  await ctx.step();
  const next = ctx.journal.state.active.prepared;
  assert.equal(next.dispatch.value.contract_version, 32);
  assert.equal(next.dispatch.value.slot, 101);
  assert.notEqual(next.surrogate.tx, ctx.prepared.surrogate.tx);
  await ctx.step();
  await ctx.step();
  await ctx.step();
  assert.deepEqual(ctx.journal.completed.map(item => item.slot), [100, 101]);
  assert.equal(ctx.journal.completed[1].checkpoint.previous_tx, ctx.prepared.surrogate.tx);
  assert.equal(ctx.journal.completed[1].checkpoint.tx, next.surrogate.tx);
  assert.equal(ctx.journal.state.next_slot, 102);
  assert.equal(ctx.broadcasts, 2);
  assert.equal(ctx.appends, 2);
  assert.equal((await ctx.storage.get('txl')).value, 2);
  assert.equal((await ctx.storage.get('txi/1')).value.err, null);
});

test('C32 bound recovery refuses signed substitutes, unrelated old operations and altered canonical evidence', async () => {
  const {ctx, op, commitment} = await fictionalCommittedCheckpoint();
  const bytes = ctx.storage.snapshotBytes();
  for (const mutate of [
    o => { o.type = 'feature'; }, o => { o.key = 'af'.repeat(32); },
    o => { o.value.ipk = 'bf'.repeat(32); }, o => { o.value.dispatch.type = 'setRules'; },
    o => { o.value.dispatch.value.contract_version = 29; }, o => { o.value.dispatch.value.slot++; },
    o => { o.value.dispatch.value.snapshot_hash = 'cf'.repeat(32); }, o => { o.value.dispatch.value.extra = true; },
    o => { o.value.dispatch.extra = true; },
  ]) {
    const changed = structuredClone(op);
    mutate(changed);
    assert.equal(await matchesCommittedC30Checkpoint(changed, ctx.storage, commitment), false);
  }
  for (const [key, change] of [
    ['admin', () => 'ba'.repeat(32)],
    ['checkpoint/prepared/100', () => null],
    ['checkpoint/prepared/100', s => ({ ...s, prepared_by: 'da'.repeat(32) })],
    ['checkpoint/prepared/100', s => ({ ...s, previous_tx: 'ea'.repeat(32) })],
    ['checkpoint/prepared/100', s => ({ ...s, state: { ...s.state, catalog_version: 44 } })],
    ['checkpoint/prepared/100', s => ({ ...s, no_change: !s.no_change })],
    ['checkpoint/prepared/100', s => ({ ...s, state: { ...s.state, contract_version: 32 } })],
    ['checkpoint/preparing', () => null], ['checkpoint/preparing', p => ({ ...p, slot: 101 })],
    ['checkpoint/current', p => ({ ...p, slot: 98 })], ['checkpoint/current', p => ({ ...p, tx: 'ed'.repeat(32) })],
    [`tx/${op.key}`, () => 0], ['checkpoint/slot/100', () => ({ tx: 'fa'.repeat(32) })],
  ]) {
    const storage = MemoryStorage.fromSnapshotBytes(bytes);
    await storage.put(key, change((await storage.get(key))?.value));
    assert.equal(await matchesCommittedC30Checkpoint(op, storage, commitment), false);
  }
  // A fresh C30 signature for the same payer, snapshot and slot has a new TX.
  const signer = ctx.protocol.versionedTransactionObject;
  ctx.protocol.versionedTransactionObject = () => ctx.prepared.dispatch;
  const substitute = await ctx.protocol.preparePaidTransaction(ctx.prepared.dispatch);
  ctx.protocol.versionedTransactionObject = signer;
  assert.notEqual(substitute.surrogate.tx, ctx.prepared.surrogate.tx);
  assert.equal(await matchesCommittedC30Checkpoint(paidOperation(substitute), ctx.storage, commitment), false);
});

test('committed C30 original is retained on unsigned rotation and replaced only with double signed absence', async () => {
  const {ctx, commitment} = await fictionalCommittedCheckpoint();
  ctx.contract.isPreparedCheckpointReplay = (operation, storage) =>
    matchesCommittedC30Checkpoint(operation, storage, commitment);
  ctx.peer.msbClient.getTxvHex = async () => 'de'.repeat(32);
  await ctx.step();
  assert.equal(ctx.journal.state.status, 'awaiting_context_reconciliation');
  assert.deepEqual(ctx.journal.state.active, ctx.active);
  ctx.peer.msbClient.getTxvHex = async () => ctx.nextTxv;
  await ctx.step();
  assert.equal(ctx.journal.state.active.phase, 'PREPARED');
  assert.equal(ctx.journal.state.active.prepared.dispatch.value.contract_version, 32);
  assert.deepEqual(ctx.journal.state.active.retired_attempts[0].prepared, ctx.prepared);
  assert.equal(ctx.journal.state.active.retired_attempts[0].nonexecution_evidence.txv, ctx.nextTxv);
  assert.equal(ctx.broadcasts, 0);
  assert.equal(ctx.appends, 0);
});

test('committed and already applied C30 recovery binds every indexed success and original MSB proof field', async () => {
  const {ctx, commitment} = await fictionalCommittedCheckpoint();
  ctx.contract.isPreparedCheckpointReplay = (operation, storage) =>
    matchesCommittedC30Checkpoint(operation, storage, commitment);
  await assert.rejects(ctx.transport.broadcast(ctx.prepared), /response lost/);
  const { proof } = await ctx.transport.inspect(ctx.prepared, ctx.active.expected_fee_au, ctx.active);
  await ctx.transport.reconcileSubnet(ctx.prepared, proof);
  const good = (await ctx.storage.get('txi/0')).value;
  for (const changed of [
    {wp: 'ab'.repeat(32)}, {ipk: 'bc'.repeat(32)}, {tx: 'cd'.repeat(32)}, {err: 'rejected'},
    {val: {...good.val, value: {...good.val.value, contract_version: 32}}},
  ]) {
    await ctx.storage.put('txi/0', { ...good, ...changed });
    await assert.rejects(ctx.transport.reconcileSubnet(ctx.prepared, proof));
  }
  await ctx.storage.put('txi/0', good);
  for (const changed of [
    { validator: 'aa'.repeat(32) }, { operation_hex: 'invalid' }, { tx: 'bb'.repeat(32) },
    { fee_au: '1' }, { network_id: 999 }, { msb_bootstrap: 'cc'.repeat(32) }, { signed_length: 101 },
  ]) await assert.rejects(ctx.transport.reconcileSubnet(ctx.prepared, { ...proof, ...changed }));
  const checkpoint = await ctx.transport.reconcileSubnet(ctx.prepared, proof);
  assert.equal(checkpoint.tx, ctx.prepared.surrogate.tx);
  assert.equal(ctx.appends, 1);
  assert.equal(ctx.broadcasts, 1);
});

test('uncertain C31 checkpoint holds through restart and sparse MSB reads with no rebroadcast', async () => {
  const ctx = await previousWorker();
  await ctx.step();
  await ctx.step();
  assert.equal(ctx.journal.state.status, 'awaiting_previous_checkpoint_context');
  assert.deepEqual(ctx.journal.state.active, ctx.active);
  assert.equal(ctx.broadcasts, 0);
  assert.equal(ctx.appends, 0);
  await assert.rejects(ctx.transport.broadcast(ctx.prepared), /read-only reconciliation/);
  ctx.base.view.checkout = () => ({ get() { throw new Error('BLOCK_NOT_AVAILABLE'); }, close: async () => {} });
  await ctx.step();
  assert.equal(ctx.journal.state.status, 'degraded');
  assert.deepEqual(ctx.journal.state.active, ctx.active);
  assert.equal(ctx.broadcasts, 0);
});

test('C31 replacement needs changed signed system context, linked and current absence and the original slot', async () => {
  const ctx = await previousWorker();
  ctx.peer.msbClient.getTxvHex = async () => 'de'.repeat(32);
  await ctx.step();
  assert.deepEqual(ctx.journal.state.active, ctx.active);
  ctx.peer.msbClient.getTxvHex = async () => ctx.nextTxv;
  const checkout = ctx.base.view.checkout;
  ctx.base.view.checkout = length => length === 90
    ? { get: () => ({ value: b4a.from('present') }), close: async () => {} } : checkout(length);
  await ctx.step();
  assert.deepEqual(ctx.journal.state.active, ctx.active);
  ctx.base.view.checkout = checkout;
  ctx.reads.length = 0;
  await ctx.step();
  const replacement = ctx.journal.state.active;
  assert.deepEqual(ctx.reads, [100, 90, 100].map(length => [ctx.prepared.surrogate.tx, length]));
  assert.equal(replacement.phase, 'PREPARED');
  assert.equal(replacement.prepared.dispatch.value.contract_version, CONTRACT_VERSION);
  assert.equal(replacement.prepared.dispatch.value.snapshot_hash, ctx.active.snapshot_hash);
  assert.equal(replacement.prepared.surrogate.txv, ctx.nextTxv);
  assert.deepEqual(replacement.retired_attempts[0].prepared, ctx.prepared);
  assert.deepEqual(replacement.retired_attempts[0].nonexecution_evidence, {
    type: 'unexecuted_at_signed_context_change', tx: ctx.prepared.surrogate.tx,
    previous_txv: ctx.prepared.surrogate.txv, txv: ctx.nextTxv,
    system_key: '04'.repeat(32), system_signed_length: 95, view_key: '07'.repeat(32),
    linked_view_signed_length: 90, checked_view_signed_length: 100 });
  assert.equal(ctx.broadcasts, 0);
  assert.equal(ctx.appends, 0);
});

test('C31 rejects changed journal, canonical snapshot, preparing record or evidence of prior payment before replacement', async () => {
  for (const corrupt of [
    ctx => { ctx.journal.state.active.snapshot_hash = 'ff'.repeat(32); },
    ctx => { ctx.journal.state.active.phase = 'MSB_CONFIRMED'; },
    async ctx => { const snap = (await ctx.storage.get('checkpoint/prepared/100')).value;
      snap.state.catalog_version = 999; await ctx.storage.put('checkpoint/prepared/100', snap); },
    ctx => ctx.storage.put('checkpoint/preparing', { slot: 100, snapshot_hash: 'a'.repeat(64) }),
    ctx => ctx.storage.put(`tx/${ctx.prepared.surrogate.tx}`, 0),
  ]) {
    const ctx = await previousWorker();
    await corrupt(ctx);
    ctx.peer.msbClient.getTxvHex = async () => ctx.nextTxv;
    await ctx.step();
    assert.equal(ctx.journal.state.status, 'degraded');
    assert.deepEqual(ctx.journal.state.active.prepared, ctx.prepared);
    assert.equal(ctx.broadcasts, 0);
    assert.equal(ctx.appends, 0);
  }
});

test('paid C31 checkpoint stays pending without subnet appends and only exact signed indexed success completes locally', async () => {
  const ctx = await previousWorker();
  // The original pre-upgrade payment already reached MSB.
  await assert.rejects(ctx.peer.msbClient.broadcastTransaction(ctx.prepared.payload), /response lost/);
  await ctx.step();
  assert.equal(ctx.journal.state.status, 'previous_checkpoint_paid_pending_application');
  assert.equal(ctx.journal.state.active.phase, 'MSB_CONFIRMED');
  assert.equal(ctx.appends, 0);
  const c31 = new ContractV31(ctx.protocol, {});
  const apply31 = new TxOperation({ validateNode: () => true, validate: () => true }, {
    wallet: ctx.peer.wallet, protocolInstance: ctx.protocol, contractInstance: c31,
    msbClient: ctx.peer.msbClient, config: ctx.peer.config,
  });
  await apply31.handle(paidOperation(ctx.prepared, ctx.journal.state.active.proof), ctx.storage, ctx.peer.base, {});
  const indexed = (await ctx.storage.get('txi/0')).value;
  assert.equal(indexed.err, null, 'original contract applied this exact checkpoint');
  for (const fields of [{ err: 'rejected' }, { tx: 'aa'.repeat(32) }, { ipk: 'bb'.repeat(32) },
    { wp: 'cc'.repeat(32) }, { val: { ...indexed.val, type: 'noop' } }]) {
    await ctx.storage.put('txi/0', { ...indexed, ...fields });
    await ctx.step();
    assert.equal(ctx.journal.state.next_slot, 100);
    assert.equal(ctx.journal.completed.length, 0);
  }
  await ctx.storage.put('txi/0', indexed);
  await ctx.step();
  assert.equal(ctx.journal.state.next_slot, 101);
  assert.equal(ctx.journal.completed.length, 1);
  assert.equal(ctx.journal.completed[0].checkpoint.tx, ctx.prepared.surrogate.tx);
  assert.equal(ctx.broadcasts, 1);
  assert.equal(ctx.appends, 0);
});

for (const sourceVersion of [24, CONTRACT_VERSION]) {
  test(`v${sourceVersion} legacy paid checkpoint survives default UTF-8 restart exactly once`, async () => {
    const ctx = await harness();
    ctx.peer.config.txContentHash = 'legacy';
    const paid = sourceVersion === 24
      ? await historicalPreparation(ctx)
      : await ctx.transport.preparePayment(await ctx.transport.prepareSnapshot(100, 3000));
    const serialized = JSON.stringify(paid.dispatch);
    const utf8Hash = b4a.toString(await blake3(b4a.from(serialized, 'utf8')), 'hex');
    assert.equal(paid.surrogate.ch, b4a.toString(await blake3(serialized), 'hex'));
    assert.notEqual(paid.surrogate.ch, utf8Hash);
    const persistedBytes = JSON.stringify(paid);
    if (sourceVersion === 24) await assert.rejects(ctx.transport.broadcast(paid), /response lost/);
    const storage = MemoryStorage.fromSnapshotBytes(ctx.storage.snapshotBytes());
    delete ctx.peer.config.txContentHash;
    const protocol = new MayhemProtocol(ctx.peer, ctx.peer.base, ctx.peer.config);
    ctx.peer.protocol.instance = protocol;
    const restored = JSON.parse(persistedBytes);
    const contract = new MayhemContract(protocol, {});
    ctx.peer.contract.instance = contract;
    const apply = new TxOperation({ validateNode: () => true, validate: () => true }, {
      wallet: ctx.peer.wallet, protocolInstance: protocol, contractInstance: contract,
      msbClient: ctx.peer.msbClient, config: ctx.peer.config,
    });
    ctx.peer.base.append = (op) => apply.handle(op, storage, ctx.peer.base, {});
    const transport = new WriterCheckpointTransport({ peer: ctx.peer,
      releaseIdentity: { contractVersion: CONTRACT_VERSION } });
    transport.state = async (key) => (await storage.get(key))?.value ?? null;
    if (sourceVersion === CONTRACT_VERSION) {
      // The current-version preparation was persisted before its first payment.
      // Its first broadcast after restart must retain the complete signed payload.
      const originalBroadcast = ctx.peer.msbClient.broadcastTransaction;
      ctx.peer.msbClient.broadcastTransaction = async (payload) => {
        assert.deepEqual(payload, restored.payload);
        return originalBroadcast(payload);
      };
      await assert.rejects(transport.broadcast(restored), /response lost/);
    }
    const rehashed = structuredClone(restored);
    rehashed.surrogate.ch = utf8Hash;
    rehashed.payload.txo.ch = utf8Hash;
    await assert.rejects(transport.validatePrepared(rehashed), /bytes or signature changed/);
    const inspected = await transport.inspect(restored, '30000000000000000');
    await transport.reconcileSubnet(restored, inspected.proof);
    const checkpoint = await transport.reconcileSubnet(restored, inspected.proof);
    await ctx.peer.base.append(paidOperation(restored, inspected.proof));
    assert.equal(checkpoint.tx, restored.surrogate.tx);
    assert.equal((await storage.get('txl')).value, 1);
    assert.equal((await storage.get('txi/0')).value.err, null);
    assert.equal(ctx.broadcasts, 1);
    assert.equal(JSON.stringify(restored), persistedBytes, 'recovery must retain the signed preparation');
  });
}

test('fresh v24 dispatch cannot fabricate historical preparation', async () => {
  const ctx = await harness();
  const paid = await historicalPreparation(ctx);
  assert.throws(() => validateMayhemOperationContractVersion(paidOperation(paid)), new RegExp(`expected CONTRACT_VERSION ${CONTRACT_VERSION}, got 24`));
  await ctx.storage.del('checkpoint/prepared/100');
  await assert.rejects(ctx.contract.execute(paidOperation(paid), ctx.storage), new RegExp(`expected CONTRACT_VERSION ${CONTRACT_VERSION}, got 24`));
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
    await assert.rejects(ctx.contract.execute(paidOperation(paid), ctx.storage), new RegExp(`expected CONTRACT_VERSION ${CONTRACT_VERSION}, got 24`));
    await assert.rejects(ctx.transport.broadcast(paid), /no matching historical canonical preparation/);
  }
  const ctx = await harness();
  const paid = await historicalPreparation(ctx);
  const unrelated = paidOperation(paid);
  unrelated.value.dispatch = { type: 'setRules', value: { op: 'set_rules', ver: 1,
    hash: 'aa'.repeat(32), contract_version: 24 } };
  await assert.rejects(ctx.contract.execute(unrelated, ctx.storage), new RegExp(`expected CONTRACT_VERSION ${CONTRACT_VERSION}, got 24`));
  const mutated = structuredClone(paid);
  mutated.dispatch.value.snapshot_hash = '99'.repeat(32);
  await assert.rejects(ctx.transport.broadcast(mutated), /bytes or signature changed/);
  assert.deepEqual(ctx.protocol.txHashDispatchCandidates(paid.dispatch), [paid.dispatch]);
});

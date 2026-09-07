import assert from 'node:assert/strict';
import test from 'node:test';
import b4a from 'b4a';
import MayhemContract, { CONTRACT_VERSION, adminContractTxDigest } from '../contract/contract.js';
import { MemoryStorage, execute, executeFeature, makeIdentity, makeVerifier } from './helpers/contract.js';

async function setup() {
  const admin = await makeIdentity();
  const context = { contract_version: CONTRACT_VERSION, network_id: 918,
    msb_bootstrap: '77'.repeat(32), subnet_bootstrap: '66'.repeat(32) };
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const contract = new MayhemContract({ peer: {
    wallet: makeVerifier(admin.wallet), config: { bootstrap: context.subnet_bootstrap },
    msbClient: { bootstrapHex: context.msb_bootstrap, networkId: context.network_id },
  } }, {});
  return { admin, context, storage, contract };
}

async function prepare(ctx, slot, observedAt = slot * 30) {
  const result = await execute(ctx.contract, ctx.storage, 'prepareStateCheckpoint', {
    op: 'prepare_state_checkpoint', slot, observed_at: observedAt,
    contract_code_sha256: '88'.repeat(32),
  }, ctx.admin.publicKey, slot + 1000);
  assert.equal(result.ok, true, result.message);
  return { op: 'state_checkpoint', slot, snapshot_hash: result.snapshot.snapshot_hash };
}

test('idle slots create distinct paid checkpoints without advancing economic epochs', async () => {
  const ctx = await setup();
  for (const slot of [100, 101, 102]) {
    const value = await prepare(ctx, slot);
    const paid = await execute(ctx.contract, ctx.storage, 'stateCheckpoint', value, ctx.admin.publicKey, slot);
    assert.equal(paid.ok, true, paid.message);
    assert.equal(paid.checkpoint.scheduled_at, slot * 30);
    assert.equal(paid.checkpoint.no_change, slot !== 100);
    assert.equal(await ctx.storage.get('epoch/apply/state'), null);
    assert.equal(await ctx.storage.get('receipt/ingress'), null);
  }
  assert.notEqual((await ctx.storage.get('checkpoint/slot/100')).value.tx,
    (await ctx.storage.get('checkpoint/slot/101')).value.tx);
});

test('free Feature and signed admin envelope cannot bypass the paid checkpoint route', async () => {
  const ctx = await setup();
  const value = await prepare(ctx, 100);
  await executeFeature(ctx.contract, ctx.storage, 'mayhem_feature', 'checkpoint/100', value, ctx.admin.publicKey);
  assert.equal(await ctx.storage.get('checkpoint/current'), null);
  const unsigned = { op: 'admin_contract_tx', prepared_command: { type: 'stateCheckpoint', value },
    address: ctx.admin.publicKey, context: ctx.context, nonce: '99'.repeat(32), sim: false };
  const tx = await adminContractTxDigest(unsigned);
  const envelope = { ...unsigned, tx,
    signature: b4a.toString(ctx.admin.wallet.sign(b4a.from(tx, 'hex')), 'hex') };
  const rejected = await executeFeature(ctx.contract, ctx.storage, 'mayhem_feature',
    `admin/contract-tx/${tx}`, envelope, ctx.admin.publicKey);
  assert.match(rejected.message, /paid MSB transaction/);
  assert.equal(await ctx.storage.get('checkpoint/current'), null);
});

test('paid checkpoint binds its immutable observation while live receipts and catalog advance', async () => {
  const ctx = await setup();
  const value = await prepare(ctx, 100);
  const prepared = (await ctx.storage.get('checkpoint/prepared/100')).value;
  await ctx.storage.put('catalog/current', { catalog_hash: 'aa'.repeat(32), ver: 2 });
  await ctx.storage.put('receipt/epoch/1/index', { count: 100 });
  ctx.storage = MemoryStorage.fromSnapshotBytes(ctx.storage.snapshotBytes());
  const result = await execute(ctx.contract, ctx.storage, 'stateCheckpoint', value, ctx.admin.publicKey, 100);
  assert.equal(result.ok, true, result.message);
  assert.equal(result.checkpoint.snapshot_hash, prepared.snapshot_hash);
  assert.equal(prepared.state.catalog_hash, null);
  const replay = await execute(ctx.contract, ctx.storage, 'stateCheckpoint', value, ctx.admin.publicKey, 100);
  assert.equal(replay.idempotent, true);
  const duplicate = await execute(ctx.contract, ctx.storage, 'stateCheckpoint', value, ctx.admin.publicKey, 101);
  assert.match(duplicate.message, /already has a paid transaction/);
});

test('checkpoint history rejects gaps, wrong sender, wrong snapshot and observation rollback', async () => {
  const ctx = await setup();
  const value = await prepare(ctx, 100, 3100);
  const stranger = await makeIdentity();
  assert.match((await execute(ctx.contract, ctx.storage, 'stateCheckpoint', value,
    stranger.publicKey, 100)).message, /Admin required/);
  assert.match((await execute(ctx.contract, ctx.storage, 'stateCheckpoint', {
    ...value, snapshot_hash: 'ff'.repeat(32),
  }, ctx.admin.publicKey, 100)).message, /Matching canonical/);
  assert.equal((await execute(ctx.contract, ctx.storage, 'stateCheckpoint', value, ctx.admin.publicKey, 100)).ok, true);
  for (const [slot, observedAt] of [[102, 3200], [101, 3099]]) {
    const result = await execute(ctx.contract, ctx.storage, 'prepareStateCheckpoint', {
      op: 'prepare_state_checkpoint', slot, observed_at: observedAt, contract_code_sha256: '88'.repeat(32),
    }, ctx.admin.publicKey, 999);
    assert.match(result.message, /follow the last paid slot/);
  }
});

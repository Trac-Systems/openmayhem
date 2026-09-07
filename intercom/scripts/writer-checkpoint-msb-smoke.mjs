// Disposable development MSB consensus ledger: real signatures, fee deduction,
// signed proofs and replay protection. The subnet view is an in-memory fixture.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';
import b4a from 'b4a';
import Wallet from 'trac-peer/src/wallet.js';
import { MsbClient } from 'trac-peer/src/msbClient.js';
import { TxOperation } from 'trac-peer/src/operations/tx/index.js';
import TransactionPool from 'trac-peer/src/transaction/transactionPool.js';
import { randomBytes } from 'node:crypto';
import PeerWallet from 'trac-wallet';
import { MainSettlementBus } from 'trac-msb/src/index.js';
import { Config } from 'trac-msb/src/config/config.js';
import { applyStateMessageFactory } from 'trac-msb/src/messages/state/applyStateMessageFactory.js';
import { safeEncodeApplyOperation } from 'trac-msb/src/utils/protobuf/operationHelpers.js';
import { createMayhemMsbConfig } from '../src/network-config.js';
import { bigIntTo16ByteBuffer } from 'trac-msb/src/utils/amountSerialization.js';
import MayhemContract from '../contract/contract.js';
import MayhemProtocol from '../contract/protocol.js';
import { verifyReleaseIdentity } from '../src/release-identity.js';
import { WriterCheckpointTransport } from '../src/writer-checkpoint-transport.js';
import { CheckpointJournal, WriterCheckpointWorker } from '../src/writer-checkpoint-worker.js';
import { MemoryStorage, executeFeature } from '../tests/helpers/contract.js';

process.umask(0o077);
const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'writer-checkpoint-msb-smoke-'));
let admin, worker;
const timeout = setTimeout(() => { console.error('Private MSB smoke timed out.'); process.exit(1); }, 90000);
try {
  let config = createMayhemMsbConfig('development', {
    storesDirectory: directory + '/', storeName: 'admin', bootstrap: randomBytes(32).toString('hex'),
    channel: 'mayhem-checkpoint-v22-smoke', enableInteractiveMode: false, enableWallet: true,
    enableErrorApplyLogs: true, enableTxApplyLogs: true,
  });
  const validator = new PeerWallet({ networkPrefix: config.addressPrefix });
  await validator.ready;
  await validator.generateKeyPair();
  let msb = new MainSettlementBus(config, validator);
  await msb.ready();
  config = new Config({ bootstrap: b4a.toString(msb.state.writingKey, 'hex') }, config);
  await msb.close();
  msb = new MainSettlementBus(config, validator);
  admin = { msb, config };
  await msb.ready();
  const append = async (payload) => {
    await msb.state.append(safeEncodeApplyOperation(payload));
    await sleep(25);
    await msb.state.base.forceFastForward();
    await sleep(25);
  };
  await msb.state.append(null);
  await append(await applyStateMessageFactory(validator, config).buildCompleteAddAdminMessage(
    validator.address, msb.state.writingKey, await msb.state.getIndexerSequenceState()));
  assert.ok(await msb.state.getAdminEntry());
  // Development reuses the native TNK address/network encoding; its independent
  // genesis/bootstrap and private stores identify the disposable ledger.
  assert.notEqual(b4a.toString(config.bootstrap, 'hex'),
    'acbc3a4344d3a804101d40e53db1dda82b767646425af73599d4cd6577d69685');
  assert.equal(b4a.toString(config.bootstrap, 'hex'), b4a.toString(msb.state.writingKey, 'hex'));
  const msbClient = new MsbClient(admin.msb);
  await msbClient.ready();
  const deployer = new PeerWallet({ networkPrefix: config.addressPrefix });
  await deployer.ready;
  await deployer.generateKeyPair();
  await append(await applyStateMessageFactory(validator, config).buildCompleteBalanceInitializationMessage(
    validator.address, deployer.address, bigIntTo16ByteBuffer(10n ** 18n), await msb.state.getIndexerSequenceState()));
  const deployment = await applyStateMessageFactory(deployer, config).buildPartialBootstrapDeploymentMessage(
    deployer.address, '43'.repeat(32), '44'.repeat(32), await msb.state.getIndexerSequenceState(), 'json');
  const bdo = deployment.bdo;
  await append(await applyStateMessageFactory(validator, config).buildCompleteBootstrapDeploymentMessage(
    deployment.address, ...['tx', 'txv', 'bs', 'ic', 'in', 'is'].map((key) => b4a.from(bdo[key], 'hex'))));
  assert.ok(await msb.state.getRegisteredBootstrapEntry('43'.repeat(32)));
  const wallet = new Wallet();
  await wallet.ready;
  await wallet.generateKeyPair();
  const address = msbClient.pubKeyHexToAddress(wallet.publicKey);
  await append(await applyStateMessageFactory(validator, config).buildCompleteBalanceInitializationMessage(
    validator.address, address, bigIntTo16ByteBuffer(10n ** 20n), await msb.state.getIndexerSequenceState()));
  const storage = new MemoryStorage({ admin: wallet.publicKey });
  const peer = { wallet, writerLocalKey: '42'.repeat(32), msbClient,
    config: { bootstrap: '43'.repeat(32), maxMsbSignedLength: 1e9,
      maxMsbSignedLengthFutureDelta: 100000, maxMsbApplyOperationBytes: 4096 },
    txPool: new TransactionPool({ txPoolMaxSize: 64 }),
    base: { writable: true, view: {
      core: { signedLength: 50 }, get: (key) => storage.get(key),
      checkout: () => {
        const snapshot = MemoryStorage.fromSnapshotBytes(storage.snapshotBytes());
        return { close: async () => {}, get: async (key) => {
          const entry = await snapshot.get(key);
          return entry ? { seq: 1, key, value: entry.value } : null;
        } };
      },
    } },
  };
  const protocol = new MayhemProtocol(peer, peer.base, peer.config);
  peer.protocol = { instance: protocol };
  const contract = new MayhemContract(protocol, {});
  peer.contract = { instance: contract };
  const txOperation = new TxOperation({ validateNode: () => true, validate: () => true }, {
    wallet, protocolInstance: protocol, contractInstance: contract, msbClient, config: peer.config,
  });
  let dropSubnetApplication = true;
  peer.base.append = (op) => dropSubnetApplication ? Promise.resolve() :
    txOperation.handle(op, storage, peer.base, {});
  const transport = new WriterCheckpointTransport({ peer,
    feature: { submit: async (key, value) => executeFeature(contract, storage,
      'mayhem_feature', key, value, wallet.publicKey) },
    releaseIdentity: verifyReleaseIdentity(),
  });
  const initial = await transport.funding();
  assert.equal(initial.balance_au, '100000000000000000000');
  let clock = 3000000;
  const journalDirectory = path.join(directory, 'journal');
  const openWorker = () => new WriterCheckpointWorker({
    journal: new CheckpointJournal(journalDirectory), transport,
    now: () => clock, monotonicNow: () => clock, log: () => {},
  });
  worker = openWorker();
  await worker.initialize();
  worker.schedule();
  const broadcast = transport.broadcast.bind(transport);
  transport.broadcast = async (prepared) => {
    await broadcast(prepared);
    throw new Error('Injected lost acknowledgement after actual signed MSB payment.');
  };
  await worker.step();
  const paid = worker.journal.state.active.prepared;
  assert.ok(paid, JSON.stringify(worker.status()));
  const proof = await transport.inspect(paid, initial.fee_au);
  assert.equal(proof.confirmed, true);
  const afterPayment = await transport.funding();
  assert.equal(BigInt(initial.balance_au) - BigInt(afterPayment.balance_au), BigInt(initial.fee_au));
  assert.equal(await storage.get('checkpoint/current'), null);
  await worker.stop();
  dropSubnetApplication = false;
  transport.broadcast = broadcast;
  worker = openWorker();
  await worker.initialize();
  for (let i = 0; i < 5 && worker.journal.state.next_slot === 100; i++) await worker.step();
  assert.equal(worker.journal.state.next_slot, 101);
  assert.equal((await transport.funding()).balance_au, afterPayment.balance_au);
  // An exact paid MSB replay must not deduct another fee, even if rejected.
  await broadcast(paid).catch(() => {});
  assert.equal((await transport.funding()).balance_au, afterPayment.balance_au);
  for (const slot of [101, 102]) {
    clock = slot * 30000;
    worker.schedule();
    for (let i = 0; i < 20 && worker.journal.state.next_slot === slot; i++) {
      await worker.step();
      await sleep(25);
    }
    assert.equal(worker.journal.state.next_slot, slot + 1, JSON.stringify(worker.status()));
  }
  const final = await transport.funding();
  assert.equal(BigInt(initial.balance_au) - BigInt(final.balance_au), 3n * BigInt(initial.fee_au));
  const completed = [100, 101, 102].map((slot) => JSON.parse(
    fs.readFileSync(path.join(journalDirectory, 'completed', `${slot}.json`), 'utf8')));
  assert.equal(new Set(completed.map((record) => record.checkpoint.tx)).size, 3);
  assert.equal(completed[2].checkpoint.no_change, true);
  const result = { ok: true, ledger: 'private-development-msb', subnet: 'memory-with-real-tx-verifier',
    directory, slots: 3, lost_ack_restart: true, paid_without_subnet_recovered: true,
    duplicate_fee_prevented: true, fee_per_tx_au: initial.fee_au,
    total_fees_au: (BigInt(initial.balance_au) - BigInt(final.balance_au)).toString(),
    signed_length: final.signed_length };
  fs.writeFileSync(path.join(directory, 'result.json'), JSON.stringify(result, null, 2));
  console.log(JSON.stringify(result));
} finally {
  clearTimeout(timeout);
  await worker?.stop();
  await admin?.msb.close();
}

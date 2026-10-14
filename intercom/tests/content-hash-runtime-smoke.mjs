import b4a from 'b4a';
import Protocol from 'trac-peer/src/artifacts/protocol.js';
import {
  createHash, createLegacyHash, createTxContentHash,
  acceptedTxContentHashes, isAcceptedTxContentHash,
} from 'trac-peer/src/utils/types.js';

// Runs unchanged under Node and the production Pear/Bare sidecar. Network and
// storage are local fixtures; no peer is started and no transaction is sent.
let assertions = 0;
function equal(actual, expected, label) {
  assertions++;
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(`${label}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
  }
}

const context = {
  networkId: 918,
  txv: 'af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262',
  iw: 'b596e093b22f80d4f66ee25e79b95866b44133e5d2b2d673d4fcac443cf30661',
  bs: 'b596e093b22f80d4f66ee25e79b95866b44133e5d2b2d673d4fcac443cf30661',
  mbs: '12f7f1668eac2e691e17cbc6a53e509c5cee78cdcac562313091c64e5fd077d6',
  nonce: '01'.repeat(32),
};
const value = { at: 7, op: 'probe_result', pass: false, probe_id: 'aa'.repeat(32) };
// Fixed content/message vectors also cover the Rust gateway's UTF-8 producer.
const vectors = [
  { dispatch: { type: 'probeResult', value },
    utf8: '937b6f4635f87d8c8d0db8e2653688c0932964220196084b32c92c40ffc5278a',
    legacy: '43f47a87993e605041034edc2cc82fbff5e4eca1bf8654a9a857d3f214509a61',
    utf8Tx: 'db81cc256f7153a9b14187b0ae35e936743c07e11d2e1580db8270bf5154da16',
    legacyTx: 'd0418782738f7c76472480d7ceb0c1ae8161ef201265abc8f9b3b764fe8aeaab' },
  { dispatch: { type: 'probeResult', value: { at: 7, model_id: 'modèle/東京🚀',
      op: value.op, pass: value.pass, probe_id: value.probe_id } },
    utf8: 'b143e81b8b1166ab210f135983189197072aa474a3a306bcf4e8fc9e4c318839',
    legacy: '4bd1fd3dc40a39f8587af4a7c8daf90e4673bd70f7d8cc1c5827a71275437a7b',
    utf8Tx: '250e710df35ab8988030e6a6c09303520e0dd254370d78538203e09713e67cea',
    legacyTx: '3b00d1f108297ef0817d1fc29b75f7aa3c2f64f079a6de2e024061d7e1cdcead' },
];

equal(await createHash('abc'),
  '6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85', 'BLAKE3 text vector');
equal(await createHash(b4a.from('abc', 'utf8')), await createHash('abc'), 'byte/text agreement');

let simulations = 0;
for (const vector of vectors) {
  const json = JSON.stringify(vector.dispatch);
  equal(await createHash(json), vector.utf8, 'UTF-8 content vector');
  equal(await createLegacyHash(json), vector.legacy, 'legacy content vector');
  for (const mode of [undefined, 'utf8', 'legacy']) {
    const produced = mode === 'legacy' ? vector.legacy : vector.utf8;
    equal(await createTxContentHash(json, mode), produced, 'configured content hash');
    equal(await acceptedTxContentHashes(json, mode), [vector.utf8, vector.legacy], 'accepted hashes');
    equal(await isAcceptedTxContentHash(json, vector.utf8.toUpperCase(), mode), true, 'UTF-8 acceptance');
    equal(await isAcceptedTxContentHash(json, vector.legacy, mode), true, 'legacy acceptance');
    equal(await isAcceptedTxContentHash(json, 'ff'.repeat(32), mode), false, 'unrelated hash rejected');

    let expectedHash, expectedTx, executed = 0;
    const address = '22'.repeat(32);
    const signature = '33'.repeat(64);
    const peer = {
      config: { bootstrap: b4a.from(context.bs, 'hex'), txContentHash: mode },
      writerLocalKey: context.iw,
      base: { view: { get: async () => null } },
      msbClient: {
        networkId: context.networkId, bootstrapHex: context.mbs,
        getTxvHex: async () => context.txv,
        pubKeyHexToAddress: (key) => { equal(key, address, 'requester identity'); return 'fixture-address'; },
        validateTransaction: async (payload) => {
          equal(payload.txo.tx, expectedTx, 'signed transaction hash');
          equal(payload, { type: 12, address: 'fixture-address', txo: {
            tx: expectedTx, txv: context.txv, iw: context.iw, in: context.nonce,
            ch: expectedHash, is: signature, bs: context.bs, mbs: context.mbs,
          } }, 'simulation preserves signed payload');
        },
      },
      contract: { instance: { execute: async (op) => {
        equal(op.key, expectedTx, 'contract transaction key');
        equal(op.value.dispatch, vector.dispatch, 'contract dispatch');
        executed++;
        return { ok: true };
      } } },
    };
    const protocol = new Protocol(peer, peer.base, {});
    equal(await protocol.resolveTxContentHash(vector.dispatch), produced, 'own transaction hash');
    for (const encoding of ['utf8', 'legacy']) {
      expectedHash = vector[encoding];
      expectedTx = vector[`${encoding}Tx`];
      equal(await protocol.generateTx(context.networkId, context.txv, context.iw,
        expectedHash, context.bs, context.mbs, context.nonce), expectedTx, 'MSB message vector');
      for (const explicitHash of [false, true]) {
        const surrogate = { tx: expectedTx, nonce: context.nonce, address, signature };
        if (explicitHash) surrogate.ch = expectedHash;
        equal(await protocol.resolveTxContentHash(vector.dispatch, surrogate), expectedHash,
          explicitHash ? 'persisted content hash' : 'signed transaction resolves omitted content hash');
        equal(await protocol.simulateTransaction('44'.repeat(32), vector.dispatch, surrogate),
          { ok: true }, 'simulation succeeds');
        simulations++;
      }
    }
    const before = executed;
    let rejected = false;
    try {
      await protocol.simulateTransaction('44'.repeat(32), vector.dispatch,
        { tx: 'ff'.repeat(32), nonce: context.nonce, address, signature });
    } catch (error) {
      rejected = String(error.message).startsWith('signed transaction hash:');
    }
    equal(rejected, true, 'unmatched transaction rejected by validation');
    equal(executed, before, 'failed validation prevents contract execution');
  }
}

console.log(JSON.stringify({ ok: true, runtime: globalThis.Bare ? 'bare' : 'node',
  version: globalThis.Bare?.versions?.bare ?? globalThis.process?.versions?.node,
  assertions, simulations }));
if (globalThis.Bare) Bare.exit(0);

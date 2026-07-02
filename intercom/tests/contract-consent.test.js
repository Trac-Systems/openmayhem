import assert from 'node:assert/strict';
import test from 'node:test';
import b4a from 'b4a';
import PeerWallet from 'trac-wallet';
import MayhemContract, { consentMessage } from '../contract/contract.js';

const ZERO_HEX = '0'.repeat(64);

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
}

const makeTxKey = (n) => n.toString(16).padStart(64, '0');

const makeOperation = (type, value, sender, txNo) => ({
  type: 'tx',
  key: makeTxKey(txNo),
  value: {
    dispatch: { type, value },
    ipk: sender,
    wp: ZERO_HEX,
  },
});

const execute = (contract, storage, type, value, sender, txNo) =>
  contract.execute(makeOperation(type, value, sender, txNo), storage);

async function makeIdentity() {
  const wallet = new PeerWallet();
  await wallet.ready;
  await wallet.generateKeyPair();
  return {
    wallet,
    publicKey: b4a.toString(wallet.publicKey, 'hex'),
  };
}

const makeVerifier = (wallet) => ({
  verify(signature, message, publicKey) {
    return wallet.verify(
      b4a.from(signature, 'hex'),
      b4a.from(String(message)),
      b4a.from(publicKey, 'hex')
    );
  },
});

const signConsent = (wallet, ver, hash) =>
  b4a.toString(wallet.sign(b4a.from(consentMessage(ver, hash))), 'hex');

test('MayhemContract consent gates ops and requires re-consent after rules bump', async () => {
  const identity = await makeIdentity();
  const storage = new MemoryStorage({ admin: identity.publicKey });
  const protocol = { peer: { wallet: makeVerifier(identity.wallet) } };
  const contract = new MayhemContract(protocol, {});
  const hashV1 = 'a'.repeat(64);
  const hashV2 = 'b'.repeat(64);

  const setV1 = await execute(
    contract,
    storage,
    'setRules',
    { op: 'set_rules', ver: 1, hash: hashV1 },
    identity.publicKey,
    1
  );
  assert.equal(setV1.ok, true);
  assert.deepEqual(await storage.get('rules/current'), {
    value: { ver: 1, hash: hashV1, activated_at: makeTxKey(1) },
  });

  const beforeConsent = await execute(
    contract,
    storage,
    'gatedNoop',
    { op: 'gated_noop' },
    identity.publicKey,
    2
  );
  assert.match(beforeConsent.message, /consent required/i);

  const consentV1 = await execute(
    contract,
    storage,
    'consent',
    {
      op: 'consent',
      ver: 1,
      hash: hashV1,
      sig: signConsent(identity.wallet, 1, hashV1),
    },
    identity.publicKey,
    3
  );
  assert.equal(consentV1.ok, true);

  const afterConsent = await execute(
    contract,
    storage,
    'gatedNoop',
    { op: 'gated_noop' },
    identity.publicKey,
    4
  );
  assert.equal(afterConsent.ok, true);

  const setV2 = await execute(
    contract,
    storage,
    'setRules',
    { op: 'set_rules', ver: 2, hash: hashV2 },
    identity.publicKey,
    5
  );
  assert.equal(setV2.ok, true);

  const staleConsent = await execute(
    contract,
    storage,
    'gatedNoop',
    { op: 'gated_noop' },
    identity.publicKey,
    6
  );
  assert.match(staleConsent.message, /rules version 2/i);

  const consentV2 = await execute(
    contract,
    storage,
    'consent',
    {
      op: 'consent',
      ver: 2,
      hash: hashV2,
      sig: signConsent(identity.wallet, 2, hashV2),
    },
    identity.publicKey,
    7
  );
  assert.equal(consentV2.ok, true);

  const afterReconsent = await execute(
    contract,
    storage,
    'gatedNoop',
    { op: 'gated_noop' },
    identity.publicKey,
    8
  );
  assert.equal(afterReconsent.ok, true);
});

test('MayhemContract rejects consent with a bad signature', async () => {
  const identity = await makeIdentity();
  const other = await makeIdentity();
  const storage = new MemoryStorage({ admin: identity.publicKey });
  const protocol = { peer: { wallet: makeVerifier(identity.wallet) } };
  const contract = new MayhemContract(protocol, {});
  const hash = 'c'.repeat(64);

  await execute(
    contract,
    storage,
    'setRules',
    { op: 'set_rules', ver: 1, hash },
    identity.publicKey,
    1
  );

  const badConsent = await execute(
    contract,
    storage,
    'consent',
    {
      op: 'consent',
      ver: 1,
      hash,
      sig: signConsent(other.wallet, 1, hash),
    },
    identity.publicKey,
    2
  );
  assert.match(badConsent.message, /invalid consent signature/i);
});

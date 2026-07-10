import assert from 'node:assert/strict';
import test from 'node:test';
import MayhemContract from '../contract/contract.js';
import {
  MemoryStorage,
  execute,
  makeIdentity,
  makeTxKey,
  makeVerifier,
  signConsent,
} from './helpers/contract.js';

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
    value: {
      ver: 1,
      hash: hashV1,
      set_by: identity.publicKey,
      set_by_role: 'admin',
      activated_at: makeTxKey(1),
    },
  });
  assert.deepEqual(await storage.get('epoch/apply/state'), {
    value: {
      updated_epoch: 0,
      updated_at: null,
      last_apply_hash: null,
    },
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

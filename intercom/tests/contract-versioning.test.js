import assert from 'node:assert/strict';
import test from 'node:test';
import b4a from 'b4a';
import MayhemContract, {
  CONTRACT_VERSION,
  NEXT_SESSION_RECEIPT_SCHEMA_VERSION,
  SESSION_RECEIPT_SCHEMA_VERSION,
  consentMessage,
  providerLifecycleIntentMessage,
  receiptMessage,
} from '../contract/contract.js';
import {
  MemoryStorage,
  execute,
  executeFeature,
  makeIdentity,
  makeVerifier,
  providerLifecycleFeatureKey,
  signConsent,
  signProviderLifecycleIntent,
} from './helpers/contract.js';

const rulesHash = '6'.repeat(64);
const versioningLockedRateMap = [
  { unit: 'input_token', per_unit_au: '100', granularity: 30 },
  { unit: 'output_token', per_unit_au: '100', granularity: 30 },
];

test('contract reports the exported contract version', async () => {
  const user = await makeIdentity();
  const storage = new MemoryStorage({ admin: user.publicKey });
  const protocol = { peer: { wallet: makeVerifier(user.wallet) } };
  const contract = new MayhemContract(protocol, {});

  const result = await contract.noop(storage);
  assert.equal(result.version, CONTRACT_VERSION);
});

test('contract accepts legacy v1 and current v2 consent signatures', async () => {
  assert.notEqual(consentMessage(1, rulesHash, 1), consentMessage(1, rulesHash));
  assert.match(consentMessage(1, rulesHash), /"signing_version":2/);

  for (const signingVersion of [1, 2]) {
    const admin = await makeIdentity();
    const user = await makeIdentity();
    const storage = new MemoryStorage({ admin: admin.publicKey });
    const protocol = { peer: { wallet: makeVerifier(user.wallet) } };
    const contract = new MayhemContract(protocol, {});

    let result = await execute(
      contract,
      storage,
      'setRules',
      { op: 'set_rules', ver: 1, hash: rulesHash },
      admin.publicKey,
      1
    );
    assert.equal(result.ok, true, result.message);

    result = await execute(
      contract,
      storage,
      'consent',
      {
        op: 'consent',
        ver: 1,
        hash: rulesHash,
        sig: signConsent(user.wallet, 1, rulesHash, signingVersion),
      },
      user.publicKey,
      2
    );
    assert.equal(result.ok, true, result.message);
    assert.equal((await storage.get(`consent/${user.publicKey}`)).value.hash, rulesHash);
  }
});

test('provider lifecycle features accept v1 and v2 signing keys during rollout', async () => {
  for (const signingVersion of [1, 2]) {
    const admin = await makeIdentity();
    const provider = await makeIdentity();
    const storage = new MemoryStorage({
      admin: admin.publicKey,
      'rules/current': { ver: 1, hash: rulesHash },
      [`consent/${provider.publicKey}`]: { ver: 1, hash: rulesHash, at: 'seed' },
    });
    const protocol = { peer: { wallet: makeVerifier(provider.wallet) } };
    const contract = new MayhemContract(protocol, {});
    const intent = {
      op: 'register_provider',
      provider: provider.publicKey,
      nonce: `${signingVersion}`.repeat(64),
    };

    assert.notEqual(
      providerLifecycleIntentMessage(intent, 1),
      providerLifecycleIntentMessage(intent, 2)
    );

    await executeFeature(
      contract,
      storage,
      'mayhem_feature',
      await providerLifecycleFeatureKey(intent, signingVersion),
      {
        op: 'provider_lifecycle',
        intent,
        sig: signProviderLifecycleIntent(provider.wallet, intent, signingVersion),
      },
      admin.publicKey
    );

    assert.equal((await storage.get(`prov/${provider.publicKey}`)).value.status, 'active');
  }
});

test('receipt verifier accepts v1 and v2 receipt signing payloads', async () => {
  const user = await makeIdentity();
  const provider = await makeIdentity();
  const enclave = await makeIdentity();
  const contract = new MayhemContract({ peer: { wallet: makeVerifier(enclave.wallet) } }, {});
  const body = {
    schema_version: 1,
    session_id: 'session-versioning',
    seq: 1,
    final: true,
    rail: 'fiat',
    user: user.publicKey,
    provider: provider.publicKey,
    enclave_id: 'enclave-versioning',
    model_id: 'model/versioning',
    price_ver: 1,
    locked_rate_map: versioningLockedRateMap,
    rules_ver: 1,
    usage: { in: 10, out: 20 },
    au_owed_cum: '100',
    prompt_hash: 'a'.repeat(64),
    ts: 1_000,
  };

  for (const signingVersion of [1, 2]) {
    const message = b4a.from(receiptMessage(body, signingVersion));
    assert.equal(
      contract.verifyReceiptEnvelope({
        body,
        enclave_pubkey: enclave.publicKey,
        enclave_sig: b4a.toString(enclave.wallet.sign(message), 'hex'),
        user_sig: b4a.toString(user.wallet.sign(message), 'hex'),
      }),
      true
    );
  }
});

test('receipt schema migration normalizes legacy usage maps', async () => {
  const user = await makeIdentity();
  const provider = await makeIdentity();
  const enclave = await makeIdentity();
  const contract = new MayhemContract({ peer: { wallet: makeVerifier(enclave.wallet) } }, {});
  const body = {
    schema_version: 1,
    session_id: 'session-schema-migration',
    seq: 1,
    final: true,
    rail: 'fiat',
    user: user.publicKey,
    provider: provider.publicKey,
    enclave_id: 'enclave-schema-migration',
    model_id: 'model/schema-migration',
    price_ver: 1,
    locked_rate_map: versioningLockedRateMap,
    rules_ver: 1,
    usage: { in: 10, out_tokens: 20 },
    au_owed_cum: '100',
    prompt_hash: 'a'.repeat(64),
    ts: 1_000,
  };
  const message = b4a.from(receiptMessage(body));
  const envelope = {
    body,
    enclave_pubkey: enclave.publicKey,
    enclave_sig: b4a.toString(enclave.wallet.sign(message), 'hex'),
    user_sig: b4a.toString(user.wallet.sign(message), 'hex'),
  };
  contract.storage = new MemoryStorage({
    [`enclave/${body.enclave_id}`]: {
      enclave_id: body.enclave_id,
      model_id: body.model_id,
      model_class: 'text-generation',
      caps: { chat: true, ctx: 8192 },
    },
  });

  const migrated = await contract.normalizeReceiptEnvelope(envelope, {
    targetSchemaVersion: NEXT_SESSION_RECEIPT_SCHEMA_VERSION,
  });
  assert.equal(migrated instanceof Error, false, migrated.message);
  assert.equal(migrated.body.schema_version, NEXT_SESSION_RECEIPT_SCHEMA_VERSION);
  assert.deepEqual(migrated.body.usage, { input_token: 10, output_token: 20 });
  assert.deepEqual(migrated.signed_body, body);
  assert.equal(contract.verifyReceiptEnvelope(migrated), true);

  const unsupported = await contract.normalizeReceiptEnvelope(
    { ...envelope, body: { ...body, schema_version: 99 } },
    { targetSchemaVersion: NEXT_SESSION_RECEIPT_SCHEMA_VERSION }
  );
  assert.equal(unsupported instanceof Error, true);
  assert.match(unsupported.message, /Unsupported receipt schema migration/);
});

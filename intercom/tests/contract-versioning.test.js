import assert from 'node:assert/strict';
import test from 'node:test';
import b4a from 'b4a';
import MayhemContract, {
  CONTRACT_VERSION,
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

test('contract accepts only the current consent signing version', async () => {
  assert.throws(() => consentMessage(1, rulesHash, 1), /Unsupported signing message version/);
  assert.match(consentMessage(1, rulesHash), /"signing_version":2/);

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
      sig: signConsent(user.wallet, 1, rulesHash),
    },
    user.publicKey,
    2
  );
  assert.equal(result.ok, true, result.message);
  assert.equal((await storage.get(`consent/${user.publicKey}`)).value.hash, rulesHash);
});

test('provider lifecycle features accept only the current signing version', async () => {
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
    nonce: '2'.repeat(64),
  };

  assert.throws(
    () => providerLifecycleIntentMessage(intent, 1),
    /Unsupported signing message version/
  );

  await executeFeature(
    contract,
    storage,
    'mayhem_feature',
    await providerLifecycleFeatureKey(intent),
    {
      op: 'provider_lifecycle',
      intent,
      sig: signProviderLifecycleIntent(provider.wallet, intent),
    },
    admin.publicKey
  );

  assert.equal((await storage.get(`prov/${provider.publicKey}`)).value.status, 'active');
});

test('receipt verifier accepts only the current signing payload', async () => {
  const user = await makeIdentity();
  const provider = await makeIdentity();
  const enclave = await makeIdentity();
  const contract = new MayhemContract({ peer: { wallet: makeVerifier(enclave.wallet) } }, {});
  const body = {
    schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
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
    locked_per_req_au: '0',
    locked_min_session_au: '0',
    served_ctx: 8192,
    ctx_bracket: 'le8k',
    ctx_bracket_table_ver: 1,
    rules_ver: 1,
    usage: { input_token: 10, output_token: 20 },
    au_owed_cum: '100',
    prompt_hash: 'a'.repeat(64),
    ts: 1_000,
  };

  assert.throws(() => receiptMessage(body, 1), /Unsupported signing message version/);
  const message = b4a.from(receiptMessage(body));
  assert.equal(
    contract.verifyReceiptEnvelope({
      body,
      enclave_pubkey: enclave.publicKey,
      enclave_sig: b4a.toString(enclave.wallet.sign(message), 'hex'),
      user_sig: b4a.toString(user.wallet.sign(message), 'hex'),
    }),
    true
  );
});

test('receipt normalization rejects old schemas and non-canonical usage', async () => {
  const user = await makeIdentity();
  const provider = await makeIdentity();
  const enclave = await makeIdentity();
  const contract = new MayhemContract({ peer: { wallet: makeVerifier(enclave.wallet) } }, {});
  const body = {
    schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
    session_id: 'session-schema-current',
    seq: 1,
    final: true,
    rail: 'fiat',
    user: user.publicKey,
    provider: provider.publicKey,
    enclave_id: 'enclave-schema-current',
    model_id: 'model/schema-current',
    price_ver: 1,
    locked_rate_map: versioningLockedRateMap,
    locked_per_req_au: '0',
    locked_min_session_au: '0',
    served_ctx: 8192,
    ctx_bracket: 'le8k',
    ctx_bracket_table_ver: 1,
    rules_ver: 1,
    usage: { input_token: 10, output_token: 20 },
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

  const normalized = await contract.normalizeReceiptEnvelope(envelope);
  assert.equal(normalized instanceof Error, false, normalized.message);
  assert.equal(normalized.body.schema_version, SESSION_RECEIPT_SCHEMA_VERSION);
  assert.equal(Object.hasOwn(normalized, 'signed_body'), false);
  assert.equal(contract.verifyReceiptEnvelope(normalized), true);

  const oldSchema = await contract.normalizeReceiptEnvelope({
    ...envelope,
    body: { ...body, schema_version: SESSION_RECEIPT_SCHEMA_VERSION - 1 },
  });
  assert.equal(oldSchema instanceof Error, true);
  assert.match(oldSchema.message, /Unsupported receipt schema version/);

  const aliasUsage = await contract.normalizeReceiptEnvelope({
    ...envelope,
    body: { ...body, usage: { in: 10, out_tokens: 20 } },
  });
  assert.equal(aliasUsage instanceof Error, true);
  assert.match(aliasUsage.message, /Receipt usage must be canonical/);
});

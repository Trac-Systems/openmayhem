import assert from 'node:assert/strict';
import test from 'node:test';
import b4a from 'b4a';
import MayhemContract, {
  CONTRACT_VERSION,
  SESSION_RECEIPT_SCHEMA_VERSION,
  consentMessage,
  providerLifecycleIntentMessage,
  receiptMessage,
  signingMessageVersions,
  spendVoucherMessage,
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
const versioningBillingId = 'bb'.repeat(32);

test('launch version gates cover A16/A17/D6/D7/M5/M6/M8 deterministic changes', () => {
  assert.equal(CONTRACT_VERSION, 20);
  assert.deepEqual(signingMessageVersions(), [2]);
  assert.equal(SESSION_RECEIPT_SCHEMA_VERSION, 11);
});

test('contract reports the exported contract version', async () => {
  const user = await makeIdentity();
  const storage = new MemoryStorage({ admin: user.publicKey });
  const protocol = { peer: { wallet: makeVerifier(user.wallet) } };
  const contract = new MayhemContract(protocol, {});

  const result = await contract.noop(storage);
  assert.equal(result.version, CONTRACT_VERSION);
});

test('contract recognizes canonical two-bit quant descriptors without treating Qwen2 as Q2', () => {
  const contract = new MayhemContract({}, {});
  assert.equal(contract.normalizeEnclaveQuant('int2'), 'int2');
  assert.equal(contract.normalizeEnclaveQuant('2bit'), 'int2');
  assert.equal(contract.normalizeEnclaveQuant('Q2_0'), 'int2');
  assert.equal(contract.normalizeEnclaveQuant('Q1_0'), 'int1');
  assert.equal(contract.normalizeEnclaveQuant('Q5_K_M'), 'int5');
  assert.equal(contract.normalizeEnclaveQuant('IQ3_XXS'), 'int3');
  assert.equal(contract.normalizeEnclaveQuant('MXFP4'), 'mxfp4');
  assert.equal(contract.normalizeEnclaveQuant('NF4'), 'nf4');
  assert.equal(contract.validateEnclaveQuant('BF8'), null);
  assert.match(contract.validateEnclaveQuant('potato').message, /canonical identifier/);
  assert.match(contract.validateEnclaveQuant('bad--bucket').message, /canonical identifier/);
  assert.equal(contract.quantBucketFromDescriptor('Ternary-Bonsai-27B-Q2_0.gguf'), 'int2');
  assert.notEqual(contract.quantBucketFromDescriptor('Qwen2.5-7B-Instruct.gguf'), 'int2');
  assert.notEqual(contract.quantBucketFromDescriptor('Qwen1.5-7B-Instruct.gguf'), 'int1');
});

test('contract keeps descriptive backends extensible while routing semantics stay closed', () => {
  const contract = new MayhemContract({}, {});
  assert.equal(contract.validateEnclaveBackend('llama.cpp'), null);
  assert.equal(contract.validateEnclaveBackend('cactus'), null);
  assert.equal(contract.validateEnclaveBackend('transformers-v2'), null);
  assert.match(contract.validateEnclaveBackend('Native Node').message, /canonical identifier/);
  assert.match(contract.validateEnclaveBackend('../escape').message, /canonical identifier/);

  assert.equal(contract.validateEnclaveCaps({
    chat: false,
    tools: false,
    json: false,
    vision: false,
    image: false,
    video: true,
    audio: true,
    ctx: 1024,
    ctx_max: 1024,
    output_modality: 'video',
    output_modalities: ['video', 'audio'],
    modality_set: ['text', 'video', 'audio'],
    speciality_levels: {},
  }, 'video-generation'), null);
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
    billing_id: versioningBillingId,
    billing_attempt: 0,
    billing_prior_usage: {},
    billing_prior_au_owed_cum: '0',
    billing_epoch: 1,
    reservation_id: '12'.repeat(32),
    reservation_expires_after_epoch: 25,
    reservation_receipt_grace_epochs: 6,
    payout_revision: '13'.repeat(32),
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

test('Rust atto money signing fixture matches JS canonical messages', () => {
  const lockedRateMap = [
    { unit: 'input_token', per_unit_au: '10000000', granularity: 1 },
    { unit: 'output_token', per_unit_au: '2500000000000000', granularity: 1000 },
  ];
  const voucher = {
    schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
    session_id: 'sess-au-roundtrip',
    billing_id: '44'.repeat(32),
    billing_attempt: 0,
    billing_prior_usage: {},
    billing_prior_au_owed_cum: '0',
    billing_epoch: 7,
    reservation_id: '45'.repeat(32),
    reservation_expires_after_epoch: 31,
    reservation_receipt_grace_epochs: 6,
    user: '11'.repeat(32),
    provider: '22'.repeat(32),
    payout_revision: '46'.repeat(32),
    rail: 'fiat',
    enclave_id: 'enclave-au-roundtrip',
    model_id: 'model/atto-roundtrip',
    price_ver: 9,
    locked_rate_map: lockedRateMap,
    locked_per_req_au: '1',
    locked_min_session_au: '2000000000000000000000000',
    served_ctx: 131072,
    required_modalities: ['text'],
    ctx_bracket: 'le128k',
    ctx_bracket_table_ver: 1,
    rules_ver: 7,
    max_spend_au: '2000000000000000000000001',
    checkpoint_every: { tokens: 4096, ms: 30000 },
  };
  const expectedVoucher = [
    '{"domain":"mayhem-spend-voucher","signing_version":2,"body":{',
    '"schema_version":11,"session_id":"sess-au-roundtrip","billing_id":"',
    '44'.repeat(32),
    '","billing_attempt":0,"billing_prior_usage":{},"billing_prior_au_owed_cum":"0",',
    '"billing_epoch":7,"reservation_id":"',
    '45'.repeat(32),
    '","reservation_expires_after_epoch":31,"reservation_receipt_grace_epochs":6,"user":"',
    '11'.repeat(32),
    '","provider":"',
    '22'.repeat(32),
    '","payout_revision":"',
    '46'.repeat(32),
    '",',
    '"rail":"fiat","enclave_id":"enclave-au-roundtrip",',
    '"model_id":"model/atto-roundtrip",',
    '"price_ver":9,"locked_rate_map":[',
    '{"unit":"input_token","per_unit_au":"10000000","granularity":1},',
    '{"unit":"output_token","per_unit_au":"2500000000000000","granularity":1000}',
    '],"locked_per_req_au":"1","locked_min_session_au":"2000000000000000000000000",',
    '"served_ctx":131072,"required_modalities":["text"],',
    '"ctx_bracket":"le128k","ctx_bracket_table_ver":1,',
    '"rules_ver":7,',
    '"max_spend_au":"2000000000000000000000001",',
    '"checkpoint_every":{"tokens":4096,"ms":30000}}}',
  ].join('');
  assert.equal(spendVoucherMessage(voucher), expectedVoucher);

  const receipt = {
    schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
    session_id: 'sess-au-roundtrip',
    billing_id: '44'.repeat(32),
    billing_attempt: 0,
    billing_prior_usage: {},
    billing_prior_au_owed_cum: '0',
    billing_epoch: 7,
    reservation_id: '45'.repeat(32),
    reservation_expires_after_epoch: 31,
    reservation_receipt_grace_epochs: 6,
    payout_revision: '46'.repeat(32),
    seq: 2,
    final: true,
    rail: 'fiat',
    user: '11'.repeat(32),
    provider: '22'.repeat(32),
    enclave_id: 'enclave-au-roundtrip',
    model_id: 'model/atto-roundtrip',
    price_ver: 9,
    locked_rate_map: lockedRateMap,
    locked_per_req_au: '1',
    locked_min_session_au: '2000000000000000000000000',
    served_ctx: 131072,
    ctx_bracket: 'le128k',
    ctx_bracket_table_ver: 1,
    rules_ver: 7,
    usage: { input_token: 3, output_token: 5 },
    au_owed_cum: '2000000000000000000000001',
    prompt_hash: '33'.repeat(32),
    ts: 1783517300,
  };
  const expectedReceipt = [
    '{"domain":"mayhem-session-receipt","signing_version":2,"body":{',
    '"schema_version":11,"session_id":"sess-au-roundtrip","billing_id":"',
    '44'.repeat(32),
    '","billing_attempt":0,"billing_prior_usage":{},"billing_prior_au_owed_cum":"0",',
    '"billing_epoch":7,"reservation_id":"',
    '45'.repeat(32),
    '","reservation_expires_after_epoch":31,"reservation_receipt_grace_epochs":6,"payout_revision":"',
    '46'.repeat(32),
    '",',
    '"seq":2,"final":true,',
    '"rail":"fiat","user":"',
    '11'.repeat(32),
    '","provider":"',
    '22'.repeat(32),
    '","enclave_id":"enclave-au-roundtrip","model_id":"model/atto-roundtrip",',
    '"price_ver":9,"locked_rate_map":[',
    '{"unit":"input_token","per_unit_au":"10000000","granularity":1},',
    '{"unit":"output_token","per_unit_au":"2500000000000000","granularity":1000}',
    '],"locked_per_req_au":"1","locked_min_session_au":"2000000000000000000000000",',
    '"served_ctx":131072,"ctx_bracket":"le128k","ctx_bracket_table_ver":1,',
    '"rules_ver":7,"usage":{"input_token":3,"output_token":5},',
    '"au_owed_cum":"2000000000000000000000001","prompt_hash":"',
    '33'.repeat(32),
    '","ts":1783517300}}',
  ].join('');
  assert.equal(receiptMessage(receipt), expectedReceipt);
});

test('receipt normalization rejects old schemas and non-canonical usage', async () => {
  const user = await makeIdentity();
  const provider = await makeIdentity();
  const enclave = await makeIdentity();
  const contract = new MayhemContract({ peer: { wallet: makeVerifier(enclave.wallet) } }, {});
  const body = {
    schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
    session_id: 'session-schema-current',
    billing_id: versioningBillingId,
    billing_attempt: 0,
    billing_prior_usage: {},
    billing_prior_au_owed_cum: '0',
    billing_epoch: 1,
    reservation_id: '14'.repeat(32),
    reservation_expires_after_epoch: 25,
    reservation_receipt_grace_epochs: 6,
    payout_revision: '15'.repeat(32),
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
      caps: { chat: true, ctx: 8192, modality_set: ['text'], speciality_levels: {} },
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

  const attributedBody = {
    ...body,
    usage_attribution: {
      reasoning_output_tokens: 15,
      vision_input_tokens: 4,
      audio_input_tokens: 3,
    },
  };
  const attributedMessage = b4a.from(receiptMessage(attributedBody));
  const attributedEnvelope = {
    body: attributedBody,
    enclave_pubkey: enclave.publicKey,
    enclave_sig: b4a.toString(enclave.wallet.sign(attributedMessage), 'hex'),
    user_sig: b4a.toString(user.wallet.sign(attributedMessage), 'hex'),
  };
  const attributed = await contract.normalizeReceiptEnvelope(attributedEnvelope);
  assert.equal(attributed instanceof Error, false, attributed.message);
  assert.deepEqual(attributed.body.usage_attribution, attributedBody.usage_attribution);
  assert.equal(attributed.body.au_owed_cum, body.au_owed_cum);
  assert.equal(contract.verifyReceiptEnvelope(attributed), true);
  assert.notEqual(receiptMessage(body), receiptMessage(attributedBody));
  assert.equal(contract.verifyReceiptEnvelope({ ...normalized, body: attributedBody }), false);

  const excessiveReasoning = await contract.normalizeReceiptEnvelope({
    ...attributedEnvelope,
    body: {
      ...attributedBody,
      usage_attribution: { reasoning_output_tokens: 21 },
    },
  });
  assert.equal(excessiveReasoning instanceof Error, true);
  assert.match(excessiveReasoning.message, /reasoning attribution exceeds billed output/i);

  const excessiveVision = await contract.normalizeReceiptEnvelope({
    ...attributedEnvelope,
    body: {
      ...attributedBody,
      usage_attribution: { vision_input_tokens: 11 },
    },
  });
  assert.equal(excessiveVision instanceof Error, true);
  assert.match(excessiveVision.message, /vision attribution exceeds billed input/i);

  const excessiveAudio = await contract.normalizeReceiptEnvelope({
    ...attributedEnvelope,
    body: {
      ...attributedBody,
      usage_attribution: { audio_input_tokens: 11 },
    },
  });
  assert.equal(excessiveAudio instanceof Error, true);
  assert.match(excessiveAudio.message, /audio attribution exceeds billed input/i);

  const unknownAttribution = await contract.normalizeReceiptEnvelope({
    ...attributedEnvelope,
    body: {
      ...attributedBody,
      usage_attribution: { provider_claimed_tokens: 1 },
    },
  });
  assert.equal(unknownAttribution instanceof Error, true);
  assert.match(unknownAttribution.message, /unsupported receipt usage attribution/i);
});

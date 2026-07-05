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
  textRateMap,
} from './helpers/contract.js';

const rulesHash = '8'.repeat(64);
const enclaveId = '9'.repeat(64);
const modelId = 'meta/llama-3.1-8b-instruct@4bit';
const DAY_SECONDS = 24 * 60 * 60;

const providerRegistration = {
  op: 'register_provider',
};

const enclaveRegistration = {
  op: 'register_enclave',
  enclave_id: enclaveId,
  model_id: modelId,
  backend: 'llama.cpp',
  artifact_root: 'a'.repeat(64),
  artifact_root_kind: 'blake3_merkle_v1',
  artifact_source: {
    kind: 'huggingface',
    repo: 'mayhem-test/llama-3.1-8b-instruct-GGUF',
    revision: '1'.repeat(40),
    path: 'llama-3.1-8b-instruct-Q4_K_M.gguf',
  },
  manifest_hash: 'b'.repeat(64),
  att_tier: 1,
  binary_hash: 'c'.repeat(64),
  caps: {
    chat: true,
    tools: false,
    ctx: 32768,
  },
};

const makePrice = (overrides = {}) => ({
  op: 'set_price',
  enclave_id: enclaveId,
  rate_map: textRateMap(18, 55),
  per_req_mu: 0,
  min_session_mu: 100,
  effective_at: 21_600,
  ...overrides,
});

async function setupRegisteredEnclave() {
  const provider = await makeIdentity();
  const admin = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const protocol = { peer: { wallet: makeVerifier(provider.wallet) } };
  const contract = new MayhemContract(protocol, {});

  await storage.put(`modelref/${modelId}`, {
    model_id: modelId,
    rate_map: textRateMap(20, 60),
  });

  for (const op of [
    {
      type: 'setRules',
      value: { op: 'set_rules', ver: 1, hash: rulesHash },
      sender: admin.publicKey,
      txNo: 1,
    },
    {
      type: 'consent',
      value: {
        op: 'consent',
        ver: 1,
        hash: rulesHash,
        sig: signConsent(provider.wallet, 1, rulesHash),
      },
      sender: provider.publicKey,
      txNo: 2,
    },
    {
      type: 'registerProvider',
      value: providerRegistration,
      sender: provider.publicKey,
      txNo: 3,
    },
    {
      type: 'registerEnclave',
      value: enclaveRegistration,
      sender: admin.publicKey,
      txNo: 4,
    },
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }

  return { contract, storage, provider, admin };
}

test('MayhemContract setPrice enforces modelref bounds and six-hour rate limit', async () => {
  const { contract, storage, provider, admin } = await setupRegisteredEnclave();

  const providerPrice = await execute(
    contract,
    storage,
    'setPrice',
    makePrice(),
    provider.publicKey,
    5
  );
  assert.match(providerPrice.message, /admin required/i);

  const tooLow = await execute(
    contract,
    storage,
    'setPrice',
    makePrice({ rate_map: textRateMap(4, 55) }),
    admin.publicKey,
    6
  );
  assert.match(tooLow.message, /unit input_token outside/i);

  const tooHigh = await execute(
    contract,
    storage,
    'setPrice',
    makePrice({ rate_map: textRateMap(18, 241) }),
    admin.publicKey,
    7
  );
  assert.match(tooHigh.message, /unit output_token outside/i);

  const first = await execute(
    contract,
    storage,
    'setPrice',
    makePrice(),
    admin.publicKey,
    8
  );
  assert.deepEqual(first, {
    ok: true,
    op: 'setPrice',
    enclave_id: enclaveId,
    ver: 1,
  });

  const tooSoon = await execute(
    contract,
    storage,
    'setPrice',
    makePrice({ rate_map: textRateMap(19, 55), effective_at: 21_660 }),
    admin.publicKey,
    9
  );
  assert.match(tooSoon.message, /once per 6h/i);

  const second = await execute(
    contract,
    storage,
    'setPrice',
    makePrice({ rate_map: textRateMap(19, 56), effective_at: 43_200 }),
    admin.publicKey,
    10
  );
  assert.deepEqual(second, {
    ok: true,
    op: 'setPrice',
    enclave_id: enclaveId,
    ver: 2,
  });

  const price = await storage.get(`price/${enclaveId}`);
  assert.deepEqual(price.value, {
    enclave_id: enclaveId,
    model_id: modelId,
    denom: 'mu_usd',
    current: {
      enclave_id: enclaveId,
      model_id: modelId,
      denom: 'mu_usd',
      ver: 1,
      rate_map: textRateMap(18, 55),
      per_req_mu: 0,
      min_session_mu: 100,
      effective_at: 21_600,
      effective_from: makeTxKey(8),
      updated_at: makeTxKey(8),
      set_by: admin.publicKey,
      set_by_role: 'admin',
    },
    pending: {
      enclave_id: enclaveId,
      model_id: modelId,
      denom: 'mu_usd',
      ver: 2,
      rate_map: textRateMap(19, 56),
      per_req_mu: 0,
      min_session_mu: 100,
      effective_at: 43_200,
      effective_from: makeTxKey(10),
      updated_at: makeTxKey(10),
      set_by: admin.publicKey,
      set_by_role: 'admin',
    },
  });

  const beforeSecond = await execute(
    contract,
    storage,
    'readPrice',
    { op: 'read_price', enclave_id: enclaveId, at: 43_199 },
    provider.publicKey,
    11
  );
  assert.equal(beforeSecond.price.ver, 1);

  const afterSecond = await execute(
    contract,
    storage,
    'readPrice',
    { op: 'read_price', enclave_id: enclaveId, at: 43_200 },
    provider.publicKey,
    12
  );
  assert.equal(afterSecond.price.ver, 2);
});

test('MayhemContract setModelRef is admin-only and forward-facing', async () => {
  const provider = await makeIdentity();
  const admin = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const protocol = { peer: { wallet: makeVerifier(provider.wallet) } };
  const contract = new MayhemContract(protocol, {});
  const modelRef = {
    op: 'set_model_ref',
    model_id: modelId,
    rate_map: textRateMap(20, 60),
    source_hash: 'd'.repeat(64),
  };

  const providerAttempt = await execute(
    contract,
    storage,
    'setModelRef',
    modelRef,
    provider.publicKey,
    1
  );
  assert.match(providerAttempt.message, /admin required/i);
  assert.equal(await storage.get(`modelref/${modelId}`), null);

  const seeded = await execute(
    contract,
    storage,
    'setModelRef',
    modelRef,
    admin.publicKey,
    2
  );
  assert.deepEqual(seeded, {
    ok: true,
    op: 'setModelRef',
    model_id: modelId,
    ver: 1,
  });

  const updated = await execute(
    contract,
    storage,
    'setModelRef',
    {
      ...modelRef,
      rate_map: textRateMap(21, 63),
    },
    admin.publicKey,
    3
  );
  assert.deepEqual(updated, {
    ok: true,
    op: 'setModelRef',
    model_id: modelId,
    ver: 2,
  });

  assert.deepEqual((await storage.get(`modelref/${modelId}`)).value, {
    model_id: modelId,
    model_class: 'text-generation',
    denom: 'mu_usd',
    rate_map: textRateMap(21, 63),
    ver: 2,
    source_hash: 'd'.repeat(64),
    updated_at: makeTxKey(3),
    set_by: admin.publicKey,
    set_by_role: 'admin',
  });
});

test('MayhemContract validates per-class rate maps including image prices', async () => {
  const admin = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const protocol = { peer: { wallet: makeVerifier(admin.wallet) } };
  const contract = new MayhemContract(protocol, {});
  const imageEnclave = {
    ...enclaveRegistration,
    enclave_id: 'e'.repeat(64),
    model_id: 'admin/image-small@fp16',
    model_class: 'image-generation',
  };

  let result = await execute(
    contract,
    storage,
    'registerEnclave',
    imageEnclave,
    admin.publicKey,
    1
  );
  assert.equal(result.ok, true, result.message);

  result = await execute(
    contract,
    storage,
    'setModelRef',
    {
      op: 'set_model_ref',
      model_id: imageEnclave.model_id,
      model_class: 'image-generation',
      rate_map: [{ unit: 'image', per_unit_mu: 500, granularity: 1 }],
    },
    admin.publicKey,
    2
  );
  assert.equal(result.ok, true, result.message);

  const invalidTextUnit = await execute(
    contract,
    storage,
    'setPrice',
    {
      op: 'set_price',
      enclave_id: imageEnclave.enclave_id,
      rate_map: textRateMap(20, 60),
      per_req_mu: 0,
      min_session_mu: 0,
      effective_at: 0,
    },
    admin.publicKey,
    3
  );
  assert.match(invalidTextUnit.message, /input_token is not allowed for model_class image-generation/i);

  result = await execute(
    contract,
    storage,
    'setPrice',
    {
      op: 'set_price',
      enclave_id: imageEnclave.enclave_id,
      rate_map: [{ unit: 'image', per_unit_mu: 600, granularity: 1 }],
      per_req_mu: 0,
      min_session_mu: 0,
      effective_at: 0,
    },
    admin.publicKey,
    4
  );
  assert.equal(result.ok, true, result.message);
  assert.deepEqual((await storage.get(`price/${imageEnclave.enclave_id}`)).value.current.rate_map, [
    { unit: 'image', per_unit_mu: 600, granularity: 1 },
  ]);
});

test('MayhemContract rejects unsafe enclave identifiers in price reads and writes', async () => {
  const provider = await makeIdentity();
  const admin = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const protocol = { peer: { wallet: makeVerifier(provider.wallet) } };
  const contract = new MayhemContract(protocol, {});

  const badWrite = await execute(
    contract,
    storage,
    'setPrice',
    makePrice({ enclave_id: 'bad/enclave' }),
    admin.publicKey,
    1
  );
  assert.match(badWrite.message, /invalid enclave id/i);
  assert.equal(await storage.get('price/bad/enclave'), null);

  const badRead = await execute(
    contract,
    storage,
    'readPrice',
    { op: 'read_price', enclave_id: 'bad/enclave', at: 0 },
    provider.publicKey,
    2
  );
  assert.match(badRead.message, /invalid enclave id/i);
});

test('MayhemContract contract admin can edit enclave pricing forward-facing', async () => {
  const { contract, storage, provider, admin } = await setupRegisteredEnclave();

  const initial = await execute(
    contract,
    storage,
    'setPrice',
    makePrice(),
    admin.publicKey,
    5
  );
  assert.equal(initial.ok, true, initial.message);

  const adminUpdate = await execute(
    contract,
    storage,
    'setPrice',
    makePrice({ rate_map: textRateMap(19, 56), effective_at: 43_200 }),
    admin.publicKey,
    6
  );
  assert.deepEqual(adminUpdate, {
    ok: true,
    op: 'setPrice',
    enclave_id: enclaveId,
    ver: 2,
  });

  const beforeActivation = await execute(
    contract,
    storage,
    'readPrice',
    { op: 'read_price', enclave_id: enclaveId, at: 43_199 },
    provider.publicKey,
    7
  );
  assert.equal(beforeActivation.price.ver, 1);
  assert.equal(beforeActivation.price.set_by, admin.publicKey);
  assert.equal(beforeActivation.price.set_by_role, 'admin');

  const afterActivation = await execute(
    contract,
    storage,
    'readPrice',
    { op: 'read_price', enclave_id: enclaveId, at: 43_200 },
    provider.publicKey,
    8
  );
  assert.equal(afterActivation.price.ver, 2);
  assert.equal(afterActivation.price.set_by, admin.publicKey);
  assert.equal(afterActivation.price.set_by_role, 'admin');
});

test('MayhemContract setPrice uses the active scheduled price-bound params', async () => {
  const { contract, storage, provider, admin } = await setupRegisteredEnclave();

  const scheduledBounds = await execute(
    contract,
    storage,
    'setParams',
    {
      op: 'set_params',
      submitted_at: 0,
      effective_at: DAY_SECONDS,
      values: {
        price_max_bps: 20_000,
      },
    },
    admin.publicKey,
    5
  );
  assert.equal(scheduledBounds.ok, true, scheduledBounds.message);

  const beforeActivation = await execute(
    contract,
    storage,
    'setPrice',
    makePrice({
      rate_map: textRateMap(60, 180),
      effective_at: DAY_SECONDS - 1,
    }),
    admin.publicKey,
    6
  );
  assert.deepEqual(beforeActivation, {
    ok: true,
    op: 'setPrice',
    enclave_id: enclaveId,
    ver: 1,
  });

  const afterActivation = await execute(
    contract,
    storage,
    'setPrice',
    makePrice({
      rate_map: textRateMap(60, 180),
      effective_at: DAY_SECONDS + 21_600,
    }),
    admin.publicKey,
    7
  );
  assert.match(afterActivation.message, /unit input_token outside/i);
});

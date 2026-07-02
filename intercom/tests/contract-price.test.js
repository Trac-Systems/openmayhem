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

const rulesHash = '8'.repeat(64);
const enclaveId = '9'.repeat(64);
const modelId = 'meta/llama-3.1-8b-instruct@4bit';
const DAY_SECONDS = 24 * 60 * 60;

const providerRegistration = {
  op: 'register_provider',
  payout_addr: 'trac1providerpayouttarget',
  payout_method: 'tnk',
};

const enclaveRegistration = {
  op: 'register_enclave',
  enclave_id: enclaveId,
  model_id: modelId,
  backend: 'llama.cpp',
  artifact_root: 'a'.repeat(64),
  manifest_hash: 'b'.repeat(64),
  att_tier: 1,
  binary_hash: 'c'.repeat(64),
  caps: {
    chat: true,
    tools: false,
    ctx: 32768,
  },
  rooms: [],
};

const makePrice = (overrides = {}) => ({
  op: 'set_price',
  enclave_id: enclaveId,
  in_per_1k_mu: 18,
  out_per_1k_mu: 55,
  per_req_mu: 0,
  min_session_mu: 100,
  effective_at: 21_600,
  ...overrides,
});

async function setupRegisteredEnclave() {
  const provider = await makeIdentity();
  const storage = new MemoryStorage({ admin: provider.publicKey });
  const protocol = { peer: { wallet: makeVerifier(provider.wallet) } };
  const contract = new MayhemContract(protocol, {});

  await storage.put(`modelref/${modelId}`, {
    model_id: modelId,
    price_ref_mu: {
      in_per_1k: 20,
      out_per_1k: 60,
    },
  });

  for (const op of [
    {
      type: 'setRules',
      value: { op: 'set_rules', ver: 1, hash: rulesHash },
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
      txNo: 2,
    },
    {
      type: 'registerProvider',
      value: providerRegistration,
      txNo: 3,
    },
    {
      type: 'registerEnclave',
      value: enclaveRegistration,
      txNo: 4,
    },
  ]) {
    const result = await execute(contract, storage, op.type, op.value, provider.publicKey, op.txNo);
    assert.equal(result.ok, true, result.message);
  }

  return { contract, storage, provider };
}

test('MayhemContract setPrice enforces modelref bounds and six-hour rate limit', async () => {
  const { contract, storage, provider } = await setupRegisteredEnclave();

  const tooLow = await execute(
    contract,
    storage,
    'setPrice',
    makePrice({ in_per_1k_mu: 4 }),
    provider.publicKey,
    5
  );
  assert.match(tooLow.message, /input price outside/i);

  const tooHigh = await execute(
    contract,
    storage,
    'setPrice',
    makePrice({ out_per_1k_mu: 241 }),
    provider.publicKey,
    6
  );
  assert.match(tooHigh.message, /output price outside/i);

  const first = await execute(
    contract,
    storage,
    'setPrice',
    makePrice(),
    provider.publicKey,
    7
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
    makePrice({ in_per_1k_mu: 19, effective_at: 21_660 }),
    provider.publicKey,
    8
  );
  assert.match(tooSoon.message, /once per 6h/i);

  const second = await execute(
    contract,
    storage,
    'setPrice',
    makePrice({ in_per_1k_mu: 19, out_per_1k_mu: 56, effective_at: 43_200 }),
    provider.publicKey,
    9
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
    provider: provider.publicKey,
    ver: 2,
    in_per_1k_mu: 19,
    out_per_1k_mu: 56,
    per_req_mu: 0,
    min_session_mu: 100,
    effective_at: 43_200,
    effective_from: makeTxKey(9),
    updated_at: makeTxKey(9),
  });
});

test('MayhemContract setPrice uses the active scheduled price-bound params', async () => {
  const { contract, storage, provider } = await setupRegisteredEnclave();

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
    provider.publicKey,
    5
  );
  assert.equal(scheduledBounds.ok, true, scheduledBounds.message);

  const beforeActivation = await execute(
    contract,
    storage,
    'setPrice',
    makePrice({
      in_per_1k_mu: 60,
      out_per_1k_mu: 180,
      effective_at: DAY_SECONDS - 1,
    }),
    provider.publicKey,
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
      in_per_1k_mu: 60,
      out_per_1k_mu: 180,
      effective_at: DAY_SECONDS + 21_600,
    }),
    provider.publicKey,
    7
  );
  assert.match(afterActivation.message, /input price outside/i);
});

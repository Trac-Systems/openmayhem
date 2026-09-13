import assert from 'node:assert/strict';
import test from 'node:test';
import MayhemContract from '../contract/contract.js';
import {
  MemoryStorage,
  execute,
  executeEpochApplyFeature,
  makeIdentity,
  makeTxKey,
  makeVerifier,
  seedSpendHoldsForApply,
  signConsent,
  textRateMap,
} from './helpers/contract.js';

const rulesHash = '8'.repeat(64);
const enclaveId = '9'.repeat(64);
const modelId = 'meta/llama-3.1-8b-instruct@4bit';
const DAY_SECONDS = 24 * 60 * 60;
const priceCtxBracket = 'le32k';
const priceCtxBracketTableVer = 1;
const priceKey = `price/${enclaveId}/${priceCtxBracket}`;
const priceEvidenceKey = `ev/price/1/${enclaveId}/${priceCtxBracket}`;

const providerRegistration = {
  op: 'register_provider',
};

const providerJoin = {
  op: 'join_enclave',
  enclave_id: enclaveId,
  att_tier: 1,
  attestation_head: 'd'.repeat(64),
  served_ctx: 32768,
  served_modalities: ['text'],
  served_specialities: {},
  ctx_bracket: priceCtxBracket,
  ctx_bracket_table_ver: priceCtxBracketTableVer,
};

const auString = (value) => String(value);

const seededBalance = (user, au, rail = 'fiat') => ({
  user,
  rail,
  denom: 'au_usd',
  au: auString(au),
  updated_epoch: 0,
  updated_at: null,
  ...(rail === 'tap'
    ? {
        chain_id: 61_000,
        pool_address: `0x${'2'.repeat(40)}`,
      }
    : {}),
});

const rateFor = (rateMap, unit) => rateMap.find((entry) => entry.unit === unit)?.per_unit_au;
const assertHash = (value) => assert.match(value, /^[0-9a-f]{64}$/);

const enclaveRegistration = {
  op: 'register_enclave',
  enclave_id: enclaveId,
  model_id: modelId,
  model_class: 'text-generation',
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
    modality_set: ['text'],
    speciality_levels: {},
  },
};

const makePrice = (overrides = {}) => ({
  op: 'set_price',
  enclave_id: enclaveId,
  rate_map: textRateMap(18, 55),
  per_req_au: '0',
  min_session_au: '100',
  effective_at: 21_600,
  ctx_bracket: priceCtxBracket,
  ...overrides,
});

const makeMarketUsage = (demandAu, sessionCount, overrides = {}) => ({
  enclave_id: enclaveId,
  ctx_bracket: priceCtxBracket,
  ctx_bracket_table_ver: priceCtxBracketTableVer,
  demand_au: auString(demandAu),
  session_count: sessionCount,
  provider_count: 1,
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
    model_class: 'text-generation',
    rate_map: textRateMap(20, 60),
  });
  await storage.put('params/market_provider_epoch_target_au', {
    key: 'market_provider_epoch_target_au',
    current: {
      value: '1000000',
      ver: 1,
      submitted_at: 0,
      effective_at: 0,
      set_at: null,
    },
    pending: null,
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

async function registerAndJoinExtraProvider(contract, storage, admin, provider, txStart) {
  const consent = await execute(
    contract,
    storage,
    'consent',
    {
      op: 'consent',
      ver: 1,
      hash: rulesHash,
      sig: signConsent(provider.wallet, 1, rulesHash),
    },
    provider.publicKey,
    txStart
  );
  assert.equal(consent.ok, true, consent.message);
  const registered = await execute(
    contract,
    storage,
    'registerProvider',
    providerRegistration,
    provider.publicKey,
    txStart + 1
  );
  assert.equal(registered.ok, true, registered.message);
  const joined = await execute(
    contract,
    storage,
    'joinEnclave',
    providerJoin,
    provider.publicKey,
    txStart + 2
  );
  assert.equal(joined.ok, true, joined.message);
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
  assert.match(tooSoon.message, /price_rate_limit_seconds/i);

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

  const price = await storage.get(priceKey);
  assert.deepEqual(price.value, {
    enclave_id: enclaveId,
    model_id: modelId,
    denom: 'au_usd',
    ctx_bracket: priceCtxBracket,
    ctx_bracket_table_ver: priceCtxBracketTableVer,
    current: {
      enclave_id: enclaveId,
      model_id: modelId,
      denom: 'au_usd',
      ver: 1,
      ctx_bracket: priceCtxBracket,
      ctx_bracket_table_ver: priceCtxBracketTableVer,
      rate_map: textRateMap(18, 55),
      per_req_au: '0',
      min_session_au: '100',
      effective_at: 21_600,
      effective_from: makeTxKey(8),
      updated_at: makeTxKey(8),
      set_by: admin.publicKey,
      set_by_role: 'admin',
    },
    pending: {
      enclave_id: enclaveId,
      model_id: modelId,
      denom: 'au_usd',
      ver: 2,
      ctx_bracket: priceCtxBracket,
      ctx_bracket_table_ver: priceCtxBracketTableVer,
      rate_map: textRateMap(19, 56),
      per_req_au: '0',
      min_session_au: '100',
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

test('MayhemContract bills multimodal LLM input through token rates only', async () => {
  const { contract, storage, admin } = await setupRegisteredEnclave();
  const enclaveKey = `enclave/${enclaveId}`;
  const enclave = (await storage.get(enclaveKey)).value;
  await storage.put(enclaveKey, {
    ...enclave,
    caps: {
      ...enclave.caps,
      vision: true,
      audio: true,
      video: true,
      modality_set: ['text', 'image', 'audio', 'video'],
      speciality_levels: {},
    },
  });

  const tokenOnly = await execute(
    contract,
    storage,
    'setPrice',
    makePrice(),
    admin.publicKey,
    5
  );
  assert.equal(tokenOnly.ok, true, tokenOnly.message);

  const doubleBill = await execute(
    contract,
    storage,
    'setPrice',
    makePrice({
      effective_at: 43_200,
      rate_map: [
        ...textRateMap(18, 55),
        { unit: 'image', per_unit_au: '1', granularity: 1 },
      ],
    }),
    admin.publicKey,
    6
  );
  assert.match(doubleBill.message, /unit image is not allowed for model_class text-generation/i);
});

test('MayhemContract epochApply keeps cold-start markets pinned to the admin seed', async () => {
  const { contract, storage, provider, admin } = await setupRegisteredEnclave();
  const user = await makeIdentity();

  const seeded = await execute(contract, storage, 'setPrice', makePrice(), admin.publicKey, 5);
  assert.equal(seeded.ok, true, seeded.message);
  const joined = await execute(
    contract,
    storage,
    'joinEnclave',
    providerJoin,
    provider.publicKey,
    6
  );
  assert.equal(joined.ok, true, joined.message);
  await storage.put(`bal/${user.publicKey}/fiat`, seededBalance(user.publicKey, 20_000_000));

  const applyValue = {
    op: 'epoch_apply',
    epoch: 1,
    at: 43_201,
    debits: [{ rail: 'fiat', user: user.publicKey, au: '10000000' }],
    earnings: [{ rail: 'fiat', provider: provider.publicKey, gross_au: '10000000' }],
    market_usage: [makeMarketUsage(10_000_000, 4)],
  };
  await seedSpendHoldsForApply(storage, applyValue);
  const applied = await executeEpochApplyFeature(
    contract,
    storage,
    applyValue,
    admin.publicKey
  );
  assert.equal(applied.ok, true, applied.message);
  assert.equal(applied.market_prices.length, 1);
  assert.deepEqual(
    { ...applied.market_prices[0], derivation_hash: '<hash>' },
    {
      enclave_id: enclaveId,
      ctx_bracket: priceCtxBracket,
      ctx_bracket_table_ver: priceCtxBracketTableVer,
      ver: 2,
      momentum_bps: 10_000,
      activity_rate: null,
      ema_activity_rate: null,
      active_supply: 1,
      active_demand_au: '10000000',
      frozen: true,
      derivation_hash: '<hash>',
    }
  );
  assertHash(applied.market_prices[0].derivation_hash);
  assert.equal(applied.price_root, applied.market_prices[0].derivation_hash);

  const schedule = await storage.get(priceKey);
  assert.equal(schedule.value.current.ver, 2);
  assert.equal(schedule.value.current.price_source, 'market_activity_hold');
  assert.deepEqual(schedule.value.current.rate_map, textRateMap(18, 55));
  const priceRoot = (await storage.get('ev/price/1')).value;
  assert.equal(priceRoot.merkle_root, applied.price_root);
  assert.equal(priceRoot.price_count, 1);
  const derivation = (await storage.get(priceEvidenceKey)).value;
  assert.equal(derivation.price_root, applied.price_root);
  assert.equal(derivation.controller.frozen, true);
});

test('MayhemContract epochApply counts settled-work supply, not idle joined wallets', async () => {
  const { contract, storage, provider, admin } = await setupRegisteredEnclave();
  const user = await makeIdentity();
  const idleProvider = await makeIdentity();

  const seeded = await execute(contract, storage, 'setPrice', makePrice(), admin.publicKey, 5);
  assert.equal(seeded.ok, true, seeded.message);
  const joined = await execute(
    contract,
    storage,
    'joinEnclave',
    providerJoin,
    provider.publicKey,
    6
  );
  assert.equal(joined.ok, true, joined.message);
  await registerAndJoinExtraProvider(contract, storage, admin, idleProvider, 7);
  await storage.put(`bal/${user.publicKey}/fiat`, seededBalance(user.publicKey, 5_000_000));

  const applyValue = {
    op: 'epoch_apply',
    epoch: 1,
    at: 43_201,
    debits: [{ rail: 'fiat', user: user.publicKey, au: '2000000' }],
    earnings: [{ rail: 'fiat', provider: provider.publicKey, gross_au: '2000000' }],
    market_usage: [makeMarketUsage(2_000_000, 2)],
  };
  await seedSpendHoldsForApply(storage, applyValue);
  const applied = await executeEpochApplyFeature(
    contract,
    storage,
    applyValue,
    admin.publicKey
  );
  assert.equal(applied.ok, true, applied.message);
  assert.equal(applied.market_prices[0].active_supply, 1);
  assert.equal(applied.market_prices[0].frozen, true);

  const derivation = (await storage.get(priceEvidenceKey)).value;
  assert.equal(derivation.controller.active_supply, 1);
  assert.equal(derivation.controller.frozen, true);
});



test('MayhemContract market price math supports sub-micro atto price steps', async () => {
  const { contract } = await setupRegisteredEnclave();
  const qwenEmbeddingPerTokenAu = '10000000';
  const next = contract.stepPriceTerm(qwenEmbeddingPerTokenAu, '10001000', {
    gain_bps: 5_000,
    max_step_bps: 1,
  });

  assert.equal(next, '10000500');
  assert.ok(BigInt(next) > BigInt(qwenEmbeddingPerTokenAu));
  assert.ok(BigInt(next) - BigInt(qwenEmbeddingPerTokenAu) < BigInt(qwenEmbeddingPerTokenAu) / 10_000n);
});

test('MayhemContract keeps context brackets as independent price markets', async () => {
  const { contract, storage, provider, admin } = await setupRegisteredEnclave();
  const user = await makeIdentity();
  const providerTwo = await makeIdentity();

  const shortSeed = await execute(contract, storage, 'setPrice', makePrice(), admin.publicKey, 5);
  assert.equal(shortSeed.ok, true, shortSeed.message);
  const longSeed = await execute(
    contract,
    storage,
    'setPrice',
    makePrice({
      ctx_bracket: 'le128k',
      rate_map: textRateMap(30, 90),
      min_session_au: '150',
    }),
    admin.publicKey,
    6
  );
  assert.equal(longSeed.ok, true, longSeed.message);
  const joined = await execute(
    contract,
    storage,
    'joinEnclave',
    providerJoin,
    provider.publicKey,
    7
  );
  assert.equal(joined.ok, true, joined.message);
  await registerAndJoinExtraProvider(contract, storage, admin, providerTwo, 8);
  await storage.put(`bal/${user.publicKey}/fiat`, seededBalance(user.publicKey, 10_000_000));

  const applyValue = {
    op: 'epoch_apply',
    epoch: 1,
    at: 43_201,
    debits: [{ rail: 'fiat', user: user.publicKey, au: '3000000' }],
    earnings: [{ rail: 'fiat', provider: provider.publicKey, gross_au: '3000000' }],
    market_usage: [
      makeMarketUsage(1_000_000, 2),
      makeMarketUsage(2_000_000, 3, { ctx_bracket: 'le128k' }),
    ],
  };
  await seedSpendHoldsForApply(storage, applyValue);
  const applied = await executeEpochApplyFeature(
    contract,
    storage,
    applyValue,
    admin.publicKey
  );
  assert.equal(applied.ok, true, applied.message);
  assert.equal(applied.market_prices.length, 2);
  const byBracket = new Map(applied.market_prices.map((entry) => [entry.ctx_bracket, entry]));
  assert.equal(byBracket.get(priceCtxBracket).ver, 2);
  assert.equal(byBracket.get('le128k').ver, 2);
  assert.equal(byBracket.get(priceCtxBracket).active_demand_au, '1000000');
  assert.equal(byBracket.get('le128k').active_demand_au, '2000000');
  assertHash(byBracket.get(priceCtxBracket).derivation_hash);
  assertHash(byBracket.get('le128k').derivation_hash);

  const shortSchedule = (await storage.get(priceKey)).value;
  const longSchedule = (await storage.get(`price/${enclaveId}/le128k`)).value;
  assert.equal(shortSchedule.current.ctx_bracket, priceCtxBracket);
  assert.equal(longSchedule.current.ctx_bracket, 'le128k');
  assert.notDeepEqual(shortSchedule.current.rate_map, longSchedule.current.rate_map);
  assert.equal(rateFor(shortSchedule.current.seed.rate_map, 'input_token'), '18');
  assert.equal(rateFor(longSchedule.current.seed.rate_map, 'input_token'), '30');

  const shortDerivation = (await storage.get(priceEvidenceKey)).value;
  const longDerivation = (await storage.get(`ev/price/1/${enclaveId}/le128k`)).value;
  assert.equal(shortDerivation.ctx_bracket, priceCtxBracket);
  assert.equal(longDerivation.ctx_bracket, 'le128k');
  assert.equal(shortDerivation.usage.active_demand_au, '1000000');
  assert.equal(longDerivation.usage.active_demand_au, '2000000');
  assert.equal(shortDerivation.price_root, longDerivation.price_root);
});

test('MayhemContract market price derivation uses active admin-tuned epoch params', async () => {
  const { contract, storage, provider, admin } = await setupRegisteredEnclave();
  const user = await makeIdentity();
  const providerTwo = await makeIdentity();

  const seeded = await execute(contract, storage, 'setPrice', makePrice(), admin.publicKey, 5);
  assert.equal(seeded.ok, true, seeded.message);
  const tuned = await execute(
    contract,
    storage,
    'setParams',
    {
      op: 'set_params',
      submitted_at: 0,
      effective_at: DAY_SECONDS,
      values: {
        market_gain_bps: 10_000,
        market_max_step_bps: 10_000,
      },
    },
    admin.publicKey,
    6
  );
  assert.equal(tuned.ok, true, tuned.message);
  const joined = await execute(
    contract,
    storage,
    'joinEnclave',
    providerJoin,
    provider.publicKey,
    7
  );
  assert.equal(joined.ok, true, joined.message);
  await registerAndJoinExtraProvider(contract, storage, admin, providerTwo, 8);
  await storage.put(`bal/${user.publicKey}/fiat`, seededBalance(user.publicKey, 10_000_000));

  const applyValue = {
    op: 'epoch_apply',
    epoch: 1,
    at: DAY_SECONDS + 1,
    debits: [{ rail: 'fiat', user: user.publicKey, au: '2000000' }],
    earnings: [
      { rail: 'fiat', provider: provider.publicKey, gross_au: '1000000' },
      { rail: 'fiat', provider: providerTwo.publicKey, gross_au: '1000000' },
    ],
    market_usage: [makeMarketUsage(2_000_000, 4, { provider_count: 2 })],
  };
  await seedSpendHoldsForApply(storage, applyValue);
  const applied = await executeEpochApplyFeature(
    contract,
    storage,
    applyValue,
    admin.publicKey
  );
  assert.equal(applied.ok, true, applied.message);
  const derivation = (await storage.get(priceEvidenceKey)).value;
  assert.deepEqual(derivation.controller.constants, {
    schema_version: 2,
    ema_alpha_bps: 2_500,
    gain_bps: 10_000,
    max_step_bps: 10_000,
    max_momentum_bps: 50_000,
  });
  assert.equal(derivation.controller.momentum_bps, 10_000);
  assert.equal(derivation.controller.frozen_reason, 'missing_canonical_activity');
});



test('MayhemContract keeps one enclave price while conserving mixed rail settlement', async () => {
  const { contract, storage, provider, admin } = await setupRegisteredEnclave();
  const fiatUser = await makeIdentity();
  const tapUser = await makeIdentity();
  const tapProvider = await makeIdentity();

  const seeded = await execute(contract, storage, 'setPrice', makePrice(), admin.publicKey, 5);
  assert.equal(seeded.ok, true, seeded.message);
  const fiatJoined = await execute(
    contract,
    storage,
    'joinEnclave',
    providerJoin,
    provider.publicKey,
    6
  );
  assert.equal(fiatJoined.ok, true, fiatJoined.message);
  await registerAndJoinExtraProvider(contract, storage, admin, tapProvider, 7);
  const tapRails = await execute(
    contract,
    storage,
    'setProviderRails',
    { op: 'set_provider_rails', rails: ['tap'] },
    tapProvider.publicKey,
    10
  );
  assert.equal(tapRails.ok, true, tapRails.message);
  const payments = await execute(
    contract,
    storage,
    'setPayments',
    {
      op: 'set_payments',
      ver: 1,
      fiat: {
        processor: 'stripe',
        integration_currency: 'usd',
        adaptive_pricing: true,
        payout_currencies: ['eur', 'gbp', 'usd'],
        locale: 'en',
      },
      tap: {
        chain_id: 61_000,
        token_address: `0x${'1'.repeat(40)}`,
        pool_address: `0x${'2'.repeat(40)}`,
      },
      tnk: {
        network: 'testnet1',
        treasury_address: `testtrac1${'1'.repeat(40)}`,
      },
    },
    admin.publicKey,
    11
  );
  assert.equal(payments.ok, true, payments.message);

  await storage.put(`bal/${fiatUser.publicKey}/fiat`, seededBalance(fiatUser.publicKey, 2_000_000, 'fiat'));
  await storage.put(`bal/${tapUser.publicKey}/tap`, seededBalance(tapUser.publicKey, 2_000_000, 'tap'));

  const applyValue = {
    op: 'epoch_apply',
    epoch: 1,
    at: 43_201,
    debits: [
      { rail: 'fiat', user: fiatUser.publicKey, au: '500000' },
      { rail: 'tap', user: tapUser.publicKey, au: '500000' },
    ],
    earnings: [
      { rail: 'fiat', provider: provider.publicKey, gross_au: '500000' },
      { rail: 'tap', provider: tapProvider.publicKey, gross_au: '500000' },
    ],
    market_usage: [makeMarketUsage(1_000_000, 2, { provider_count: 2 })],
  };
  await seedSpendHoldsForApply(storage, applyValue);
  const applied = await executeEpochApplyFeature(
    contract,
    storage,
    applyValue,
    admin.publicKey
  );
  assert.equal(applied.ok, true, applied.message);
  assert.deepEqual(applied.rails, ['fiat', 'tap']);
  assert.deepEqual(applied.market_prices, [
    {
      enclave_id: enclaveId,
      ctx_bracket: priceCtxBracket,
      ctx_bracket_table_ver: priceCtxBracketTableVer,
      ver: 2,
      momentum_bps: 10_000,
      activity_rate: null,
      ema_activity_rate: null,
      active_supply: 2,
      active_demand_au: '1000000',
      frozen: true,
      derivation_hash: applied.market_prices[0].derivation_hash,
    },
  ]);
  assertHash(applied.market_prices[0].derivation_hash);

  const schedule = await storage.get(priceKey);
  assert.equal(schedule.value.current.ver, 2);
  assert.equal(schedule.value.current.price_source, 'market_activity_hold');
  assert.equal(await storage.get(`price/${enclaveId}/fiat`), null);
  assert.equal(await storage.get(`price/${enclaveId}/tap`), null);
  assert.equal(await storage.get(`price/${enclaveId}`), null);
  assert.equal((await storage.get(`bal/${fiatUser.publicKey}/fiat`)).value.au, '1500000');
  assert.equal((await storage.get(`bal/${tapUser.publicKey}/tap`)).value.au, '1500000');
  assert.equal((await storage.get(`earn/fiat/${provider.publicKey}`)).value.total_au, '425000');
  assert.equal((await storage.get(`earn/tap/${tapProvider.publicKey}`)).value.total_au, '375000');
  assert.equal((await storage.get('fee/fiat/cum')).value.cum_au, '75000');
  assert.equal((await storage.get('fee/tap/cum')).value.cum_au, '75000');
  assert.equal((await storage.get('burn/fiat/cum')).value.cum_au, '0');
  assert.equal((await storage.get('burn/tap/cum')).value.cum_au, '50000');

  const crossRailMismatchValue = {
    op: 'epoch_apply',
    epoch: 2,
    at: 46_801,
    debits: [{ rail: 'fiat', user: fiatUser.publicKey, au: '100' }],
    earnings: [{ rail: 'tap', provider: tapProvider.publicKey, gross_au: '100' }],
    market_usage: [makeMarketUsage(100, 1)],
  };
  await seedSpendHoldsForApply(storage, crossRailMismatchValue);
  const crossRailMismatch = await executeEpochApplyFeature(
    contract,
    storage,
    crossRailMismatchValue,
    admin.publicKey
  );
  assert.match(crossRailMismatch.message, /per rail/i);
});

test('MayhemContract epochApply rejects market usage that does not reconcile to settled gross', async () => {
  const { contract, storage, provider, admin } = await setupRegisteredEnclave();
  const user = await makeIdentity();

  const seeded = await execute(contract, storage, 'setPrice', makePrice(), admin.publicKey, 5);
  assert.equal(seeded.ok, true, seeded.message);
  await storage.put(`bal/${user.publicKey}/fiat`, seededBalance(user.publicKey, 10_000));
  const mismatchValue = {
    op: 'epoch_apply',
    epoch: 1,
    at: 43_201,
    debits: [{ rail: 'fiat', user: user.publicKey, au: '2000' }],
    earnings: [{ rail: 'fiat', provider: provider.publicKey, gross_au: '2000' }],
    market_usage: [makeMarketUsage(1_999, 1)],
  };
  await seedSpendHoldsForApply(storage, mismatchValue);
  const mismatch = await executeEpochApplyFeature(
    contract,
    storage,
    mismatchValue,
    admin.publicKey
  );
  assert.match(mismatch.message, /market usage demand must equal/i);
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
    model_class: 'text-generation',
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
    denom: 'au_usd',
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
    caps: {
      image: true,
      output_modality: 'image',
      output_modalities: ['image'],
      modality_set: ['image'],
      speciality_levels: {},
    },
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
      rate_map: [
        { unit: 'image', per_unit_au: '500', granularity: 1 },
        { unit: 'step', per_unit_au: '2', granularity: 1 },
      ],
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
      per_req_au: '0',
      min_session_au: '0',
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
      rate_map: [
        { unit: 'image', per_unit_au: '600', granularity: 1 },
        { unit: 'step', per_unit_au: '2', granularity: 1 },
      ],
      per_req_au: '0',
      min_session_au: '0',
      effective_at: 0,
    },
    admin.publicKey,
    4
  );
  assert.equal(result.ok, true, result.message);
  assert.deepEqual((await storage.get(`price/${imageEnclave.enclave_id}`)).value.current.rate_map, [
    { unit: 'image', per_unit_au: '600', granularity: 1 },
    { unit: 'step', per_unit_au: '2', granularity: 1 },
  ]);
});

test('MayhemContract validates workflow outcome-class governed rate units', async () => {
  const admin = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const protocol = { peer: { wallet: makeVerifier(admin.wallet) } };
  const contract = new MayhemContract(protocol, {});
  const workflowEnclave = {
    ...enclaveRegistration,
    enclave_id: 'f'.repeat(64),
    model_id: 'image.light.le1_2mp',
    model_class: 'workflow',
    caps: {
      image: true,
      output_modality: 'image',
      output_modalities: ['image'],
      modality_set: ['image'],
      speciality_levels: {},
    },
  };

  let result = await execute(
    contract,
    storage,
    'registerEnclave',
    workflowEnclave,
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
      model_id: workflowEnclave.model_id,
      model_class: 'workflow',
      rate_map: [
        { unit: 'megapixel_step', per_unit_au: '180000000000000', granularity: 1000 },
      ],
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
      enclave_id: workflowEnclave.enclave_id,
      rate_map: textRateMap(20, 60),
      per_req_au: '0',
      min_session_au: '0',
      effective_at: 0,
    },
    admin.publicKey,
    3
  );
  assert.match(invalidTextUnit.message, /input_token is not allowed for model_class workflow/i);

  result = await execute(
    contract,
    storage,
    'setPrice',
    {
      op: 'set_price',
      enclave_id: workflowEnclave.enclave_id,
      rate_map: [
        { unit: 'megapixel_step', per_unit_au: '200000000000000', granularity: 1000 },
      ],
      per_req_au: '0',
      min_session_au: '0',
      effective_at: 0,
    },
    admin.publicKey,
    4
  );
  assert.equal(result.ok, true, result.message);
});

test('MayhemContract prices video workflow classes in exact pixel-frames', async () => {
  const admin = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const protocol = { peer: { wallet: makeVerifier(admin.wallet) } };
  const contract = new MayhemContract(protocol, {});
  const videoEnclave = {
    ...enclaveRegistration,
    enclave_id: 'a'.repeat(64),
    model_id: 'video.minimax_h3.lowvram_t2v_i2v',
    model_class: 'workflow',
    caps: {
      video: true,
      audio: true,
      output_modality: 'video',
      output_modalities: ['video', 'audio'],
      modality_set: ['audio', 'video'],
      speciality_levels: {},
    },
  };
  // The published H3 low-VRAM tariff: a fixed per-request component plus a rate
  // per megapixel-frame, anchored so 736x1280 for five seconds prices at $0.125.
  const pixelFrameRateMap = [
    { unit: 'pixel_frame', per_unit_au: '1067672950634057', granularity: 1000000 },
  ];
  const perReqAu = '4300000000000000';

  let result = await execute(
    contract,
    storage,
    'registerEnclave',
    videoEnclave,
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
      model_id: videoEnclave.model_id,
      model_class: 'workflow',
      rate_map: pixelFrameRateMap,
    },
    admin.publicKey,
    2
  );
  assert.equal(result.ok, true, result.message);

  result = await execute(
    contract,
    storage,
    'setPrice',
    {
      op: 'set_price',
      enclave_id: videoEnclave.enclave_id,
      rate_map: pixelFrameRateMap,
      per_req_au: perReqAu,
      min_session_au: '0',
      effective_at: 0,
    },
    admin.publicKey,
    3
  );
  assert.equal(result.ok, true, result.message);

  // 544x960 for 120 frames is 62,668,800 pixel-frames; 736x1280 is 113,049,600.
  // Under megapixel_step both billed 120 units and cost the same.
  assert.equal(
    contract.usageAuForLockedTerms(pixelFrameRateMap, perReqAu, '0', {
      pixel_frame: 62668800,
    }),
    '71209782608695592'
  );
  assert.equal(
    contract.usageAuForLockedTerms(pixelFrameRateMap, perReqAu, '0', {
      pixel_frame: 113049600,
    }),
    '124999999999999891'
  );

  const imageUnit = contract.validateRateMap(
    [{ unit: 'input_token', per_unit_au: '1', granularity: 1 }],
    'workflow',
    'set_price rate_map'
  );
  assert.match(imageUnit.message, /input_token is not allowed for model_class workflow/i);
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

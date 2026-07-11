import assert from 'node:assert/strict';
import test from 'node:test';
import MayhemContract from '../contract/contract.js';
import {
  MemoryStorage,
  execute,
  executeEpochApplyFeature,
  executeFeature,
  executeRateFeature,
  executeTnkSettlementFeature,
  makeIdentity,
  makeVerifier,
  seedSpendHoldsForApply,
  signConsent,
  textRateMap,
  tnkSettlementFeatureKey,
} from './helpers/contract.js';

const rulesHash = 'a'.repeat(64);
const rate = {
  op: 'rate_oracle',
  tnk_usd_au: '50000000000000000',
  source: 'gate-spot',
  ts: 90_000,
};

const tnkE18 = (value) => String(value);
const cleanExitEnclaveId = '3'.repeat(64);
const cleanExitEnclave = {
  op: 'register_enclave',
  enclave_id: cleanExitEnclaveId,
  model_id: 'mayhem/clean-exit-tnk@q4',
  model_class: 'text-generation',
  backend: 'llama.cpp',
  artifact_root: '4'.repeat(64),
  artifact_root_kind: 'blake3_merkle_v1',
  artifact_source: {
    kind: 'huggingface',
    repo: 'mayhem-test/clean-exit-tnk',
    revision: '5'.repeat(40),
    path: 'clean-exit-tnk.gguf',
  },
  manifest_hash: '6'.repeat(64),
  att_tier: 1,
  quant: 'INT4',
  binary_hash: '7'.repeat(64),
  caps: {
    chat: true,
    embeddings: false,
    tools: false,
    ctx: 8192,
    modality_set: ['text'],
  },
};
const cleanExitModelRef = {
  op: 'set_model_ref',
  model_id: cleanExitEnclave.model_id,
  model_class: 'text-generation',
  rate_map: textRateMap(20, 60),
};
const cleanExitPrice = {
  op: 'set_price',
  enclave_id: cleanExitEnclaveId,
  rate_map: textRateMap(20, 60),
  per_req_au: '0',
  min_session_au: '0',
  effective_at: 0,
};

async function setupTnkSettlementContract(paramOverrides = {}) {
  const admin = await makeIdentity();
  const providers = [
    { identity: await makeIdentity(), target: 'testtrac1providerone', gross_au: '2000000' },
    { identity: await makeIdentity(), target: 'testtrac1providertwo', gross_au: '1000000' },
  ];
  const user = await makeIdentity();
  const outsider = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const contract = new MayhemContract(
    { peer: { wallet: makeVerifier(providers[0].identity.wallet) } },
    {}
  );

  const bootstrap = [
    {
      type: 'setRules',
      value: { op: 'set_rules', ver: 1, hash: rulesHash },
      sender: admin.publicKey,
      txNo: 1,
    },
    {
      type: 'setParams',
      value: {
        op: 'set_params',
        submitted_at: 0,
        effective_at: 86_400,
        values: {
          holdback_epochs: 0,
          new_provider_holdback_epochs: 0,
          challenge_epochs: 0,
          ...paramOverrides,
        },
      },
      sender: admin.publicKey,
      txNo: 2,
    },
  ];
  for (const op of bootstrap) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }

  let txNo = 3;
  for (const provider of providers) {
    for (const op of [
      {
        type: 'consent',
        value: {
          op: 'consent',
          ver: 1,
          hash: rulesHash,
          sig: signConsent(provider.identity.wallet, 1, rulesHash),
        },
        sender: provider.identity.publicKey,
      },
      {
        type: 'registerProvider',
        value: { op: 'register_provider' },
        sender: provider.identity.publicKey,
      },
      {
        type: 'setProviderRails',
        value: { op: 'set_provider_rails', rails: ['tnk'] },
        sender: provider.identity.publicKey,
      },
      {
        type: 'setProviderPayout',
        value: {
          op: 'set_provider_payout',
          provider: provider.identity.publicKey,
          payout_addr: provider.target,
          payout_method: 'tnk',
        },
        sender: admin.publicKey,
      },
    ]) {
      const result = await execute(contract, storage, op.type, op.value, op.sender, txNo++);
      assert.equal(result.ok, true, result.message);
    }
  }

  await storage.put(`bal/${user.publicKey}/tnk`, {
    user: user.publicKey,
    rail: 'tnk',
    denom: 'au_usd',
    au: '3000000',
    updated_epoch: 0,
    updated_at: null,
  });
  const rateResult = await executeRateFeature(contract, storage, rate, admin.publicKey);
  assert.equal(rateResult.ok, true, rateResult.message);

  const applyValue = {
    op: 'epoch_apply',
    epoch: 1,
    at: 90_000,
    debits: [{ rail: 'tnk', user: user.publicKey, au: '3000000' }],
    earnings: providers.map((provider) => ({
      rail: 'tnk',
      provider: provider.identity.publicKey,
      gross_au: provider.gross_au,
    })),
  };
  await seedSpendHoldsForApply(storage, applyValue);
  const applied = await executeEpochApplyFeature(contract, storage, applyValue, admin.publicKey);
  assert.deepEqual(applied, {
    ok: true,
    op: 'epochApply',
    epoch: 1,
    idempotent: false,
    debited_au: '3000000',
    earned_au: '2550000',
    fee_au: '450000',
    burn_au: '0',
    rails: ['tnk'],
  });
  const applyState = (await storage.get('epoch/apply/state')).value;
  return { admin, providers, user, outsider, storage, contract, applyState };
}

async function settlementValue(ctx, overrides = {}) {
  const providerOutputs = ctx.providers
    .map((provider) => ({
      role: 'provider',
      provider: provider.identity.publicKey,
      to: provider.target,
      au: provider.gross_au === '2000000' ? '1700000' : '850000',
      tnk_e18: provider.gross_au === '2000000' ? tnkE18(34_000_000) : tnkE18(17_000_000),
    }))
    .sort((left, right) => left.provider.localeCompare(right.provider));
  const outputs = [
    ...providerOutputs,
    {
      role: 'operator_fee',
      to: 'testtrac1operator',
      au: '450000',
      tnk_e18: tnkE18(9_000_000),
    },
  ];
  const transferRoot = await ctx.contract.tnkSettlementTransferRoot(outputs);
  return {
    op: 'tnk_settlement',
    epoch: 1,
    at: 90_000,
    rail: 'tnk',
    network: 'testnet1',
    treasury_from: 'testtrac1treasury',
    operator_to: 'testtrac1operator',
    epoch_apply_hash: ctx.applyState.last_apply_hash,
    rate_tnk_usd_au: rate.tnk_usd_au,
    rate_source: rate.source,
    rate_ts: rate.ts,
    msb_tx_hashes: ["b".repeat(64), "c".repeat(64), "d".repeat(64)],
    transfer_root: transferRoot,
    provider_count: 2,
    provider_au: '2550000',
    operator_fee_au: '450000',
    gross_au: '3000000',
    tnk_e18: tnkE18(60_000_000),
    outputs,
    ...overrides,
  };
}

test('MayhemContract tnkSettlement records official per-output transfers and is idempotent', async () => {
  const ctx = await setupTnkSettlementContract();
  const settlement = await settlementValue(ctx);

  ctx.contract._mayhemLastFeatureResult = undefined;
  const placeholderResult = await executeFeature(
    ctx.contract,
    ctx.storage,
    'mayhem_feature',
    `settle/tnk/1/${'0'.repeat(64)}`,
    { ...settlement, msb_tx_hashes: ['not-a-real-msb-tx-hash'] },
    ctx.admin.publicKey
  );
  const placeholder = placeholderResult ?? ctx.contract._mayhemLastFeatureResult;
  assert.match(placeholder.message, /real 64-hex MSB tx hash/i);

  ctx.contract._mayhemLastFeatureResult = undefined;
  const nonAdminResult = await executeFeature(
    ctx.contract,
    ctx.storage,
    'mayhem_feature',
    await tnkSettlementFeatureKey(ctx.contract, settlement),
    settlement,
    ctx.outsider.publicKey
  );
  const nonAdmin = nonAdminResult ?? ctx.contract._mayhemLastFeatureResult;
  assert.match(nonAdmin.message, /admin required/i);

  const settled = await executeTnkSettlementFeature(
    ctx.contract,
    ctx.storage,
    settlement,
    ctx.admin.publicKey
  );
  assert.deepEqual(settled, {
    ok: true,
    op: 'tnkSettlement',
    epoch: 1,
    rail: 'tnk',
    idempotent: false,
    provider_au: '2550000',
    operator_fee_au: '450000',
    gross_au: '3000000',
    msb_tx_hashes: ["b".repeat(64), "c".repeat(64), "d".repeat(64)],
    transfer_root: settlement.transfer_root,
  });

  for (const provider of ctx.providers) {
    const earning = (await ctx.storage.get(`earn/tnk/${provider.identity.publicKey}`)).value;
    const outputIndex = settlement.outputs.findIndex((output) => (
      output.role === 'provider' && output.provider === provider.identity.publicKey
    ));
    assert.notEqual(outputIndex, -1);
    assert.equal(earning.held_au, '0');
    assert.deepEqual(earning.holdbacks, []);
    assert.equal(
      earning.paid_cum_au,
      provider.gross_au === '2000000' ? '1700000' : '850000'
    );
    assert.equal(earning.last_settlement_msb_tx_hash, settlement.msb_tx_hashes[outputIndex]);
  }
  const fee = (await ctx.storage.get('fee/tnk/cum')).value;
  const operatorOutputIndex = settlement.outputs.findIndex((output) => output.role === 'operator_fee');
  assert.notEqual(operatorOutputIndex, -1);
  assert.equal(fee.cum_au, '450000');
  assert.equal(fee.swept_cum_au, '450000');
  assert.equal(fee.last_settlement_msb_tx_hash, settlement.msb_tx_hashes[operatorOutputIndex]);
  const record = (await ctx.storage.get('settle/tnk/1')).value;
  assert.equal(record.idempotency_key, `mayhem:tnk:settle:v1:testnet1:mayhem:1:${ctx.applyState.last_apply_hash}`);
  assert.deepEqual(record.msb_tx_hashes, settlement.msb_tx_hashes);
  assert.deepEqual(record.outputs, settlement.outputs);

  const snapshot = ctx.storage.snapshotBytes();
  const replay = await executeTnkSettlementFeature(
    ctx.contract,
    ctx.storage,
    settlement,
    ctx.admin.publicKey
  );
  assert.deepEqual(replay, {
    ok: true,
    op: 'tnkSettlement',
    epoch: 1,
    rail: 'tnk',
    idempotent: true,
    msb_tx_hashes: ["b".repeat(64), "c".repeat(64), "d".repeat(64)],
  });
  assert.equal(ctx.storage.snapshotBytes(), snapshot);

  const changed = await executeTnkSettlementFeature(
    ctx.contract,
    ctx.storage,
    { ...settlement, msb_tx_hashes: ["c".repeat(64), "d".repeat(64), "e".repeat(64)] },
    ctx.admin.publicKey
  );
  assert.match(changed.message, /already exists/i);
  assert.equal(ctx.storage.snapshotBytes(), snapshot);
});

test('MayhemContract tnkSettlement releases clean-exit provider earnings after holdback maturity', async () => {
  const ctx = await setupTnkSettlementContract({
    holdback_epochs: 2,
    new_provider_holdback_epochs: 2,
  });
  const provider = ctx.providers[0].identity;

  const registeredEnclave = await execute(
    ctx.contract,
    ctx.storage,
    'registerEnclave',
    cleanExitEnclave,
    ctx.admin.publicKey,
    20
  );
  assert.equal(registeredEnclave.ok, true, registeredEnclave.message);
  const modelRef = await execute(
    ctx.contract,
    ctx.storage,
    'setModelRef',
    cleanExitModelRef,
    ctx.admin.publicKey,
    21
  );
  assert.equal(modelRef.ok, true, modelRef.message);
  const price = await execute(
    ctx.contract,
    ctx.storage,
    'setPrice',
    cleanExitPrice,
    ctx.admin.publicKey,
    22
  );
  assert.equal(price.ok, true, price.message);
  const joinedEnclave = await execute(
    ctx.contract,
    ctx.storage,
    'joinEnclave',
    {
      op: 'join_enclave',
      enclave_id: cleanExitEnclaveId,
      served_ctx: 8192,
      served_modalities: ['text'],
      ctx_bracket: 'le8k',
      ctx_bracket_table_ver: 1,
    },
    provider.publicKey,
    23
  );
  assert.equal(joinedEnclave.ok, true, joinedEnclave.message);
  const leftEnclave = await execute(
    ctx.contract,
    ctx.storage,
    'leaveEnclave',
    { op: 'leave_enclave', enclave_id: cleanExitEnclaveId },
    provider.publicKey,
    24
  );
  assert.equal(leftEnclave.ok, true, leftEnclave.message);
  assert.equal((await ctx.storage.get(`prov/${provider.publicKey}`)).value.status, 'active');
  assert.deepEqual((await ctx.storage.get(`prov/${provider.publicKey}`)).value.enclaves, []);
  assert.equal(
    (await ctx.storage.get(`serve/${provider.publicKey}/${cleanExitEnclaveId}`)).value.status,
    'inactive'
  );

  const earlySettlement = await executeTnkSettlementFeature(
    ctx.contract,
    ctx.storage,
    await settlementValue(ctx, { msb_tx_hashes: ["c".repeat(64), "d".repeat(64), "e".repeat(64)] }),
    ctx.admin.publicKey
  );
  assert.match(earlySettlement.message, /no payable earnings/i);
  assert.equal(await ctx.storage.get('settle/tnk/1'), null);

  for (const epoch of [2, 3]) {
    const applied = await executeEpochApplyFeature(
      ctx.contract,
      ctx.storage,
      {
        op: 'epoch_apply',
        epoch,
        at: 90_000 + epoch,
        debits: [],
        earnings: [],
      },
      ctx.admin.publicKey
    );
    assert.equal(applied.ok, true, applied.message);
    assert.equal(applied.debited_au, '0');
    assert.equal(applied.earned_au, '0');
    assert.equal(applied.fee_au, '0');
  }
  ctx.applyState = (await ctx.storage.get('epoch/apply/state')).value;
  const maturedSettlement = await settlementValue(ctx, {
    epoch: 3,
    at: 92_000,
    msb_tx_hashes: ["d".repeat(64), "e".repeat(64), "f".repeat(64)],
  });
  const settled = await executeTnkSettlementFeature(
    ctx.contract,
    ctx.storage,
    maturedSettlement,
    ctx.admin.publicKey
  );
  assert.equal(settled.ok, true, settled.message);
  assert.equal(settled.epoch, 3);

  for (const providerRecord of ctx.providers) {
    const earning = (await ctx.storage.get(`earn/tnk/${providerRecord.identity.publicKey}`)).value;
    const outputIndex = maturedSettlement.outputs.findIndex((output) => (
      output.role === 'provider' && output.provider === providerRecord.identity.publicKey
    ));
    assert.notEqual(outputIndex, -1);
    assert.equal(earning.held_au, '0');
    assert.deepEqual(earning.holdbacks, []);
    assert.equal(
      earning.paid_cum_au,
      providerRecord.gross_au === '2000000' ? '1700000' : '850000'
    );
    assert.equal(earning.last_settlement_epoch, 3);
    assert.equal(earning.last_settlement_msb_tx_hash, maturedSettlement.msb_tx_hashes[outputIndex]);
  }
});

test('MayhemContract tnkSettlement pays banned providers released non-slashed TNK earnings', async () => {
  const ctx = await setupTnkSettlementContract();
  const bannedProvider = ctx.providers[0];
  const banned = await execute(
    ctx.contract,
    ctx.storage,
    'banProvider',
    {
      op: 'ban_provider',
      provider: bannedProvider.identity.publicKey,
      reason_hash: '9'.repeat(64),
    },
    ctx.admin.publicKey,
    20
  );
  assert.equal(banned.ok, true, banned.message);
  assert.equal((await ctx.storage.get(`prov/${bannedProvider.identity.publicKey}`)).value.status, 'banned');

  const settlement = await settlementValue(ctx, {
    msb_tx_hashes: ["e".repeat(64), "f".repeat(64), "1".repeat(64)],
  });
  const settled = await executeTnkSettlementFeature(
    ctx.contract,
    ctx.storage,
    settlement,
    ctx.admin.publicKey
  );
  assert.equal(settled.ok, true, settled.message);

  const earning = (await ctx.storage.get(`earn/tnk/${bannedProvider.identity.publicKey}`)).value;
  const outputIndex = settlement.outputs.findIndex((output) => (
    output.role === 'provider' && output.provider === bannedProvider.identity.publicKey
  ));
  assert.notEqual(outputIndex, -1);
  assert.equal(earning.paid_cum_au, '1700000');
  assert.equal(earning.last_settlement_msb_tx_hash, settlement.msb_tx_hashes[outputIndex]);
});

test('MayhemContract tnkSettlement uses the active admin max_tnk_settlement_outputs param', async () => {
  const ctx = await setupTnkSettlementContract({ max_tnk_settlement_outputs: 2 });
  const settlement = await settlementValue(ctx);

  const tooManyOutputs = await executeTnkSettlementFeature(
    ctx.contract,
    ctx.storage,
    settlement,
    ctx.admin.publicKey
  );
  assert.match(tooManyOutputs.message, /max_tnk_settlement_outputs/i);
  assert.equal(await ctx.storage.get('settle/tnk/1'), null);
});

test('MayhemContract tnkSettlement has no hidden output cap below the active admin param', async () => {
  const ctx = await setupTnkSettlementContract({ max_tnk_settlement_outputs: 5_001 });
  const settlement = await settlementValue(ctx);
  const template = settlement.outputs.find((output) => output.role === 'provider');
  const outputs = Array.from({ length: 5_001 }, (_, i) => ({
    ...template,
    provider: i.toString(16).padStart(64, '0'),
  }));

  const result = await executeTnkSettlementFeature(
    ctx.contract,
    ctx.storage,
    {
      ...settlement,
      outputs,
      msb_tx_hashes: outputs.map((_, i) => (i + 1).toString(16).padStart(64, '0')),
    },
    ctx.admin.publicKey
  );
  assert.doesNotMatch(result.message, /max_tnk_settlement_outputs|exceeds limit/i);
  assert.match(result.message, /provider count does not match outputs/i);
  assert.equal(await ctx.storage.get('settle/tnk/1'), null);
});

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
  signConsent,
  tnkSettlementFeatureKey,
} from './helpers/contract.js';

const rulesHash = 'a'.repeat(64);
const rate = {
  op: 'rate_oracle',
  tnk_usd_e6: 50_000,
  source: 'gate-spot',
  ts: 90_000,
};

const tnkE18 = (whole) => `${whole}000000000000000000`;

async function setupTnkSettlementContract(paramOverrides = {}) {
  const admin = await makeIdentity();
  const providers = [
    { identity: await makeIdentity(), target: 'testtrac1providerone', gross_mu: 2_000_000 },
    { identity: await makeIdentity(), target: 'testtrac1providertwo', gross_mu: 1_000_000 },
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
        values: { holdback_epochs: 0, challenge_epochs: 0, ...paramOverrides },
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
    denom: 'mu_usd',
    mu: 3_000_000,
    updated_epoch: 0,
    updated_at: null,
  });
  const rateResult = await executeRateFeature(contract, storage, rate, admin.publicKey);
  assert.equal(rateResult.ok, true, rateResult.message);

  const applyValue = {
    op: 'epoch_apply',
    epoch: 1,
    at: 90_000,
    debits: [{ rail: 'tnk', user: user.publicKey, mu: 3_000_000 }],
    earnings: providers.map((provider) => ({
      rail: 'tnk',
      provider: provider.identity.publicKey,
      gross_mu: provider.gross_mu,
    })),
  };
  const applied = await executeEpochApplyFeature(contract, storage, applyValue, admin.publicKey);
  assert.deepEqual(applied, {
    ok: true,
    op: 'epochApply',
    epoch: 1,
    idempotent: false,
    debited_mu: 3_000_000,
    earned_mu: 2_550_000,
    fee_mu: 450_000,
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
      mu: provider.gross_mu === 2_000_000 ? 1_700_000 : 850_000,
      tnk_e18: provider.gross_mu === 2_000_000 ? tnkE18(34) : tnkE18(17),
    }))
    .sort((left, right) => left.provider.localeCompare(right.provider));
  const outputs = [
    ...providerOutputs,
    {
      role: 'operator_fee',
      to: 'testtrac1operator',
      mu: 450_000,
      tnk_e18: tnkE18(9),
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
    rate_tnk_usd_e6: rate.tnk_usd_e6,
    rate_source: rate.source,
    rate_ts: rate.ts,
    msb_tx_hash: 'b'.repeat(64),
    transfer_root: transferRoot,
    provider_count: 2,
    provider_mu: 2_550_000,
    operator_fee_mu: 450_000,
    gross_mu: 3_000_000,
    tnk_e18: tnkE18(60),
    outputs,
    ...overrides,
  };
}

test('MayhemContract tnkSettlement records one real batch and is idempotent', async () => {
  const ctx = await setupTnkSettlementContract();
  const settlement = await settlementValue(ctx);

  ctx.contract._mayhemLastFeatureResult = undefined;
  const placeholderResult = await executeFeature(
    ctx.contract,
    ctx.storage,
    'mayhem_feature',
    `settle/tnk/1/${'0'.repeat(64)}`,
    { ...settlement, msb_tx_hash: 'not-a-real-msb-tx-hash' },
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
    provider_mu: 2_550_000,
    operator_fee_mu: 450_000,
    gross_mu: 3_000_000,
    msb_tx_hash: 'b'.repeat(64),
    transfer_root: settlement.transfer_root,
  });

  for (const provider of ctx.providers) {
    const earning = (await ctx.storage.get(`earn/tnk/${provider.identity.publicKey}`)).value;
    assert.equal(earning.held_mu, 0);
    assert.deepEqual(earning.holdbacks, []);
    assert.equal(
      earning.paid_cum_mu,
      provider.gross_mu === 2_000_000 ? 1_700_000 : 850_000
    );
    assert.equal(earning.last_settlement_msb_tx_hash, 'b'.repeat(64));
  }
  const fee = (await ctx.storage.get('fee/tnk/cum')).value;
  assert.equal(fee.cum_mu, 450_000);
  assert.equal(fee.swept_cum_mu, 450_000);
  assert.equal(fee.last_settlement_msb_tx_hash, 'b'.repeat(64));
  const record = (await ctx.storage.get('settle/tnk/1')).value;
  assert.equal(record.idempotency_key, `mayhem:tnk:settle:v1:testnet1:mayhem:1:${ctx.applyState.last_apply_hash}`);
  assert.equal(record.msb_tx_hash, 'b'.repeat(64));
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
    msb_tx_hash: 'b'.repeat(64),
  });
  assert.equal(ctx.storage.snapshotBytes(), snapshot);

  const changed = await executeTnkSettlementFeature(
    ctx.contract,
    ctx.storage,
    { ...settlement, msb_tx_hash: 'c'.repeat(64) },
    ctx.admin.publicKey
  );
  assert.match(changed.message, /already exists/i);
  assert.equal(ctx.storage.snapshotBytes(), snapshot);
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
    { ...settlement, outputs },
    ctx.admin.publicKey
  );
  assert.doesNotMatch(result.message, /max_tnk_settlement_outputs|exceeds limit/i);
  assert.match(result.message, /provider count does not match outputs/i);
  assert.equal(await ctx.storage.get('settle/tnk/1'), null);
});

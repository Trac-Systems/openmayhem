import assert from 'node:assert/strict';
import test from 'node:test';
import MayhemContract, { CONTRACT_VERSION } from '../contract/contract.js';
import MayhemProtocol from '../contract/protocol.js';
import {
  MemoryStorage,
  execute,
  executeEpochApplyFeature,
  executeFeature,
  executeSpendReservationFeature,
  epochApplyFeatureKey,
  makeIdentity,
  makeTxKey,
  makeVerifier,
  seedCurrentAdminPrice,
  signConsent,
  signSpendReservation,
  signSpendVoucher,
  spendReservationFeatureKey,
} from './helpers/contract.js';

const rulesHash = '7'.repeat(64);
const TEXT_LOCKED_RATE_MAP = Object.freeze([
  { unit: 'input_token', per_unit_au: '20', granularity: 1_000 },
  { unit: 'output_token', per_unit_au: '60', granularity: 1_000 },
]);
const EMBEDDING_LOCKED_RATE_MAP = Object.freeze([
  { unit: 'input_token', per_unit_au: '2', granularity: 1_000 },
]);
const CTX_BRACKET_TABLE_VERSION = 1;
const ctxBracketForTokens = (tokens) => {
  if (tokens <= 8_192) return 'le8k';
  if (tokens <= 32_768) return 'le32k';
  if (tokens <= 131_072) return 'le128k';
  if (tokens <= 262_144) return 'le256k';
  return 'gt256k';
};

const providerRegistration = {
  op: 'register_provider',
};

const auString = (value) => String(value);

const seededBalance = (user, au, rail = 'fiat') => ({
  user,
  rail,
  denom: 'au_usd',
  au: auString(au),
  updated_epoch: 0,
  updated_at: null,
});

const paymentKeys = (storage) =>
  Array.from(storage.values.keys())
    .filter((key) => key.startsWith('bal/') || key.startsWith('earn/') || key.startsWith('fee/'))
    .sort();

const makeEpochApply = (epoch, user, provider, grossAu) => {
  const au = auString(grossAu);
  return {
    op: 'epoch_apply',
    epoch,
    at: epoch * 3600,
    debits: [{ rail: 'fiat', user, au }],
    earnings: [{ rail: 'fiat', provider, gross_au: au }],
  };
};

const mapPaidOps = (commands) => {
  const protocol = new MayhemProtocol({}, {});
  return commands
    .map((command) => protocol.mapTxCommand(JSON.stringify(command)))
    .filter(Boolean);
};

async function setupLedgerContract(identities = null) {
  const admin = identities?.admin ?? await makeIdentity();
  const provider = identities?.provider ?? await makeIdentity();
  const provider2 = identities?.provider2 ?? await makeIdentity();
  const user = identities?.user ?? await makeIdentity();
  const outsider = identities?.outsider ?? await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const protocol = { peer: { wallet: makeVerifier(provider.wallet) } };
  const contract = new MayhemContract(protocol, {});

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
      type: 'consent',
      value: {
        op: 'consent',
        ver: 1,
        hash: rulesHash,
        sig: signConsent(provider2.wallet, 1, rulesHash),
      },
      sender: provider2.publicKey,
      txNo: 4,
    },
    {
      type: 'registerProvider',
      value: providerRegistration,
      sender: provider2.publicKey,
      txNo: 5,
    },
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }

  await storage.put(`bal/${user.publicKey}/fiat`, seededBalance(user.publicKey, 1_000_000));
  return { admin, provider, provider2, user, outsider, storage, contract };
}

async function seedReservationServing(ctx, provider = ctx.provider) {
  const enclaveId = 'e1'.repeat(32);
  const modelId = 'test/model@4bit';
  await ctx.storage.put(`enclave/${enclaveId}`, {
    enclave_id: enclaveId,
    model_id: modelId,
    model_class: 'text-generation',
    backend: 'llama.cpp',
    artifact_root: 'a1'.repeat(32),
    artifact_root_kind: 'blake3_merkle_v1',
    artifact_source: 'huggingface://trac-network/test/model.gguf',
    manifest_hash: 'b1'.repeat(32),
    binary_hash: 'c1'.repeat(32),
    att_tier: 1,
    caps: { chat: true, tools: true, json: true, ctx: 8192, ctx_max: 8192 },
    status: 'active',
    created_by: ctx.admin.publicKey,
    created_by_role: 'admin',
    created_at: makeTxKey(10),
    updated_at: makeTxKey(10),
  });
  await ctx.storage.put(`serve/${provider.publicKey}/${enclaveId}`, {
    provider: provider.publicKey,
    enclave_id: enclaveId,
    model_id: modelId,
    status: 'active',
    served_ctx: 8192,
    ctx_bracket: 'le8k',
    ctx_bracket_table_ver: CTX_BRACKET_TABLE_VERSION,
    joined_at: makeTxKey(11),
    updated_at: makeTxKey(11),
    via: 'feature',
  });
  await seedCurrentAdminPrice(ctx.storage, {
    enclaveId,
    modelId,
    admin: ctx.admin.publicKey,
    txNo: 12,
    ver: 1,
    inPer1kAu: 20,
    outPer1kAu: 60,
    minSessionAu: 100,
    effectiveAt: 0,
  });
  return { enclaveId, modelId };
}

async function seedNonTextReservationServing(ctx, provider = ctx.provider) {
  const enclaveId = 'e2'.repeat(32);
  const modelId = 'test/embedding@q8';
  await ctx.storage.put(`enclave/${enclaveId}`, {
    enclave_id: enclaveId,
    model_id: modelId,
    model_class: 'embedding',
    backend: 'llama.cpp',
    artifact_root: 'a2'.repeat(32),
    artifact_root_kind: 'blake3_merkle_v1',
    artifact_source: 'huggingface://trac-network/test/embedding.gguf',
    manifest_hash: 'b2'.repeat(32),
    binary_hash: 'c2'.repeat(32),
    att_tier: 1,
    caps: { embedding: true, ctx: 512, ctx_max: 512 },
    status: 'active',
    created_by: ctx.admin.publicKey,
    created_by_role: 'admin',
    created_at: makeTxKey(20),
    updated_at: makeTxKey(20),
  });
  await ctx.storage.put(`serve/${provider.publicKey}/${enclaveId}`, {
    provider: provider.publicKey,
    enclave_id: enclaveId,
    model_id: modelId,
    status: 'active',
    served_ctx: 512,
    ctx_bracket: null,
    ctx_bracket_table_ver: null,
    joined_at: makeTxKey(21),
    updated_at: makeTxKey(21),
    via: 'feature',
  });
  await seedCurrentAdminPrice(ctx.storage, {
    enclaveId,
    modelId,
    admin: ctx.admin.publicKey,
    txNo: 22,
    ver: 1,
    rateMap: EMBEDDING_LOCKED_RATE_MAP,
    minSessionAu: 10,
    effectiveAt: 0,
    ctxBracket: null,
  });
  return { enclaveId, modelId };
}

function signedSpendReservation(
  ctx,
  {
    provider = ctx.provider,
    enclaveId,
    sessionId,
    maxSpendAu,
    epoch = 1,
    priceVer = 1,
    lockedRateMap = TEXT_LOCKED_RATE_MAP,
    lockedPerReqAu = 0,
    lockedMinSessionAu = 100,
    servedCtx = 8192,
    ctxBracket = ctxBracketForTokens(servedCtx),
    ctxBracketTableVer = CTX_BRACKET_TABLE_VERSION,
    at = epoch * 3_600,
  } = {}
) {
  const maxSpendAuString = auString(maxSpendAu);
  const lockedPerReqAuString = auString(lockedPerReqAu);
  const lockedMinSessionAuString = auString(lockedMinSessionAu);
  const voucherBody = {
    session_id: sessionId,
    rail: 'fiat',
    enclave_id: enclaveId,
    price_ver: priceVer,
    locked_rate_map: lockedRateMap,
    locked_per_req_au: lockedPerReqAuString,
    locked_min_session_au: lockedMinSessionAuString,
    served_ctx: servedCtx,
    ctx_bracket: ctxBracket,
    ctx_bracket_table_ver: ctxBracketTableVer,
    max_spend_au: maxSpendAuString,
    checkpoint_every: { tokens: 8192, ms: 30_000 },
  };
  const unsigned = {
    op: 'spend_reserve',
    contract_version: CONTRACT_VERSION,
    session_id: sessionId,
    epoch,
    at,
    rail: 'fiat',
    user: ctx.user.publicKey,
    provider: provider.publicKey,
    enclave_id: enclaveId,
    price_ver: priceVer,
    rules_ver: 1,
    served_ctx: servedCtx,
    ctx_bracket: ctxBracket,
    ctx_bracket_table_ver: ctxBracketTableVer,
    max_spend_au: maxSpendAuString,
    voucher: {
      ...voucherBody,
      user_sig: signSpendVoucher(ctx.user.wallet, voucherBody),
    },
    provider_sig: '',
  };
  return {
    ...unsigned,
    provider_sig: signSpendReservation(provider.wallet, unsigned),
  };
}

test('MayhemProtocol keeps epochApply off the paid tx route', () => {
  const protocol = new MayhemProtocol({}, {});
  const paidOps = [
    { op: 'epoch_commit', epoch: 1, at: 3_600, roots: {}, totals: {} },
    makeEpochApply(1, 'user-a', 'provider-a', 1_000),
  ]
    .map((command) => protocol.mapTxCommand(JSON.stringify(command)))
    .filter(Boolean);

  assert.deepEqual(paidOps.map((op) => op.type), ['epochCommit']);
});

test('MayhemProtocol maps empty epoch seals to the paid admin tx route', () => {
  const protocol = new MayhemProtocol({}, {});
  const op = protocol.mapTxCommand(JSON.stringify({
    op: 'epoch_seal_empty',
    epoch: 1,
    at: 3_600,
    reason_hash: 'a'.repeat(64),
  }));

  assert.deepEqual(op, {
    type: 'epochSealEmpty',
    value: {
      op: 'epoch_seal_empty',
      epoch: 1,
      at: 3_600,
      reason_hash: 'a'.repeat(64),
    },
  });
});

test('MayhemProtocol keeps deposit evidence off the paid tx route', () => {
  const protocol = new MayhemProtocol({}, {});
  const paidOps = [
    { op: 'epoch_commit', epoch: 1, at: 3_600, roots: {}, totals: {} },
    { op: 'deposit_tnk', memo_hash: 'memo-1' },
    { op: 'deposit_tnk', memo_hash: 'memo-2' },
    {
      op: 'tnk_deposit',
      memo_hash: 'memo-1',
      tnk_e18: '1000000000000000000',
      msb_tx_hash: 'a'.repeat(64),
      epoch: 1,
      at: 3_600,
    },
    {
      op: 'tap_deposit',
      who: '0x1111111111111111111111111111111111111111',
      tap_wei: '1000000000000000000',
      eth_tx_hash: `0x${'b'.repeat(64)}`,
      log_index: 0,
      block_number: 123,
      pool_address: '0x2222222222222222222222222222222222222222',
      chain_id: 61_000,
      epoch: 1,
      at: 3_600,
    },
    {
      op: 'fiat_deposit',
      rail: 'stripe',
      who: 'user-a',
      au: '1000000',
      ext_ref_hash: 'c'.repeat(64),
      fiat_currency: 'usd',
      fiat_amount_minor: 100,
      epoch: 1,
      at: 3_600,
    },
  ]
    .map((command) => protocol.mapTxCommand(JSON.stringify(command)))
    .filter(Boolean);

  assert.deepEqual(paidOps.map((op) => op.type), ['epochCommit']);
});

test('MayhemProtocol keeps spend reservations off the paid tx route', () => {
  const protocol = new MayhemProtocol({}, {});
  const paidOps = [
    { op: 'epoch_commit', epoch: 1, at: 3_600, roots: {}, totals: {} },
    {
      op: 'spend_reserve',
      contract_version: CONTRACT_VERSION,
      session_id: 'a'.repeat(64),
      epoch: 1,
      at: 3_600,
      rail: 'fiat',
      user: 'b'.repeat(64),
      provider: 'c'.repeat(64),
      enclave_id: 'd'.repeat(64),
      price_ver: 1,
      rules_ver: 1,
      max_spend_au: '1000',
      voucher: {},
      provider_sig: 'e'.repeat(128),
    },
  ]
    .map((command) => protocol.mapTxCommand(JSON.stringify(command)))
    .filter(Boolean);

  assert.deepEqual(paidOps.map((op) => op.type), ['epochCommit']);
});

test('MayhemProtocol keeps payout claims off the paid tx route', () => {
  const protocol = new MayhemProtocol({}, {});
  const paidOps = [
    { op: 'epoch_commit', epoch: 1, at: 3_600, roots: {}, totals: {} },
    {
      op: 'payout_confirm',
      epoch: 1,
      who: 'provider-a',
      au: '100',
      tnk_e18: '1000000000000000000',
      msb_tx_hash: 'a'.repeat(64),
      at: 3_600,
    },
    {
      op: 'tnk_settlement',
      epoch: 1,
      at: 3_600,
      rail: 'tnk',
      network: 'testnet1',
      treasury_from: 'testtrac1treasury',
      operator_to: 'testtrac1operator',
      epoch_apply_hash: 'a'.repeat(64),
      rate_tnk_usd_au: '50000000000000000',
      rate_source: 'gate-spot',
      rate_ts: 3_600,
      msb_tx_hash: 'b'.repeat(64),
      transfer_root: 'c'.repeat(64),
      provider_count: 0,
      provider_au: '0',
      operator_fee_au: '1',
      gross_au: '1',
      tnk_e18: '20000000000000',
      outputs: [
        {
          role: 'operator_fee',
          to: 'testtrac1operator',
          au: '1',
          tnk_e18: '20000000000000',
        },
      ],
    },
    {
      op: 'payout_confirm',
      epoch: 1,
      who: 'provider-b',
      au: '200',
      rail: 'stripe',
      external_ref: 'tr_provider_b',
      fiat_currency: 'usd',
      fiat_amount_minor: 1,
      at: 3_601,
    },
  ]
    .map((command) => protocol.mapTxCommand(JSON.stringify(command)))
    .filter(Boolean);

  assert.deepEqual(paidOps.map((op) => op.type), ['epochCommit']);
});

test('MayhemProtocol keeps rate oracle updates off the paid tx route', () => {
  const commands = [
    { op: 'epoch_commit', epoch: 1, at: 3_600, roots: {}, totals: {} },
    { op: 'rate_oracle', tnk_usd_au: '50000000000000000', source: 'gate-spot', ts: 3_600 },
    { op: 'rate_oracle', tnk_usd_au: '51000000000000000', source: 'mexc-spot', ts: 5_400 },
    { op: 'tap_rate_oracle', tap_usd_au: '50000000000000000', source: 'uniswap-v2', ts: 3_600 },
    { op: 'tap_rate_oracle', tap_usd_au: '52000000000000000', source: 'config', ts: 5_400 },
  ];

  assert.deepEqual(mapPaidOps(commands).map((op) => op.type), ['epochCommit']);
});

test('MayhemProtocol steady-state sponsorship stays at one paid tx per active epoch', () => {
  const activeEpochs = 3;
  const commands = [];

  for (let epoch = 1; epoch <= activeEpochs; epoch += 1) {
    commands.push({ op: 'epoch_commit', epoch, at: epoch * 3_600, roots: {}, totals: {} });
    commands.push(makeEpochApply(epoch, `user-${epoch}`, `provider-${epoch}`, 1_000 + epoch));
    commands.push({ op: 'rate_oracle', tnk_usd_au: `${50_000n + BigInt(epoch)}000000000000`, source: 'gate-spot', ts: epoch * 3_600 });
    commands.push({ op: 'tap_rate_oracle', tap_usd_au: `${50_000n + BigInt(epoch)}000000000000`, source: 'uniswap-v2', ts: epoch * 3_600 });

    for (let i = 0; i < 4; i += 1) {
      commands.push({
        op: 'deposit_tnk',
        sender: `user-${epoch}-${i}`,
        intent: { memo_hash: `memo-${epoch}-${i}` },
        sig: 'sig',
      });
      commands.push({
        op: 'tnk_deposit',
        memo_hash: `memo-${epoch}-${i}`,
        tnk_e18: '1000000000000000000',
        msb_tx_hash: `${epoch}${i}`.padEnd(64, 'a'),
        epoch,
        at: epoch * 3_600 + i,
      });
      commands.push({
        op: 'tap_deposit',
        who: `0x${String(epoch).repeat(40).slice(0, 40)}`,
        tap_wei: '1000000000000000000',
        eth_tx_hash: `0x${`${epoch}${i}`.padEnd(64, 'b')}`,
        log_index: i,
        block_number: 100 + i,
        pool_address: '0x2222222222222222222222222222222222222222',
        chain_id: 61_000,
        epoch,
        at: epoch * 3_600 + i,
      });
      commands.push({
        op: 'fiat_deposit',
        rail: 'stripe',
        who: `user-${epoch}-${i}`,
        au: '1000000',
        ext_ref_hash: `${epoch}${i}`.padEnd(64, 'c'),
        fiat_currency: 'usd',
        fiat_amount_minor: 100,
        epoch,
        at: epoch * 3_600 + i,
      });
      commands.push({
        op: 'payout_confirm',
        epoch,
        who: `provider-${epoch}-${i}`,
        au: '100' + i,
        tnk_e18: '1000000000000000000',
        msb_tx_hash: `${epoch}${i}`.padEnd(64, 'd'),
        at: epoch * 3_600 + i,
      });
    }
  }

  const paidOps = mapPaidOps(commands);
  assert.deepEqual(paidOps.map((op) => op.type), Array(activeEpochs).fill('epochCommit'));
  assert.equal(paidOps.length, activeEpochs);

  const paidByEpoch = new Map();
  for (const op of paidOps) {
    paidByEpoch.set(op.value.epoch, (paidByEpoch.get(op.value.epoch) ?? 0) + 1);
  }
  for (let epoch = 1; epoch <= activeEpochs; epoch += 1) {
    assert.equal(paidByEpoch.get(epoch), 1, `epoch ${epoch} must have exactly one paid anchor`);
  }
});

test('MayhemContract spend reservation enforces active-epoch unreserved user balance', async () => {
  const ctx = await setupLedgerContract();
  const { enclaveId } = await seedReservationServing(ctx, ctx.provider);
  await seedReservationServing(ctx, ctx.provider2);

  const first = signedSpendReservation(ctx, {
    provider: ctx.provider,
    enclaveId,
    sessionId: 'a1'.repeat(32),
    maxSpendAu: 700_000,
  });
  const firstKey = await spendReservationFeatureKey(ctx.contract, first, ctx.storage);
  const firstResult = await executeSpendReservationFeature(
    ctx.contract,
    ctx.storage,
    first,
    ctx.provider.publicKey
  );
  assert.equal(firstResult.ok, true, firstResult.message);
  assert.equal(firstResult.idempotent, false);
  assert.equal(firstResult.available_au, '300000');

  const hold = (await ctx.storage.get(`hold/fiat/${ctx.user.publicKey}/1`)).value;
  assert.equal(hold.user, ctx.user.publicKey);
  assert.equal(hold.rail, 'fiat');
  assert.equal(hold.epoch, 1);
  assert.equal(hold.reserved_au, '700000');
  assert.equal(hold.balance_au_at_last_reserve, '1000000');
  assert.equal(hold.sessions.length, 1);
  assert.equal(hold.sessions[0].session_id, 'a1'.repeat(32));
  assert.equal(hold.sessions[0].provider, ctx.provider.publicKey);
  assert.equal(hold.sessions[0].feature_key, firstKey);
  assert.equal(hold.sessions[0].served_ctx, 8192);
  assert.equal(hold.sessions[0].ctx_bracket, 'le8k');
  assert.equal(hold.sessions[0].ctx_bracket_table_ver, CTX_BRACKET_TABLE_VERSION);
  assert.match(hold.sessions[0].voucher_hash, /^[0-9a-f]{64}$/);

  const replay = await executeSpendReservationFeature(
    ctx.contract,
    ctx.storage,
    first,
    ctx.provider.publicKey
  );
  assert.equal(replay.ok, true, replay.message);
  assert.equal(replay.idempotent, true);
  assert.equal(replay.reserved_au, '700000');

  const second = signedSpendReservation(ctx, {
    provider: ctx.provider2,
    enclaveId,
    sessionId: 'a2'.repeat(32),
    maxSpendAu: 400_000,
  });
  const secondResult = await executeSpendReservationFeature(
    ctx.contract,
    ctx.storage,
    second,
    ctx.provider2.publicKey
  );
  assert.match(secondResult.message, /Insufficient unreserved credit balance/);
  assert.equal((await ctx.storage.get(`hold/fiat/${ctx.user.publicKey}/1`)).value.reserved_au, '700000');
});

test('MayhemContract spend reservation accepts unbracketed non-text markets only', async () => {
  const ctx = await setupLedgerContract();
  const { enclaveId } = await seedNonTextReservationServing(ctx, ctx.provider);

  const accepted = signedSpendReservation(ctx, {
    provider: ctx.provider,
    enclaveId,
    sessionId: 'e3'.repeat(32),
    maxSpendAu: 100_000,
    lockedRateMap: EMBEDDING_LOCKED_RATE_MAP,
    lockedMinSessionAu: 10,
    servedCtx: 512,
    ctxBracket: null,
    ctxBracketTableVer: null,
  });
  const result = await executeSpendReservationFeature(
    ctx.contract,
    ctx.storage,
    accepted,
    ctx.provider.publicKey
  );
  assert.equal(result.ok, true, result.message);

  const hold = (await ctx.storage.get(`hold/fiat/${ctx.user.publicKey}/1`)).value;
  assert.equal(hold.sessions[0].ctx_bracket, null);
  assert.equal(hold.sessions[0].ctx_bracket_table_ver, null);

  const rejected = signedSpendReservation(ctx, {
    provider: ctx.provider,
    enclaveId,
    sessionId: 'e4'.repeat(32),
    maxSpendAu: 100_000,
    lockedRateMap: EMBEDDING_LOCKED_RATE_MAP,
    lockedMinSessionAu: 10,
    servedCtx: 512,
    ctxBracket: 'le8k',
    ctxBracketTableVer: CTX_BRACKET_TABLE_VERSION,
  });
  await assert.rejects(
    () => spendReservationFeatureKey(ctx.contract, rejected, ctx.storage),
    /only valid for text-generation enclaves/
  );
});

test('MayhemContract spend reservation keeps the locked quote after market price advances', async () => {
  const ctx = await setupLedgerContract();
  const { enclaveId, modelId } = await seedReservationServing(ctx, ctx.provider);

  const lockedAtV1 = signedSpendReservation(ctx, {
    provider: ctx.provider,
    enclaveId,
    sessionId: 'f3'.repeat(32),
    maxSpendAu: 100_000,
    priceVer: 1,
    lockedRateMap: TEXT_LOCKED_RATE_MAP,
  });

  const v2RateMap = [
    { unit: 'input_token', per_unit_au: '40', granularity: 1_000 },
    { unit: 'output_token', per_unit_au: '120', granularity: 1_000 },
  ];
  await seedCurrentAdminPrice(ctx.storage, {
    enclaveId,
    modelId,
    admin: ctx.admin.publicKey,
    txNo: 20,
    ver: 2,
    inPer1kAu: 40,
    outPer1kAu: 120,
    minSessionAu: 100,
    effectiveAt: 3_600,
  });
  assert.equal((await ctx.storage.get(`price/${enclaveId}/le8k`)).value.current.ver, 2);

  const result = await executeSpendReservationFeature(
    ctx.contract,
    ctx.storage,
    lockedAtV1,
    ctx.provider.publicKey
  );
  assert.equal(result.ok, true, result.message);

  const hold = (await ctx.storage.get(`hold/fiat/${ctx.user.publicKey}/1`)).value;
  assert.equal(hold.sessions.length, 1);
  assert.equal(hold.sessions[0].price_ver, 1);
  assert.deepEqual(hold.sessions[0].locked_rate_map, TEXT_LOCKED_RATE_MAP);
  assert.equal(hold.sessions[0].locked_per_req_au, '0');
  assert.equal(hold.sessions[0].locked_min_session_au, '100');
  assert.equal(hold.sessions[0].served_ctx, 8192);
  assert.equal(hold.sessions[0].ctx_bracket, 'le8k');
  assert.equal(hold.sessions[0].ctx_bracket_table_ver, CTX_BRACKET_TABLE_VERSION);

  const forgedV1Quote = signedSpendReservation(ctx, {
    provider: ctx.provider,
    enclaveId,
    sessionId: 'f4'.repeat(32),
    maxSpendAu: 100_000,
    priceVer: 1,
    lockedRateMap: v2RateMap,
  });
  const rejected = await executeSpendReservationFeature(
    ctx.contract,
    ctx.storage,
    forgedV1Quote,
    ctx.provider.publicKey
  );
  assert.match(rejected.message, /locked rate_map/i);
});

test('MayhemContract spend reservation moves to next epoch after epochApply', async () => {
  const ctx = await setupLedgerContract();
  const { enclaveId } = await seedReservationServing(ctx, ctx.provider);

  const first = signedSpendReservation(ctx, {
    provider: ctx.provider,
    enclaveId,
    sessionId: 'b1'.repeat(32),
    maxSpendAu: 900_000,
  });
  const firstResult = await executeSpendReservationFeature(
    ctx.contract,
    ctx.storage,
    first,
    ctx.provider.publicKey
  );
  assert.equal(firstResult.ok, true, firstResult.message);

  const applyValue = makeEpochApply(1, ctx.user.publicKey, ctx.provider.publicKey, 100_000);
  const applied = await executeEpochApplyFeature(ctx.contract, ctx.storage, applyValue, ctx.admin.publicKey);
  assert.equal(applied.ok, true, applied.message);
  assert.equal((await ctx.storage.get(`bal/${ctx.user.publicKey}/fiat`)).value.au, '900000');

  const nextEpoch = signedSpendReservation(ctx, {
    provider: ctx.provider,
    enclaveId,
    sessionId: 'b2'.repeat(32),
    maxSpendAu: 900_000,
    epoch: 2,
  });
  const nextResult = await executeSpendReservationFeature(
    ctx.contract,
    ctx.storage,
    nextEpoch,
    ctx.provider.publicKey
  );
  assert.equal(nextResult.ok, true, nextResult.message);
  assert.equal(nextResult.available_au, '0');
});

test('MayhemContract context bracket tables are admin scheduled and pinned by version', async () => {
  const ctx = await setupLedgerContract();
  const { enclaveId } = await seedReservationServing(ctx, ctx.provider);

  const update = {
    op: 'set_ctx_brackets',
    submitted_at: 0,
    effective_at: 86_400,
    brackets: [
      { id: 'le16k', max_ctx: 16_384 },
      { id: 'le64k', max_ctx: 65_536 },
      { id: 'gt64k', max_ctx: null },
    ],
  };
  const nonAdmin = await execute(ctx.contract, ctx.storage, 'setCtxBrackets', update, ctx.provider.publicKey, 20);
  assert.match(nonAdmin.message, /Admin required/);

  const scheduled = await execute(ctx.contract, ctx.storage, 'setCtxBrackets', update, ctx.admin.publicKey, 21);
  assert.equal(scheduled.ok, true, scheduled.message);
  assert.equal(scheduled.ver, 2);

  const before = await execute(
    ctx.contract,
    ctx.storage,
    'readCtxBrackets',
    { op: 'read_ctx_brackets', at: 86_399 },
    ctx.user.publicKey,
    22
  );
  assert.equal(before.table.ver, 1);
  assert.equal(before.table.brackets[0].id, 'le8k');

  const after = await execute(
    ctx.contract,
    ctx.storage,
    'readCtxBrackets',
    { op: 'read_ctx_brackets', at: 86_400 },
    ctx.user.publicKey,
    23
  );
  assert.equal(after.table.ver, 2);
  assert.equal(after.table.brackets[0].id, 'le16k');

  const oldTableAfterActivation = signedSpendReservation(ctx, {
    provider: ctx.provider,
    enclaveId,
    sessionId: 'c1'.repeat(32),
    servedCtx: 12_000,
    ctxBracket: 'le32k',
    ctxBracketTableVer: 1,
    maxSpendAu: 100_000,
    at: 86_400,
  });
  await assert.rejects(
    spendReservationFeatureKey(ctx.contract, oldTableAfterActivation, ctx.storage),
    /not active/
  );

  const currentTableAfterActivation = signedSpendReservation(ctx, {
    provider: ctx.provider,
    enclaveId,
    sessionId: 'c2'.repeat(32),
    servedCtx: 12_000,
    ctxBracket: 'le16k',
    ctxBracketTableVer: 2,
    maxSpendAu: 100_000,
    at: 86_400,
  });
  const key = await spendReservationFeatureKey(ctx.contract, currentTableAfterActivation, ctx.storage);
  assert.match(key, /^hold\/reserve\/fiat\//);
});

test('MayhemContract epochApply mutates credit, earning, and fee state in place', async () => {
  const { admin, provider, user, outsider, storage, contract } = await setupLedgerContract();

  const nonAdmin = await execute(
    contract,
    storage,
    'epochApply',
    makeEpochApply(1, user.publicKey, provider.publicKey, 1_500),
    outsider.publicKey,
    4
  );
  assert.match(nonAdmin.message, /unknown contract operation type|function not registered/i);

  const nonAdminFeature = await executeEpochApplyFeature(
    contract,
    storage,
    makeEpochApply(1, user.publicKey, provider.publicKey, 1_500),
    outsider.publicKey
  );
  assert.match(nonAdminFeature.message, /admin required/i);

  const mismatch = await executeEpochApplyFeature(
    contract,
    storage,
    {
      op: 'epoch_apply',
      epoch: 1,
      at: 3600,
      debits: [{ rail: 'fiat', user: user.publicKey, au: '1500' }],
      earnings: [{ rail: 'fiat', provider: provider.publicKey, gross_au: '1400' }],
    },
    admin.publicKey
  );
  assert.match(mismatch.message, /must equal/i);

  const firstApply = {
    op: 'epoch_apply',
    epoch: 1,
    at: 3600,
    debits: [
      { rail: 'fiat', user: user.publicKey, au: '1000' },
      { rail: 'fiat', user: user.publicKey, au: '500' },
    ],
    earnings: [
      { rail: 'fiat', provider: provider.publicKey, gross_au: '1250' },
      { rail: 'fiat', provider: provider.publicKey, gross_au: '250' },
    ],
  };
  const firstApplyKey = await epochApplyFeatureKey(contract, firstApply);
  const wrongKeySnapshot = storage.snapshotBytes();
  const wrongKey = await executeFeature(
    contract,
    storage,
    'mayhem_feature',
    `epoch/apply/1/${'0'.repeat(64)}`,
    firstApply,
    admin.publicKey
  );
  assert.equal(wrongKey, undefined);
  assert.equal(storage.snapshotBytes(), wrongKeySnapshot);

  const first = await executeEpochApplyFeature(
    contract,
    storage,
    firstApply,
    admin.publicKey
  );
  assert.deepEqual(first, {
    ok: true,
    op: 'epochApply',
    epoch: 1,
    idempotent: false,
    debited_au: '1500',
    earned_au: '1275',
    fee_au: '225',
    rails: ['fiat'],
  });

  assert.deepEqual((await storage.get(`bal/${user.publicKey}/fiat`)).value, {
    user: user.publicKey,
    rail: 'fiat',
    denom: 'au_usd',
    au: '998500',
    updated_epoch: 1,
    updated_at: firstApplyKey,
  });
  assert.deepEqual((await storage.get(`earn/fiat/${provider.publicKey}`)).value, {
    provider: provider.publicKey,
    rail: 'fiat',
    denom: 'au_usd',
    total_au: '1275',
    held_au: '1275',
    paid_cum_au: '0',
    holdbacks: [{ epoch: 1, au: '1275', locked_epochs: 168 }],
    updated_epoch: 1,
    updated_at: firstApplyKey,
    last_holdback_release_epoch: 1,
  });
  const feeAfterFirst = (await storage.get('fee/fiat/cum')).value;
  assert.equal(feeAfterFirst.denom, 'au_usd');
  assert.equal(feeAfterFirst.cum_au, '225');
  assert.equal(feeAfterFirst.swept_cum_au, '0');
  assert.equal(feeAfterFirst.updated_epoch, 1);
  assert.equal(feeAfterFirst.updated_at, firstApplyKey);
  assert.equal(feeAfterFirst.last_fee_bps, 1_500);
  assert.equal(feeAfterFirst.last_apply_hash.length, 64);

  const snapshotBeforeReplay = storage.snapshotBytes();
  const replay = await executeEpochApplyFeature(
    contract,
    storage,
    firstApply,
    admin.publicKey
  );
  assert.deepEqual(replay, {
    ok: true,
    op: 'epochApply',
    epoch: 1,
    idempotent: true,
    debited_au: '0',
    earned_au: '0',
    fee_au: '0',
  });
  assert.equal(storage.snapshotBytes(), snapshotBeforeReplay);

  const changedReplay = await executeEpochApplyFeature(
    contract,
    storage,
    makeEpochApply(1, user.publicKey, provider.publicKey, 2_000),
    admin.publicKey
  );
  assert.match(changedReplay.message, /monotonic/i);

  const gap = await executeEpochApplyFeature(
    contract,
    storage,
    makeEpochApply(3, user.publicKey, provider.publicKey, 1_000),
    admin.publicKey
  );
  assert.match(gap.message, /contiguous/i);

  const insufficientSnapshot = storage.snapshotBytes();
  const insufficient = await executeEpochApplyFeature(
    contract,
    storage,
    makeEpochApply(2, user.publicKey, provider.publicKey, 2_000_000),
    admin.publicKey
  );
  assert.match(insufficient.message, /insufficient credit balance/i);
  assert.equal(storage.snapshotBytes(), insufficientSnapshot);
});

test('MayhemContract graduated holdback steps down without shortening older buckets', async () => {
  const contract = new MayhemContract({}, {});
  const params = {
    holdback_epochs: 24,
    challenge_epochs: 6,
    new_provider_holdback_epochs: 168,
    probation_successful_sessions: 50,
  };

  assert.equal(
    contract.providerLockedEarningEpochs({ probation: { successful_sessions: 0 } }, params),
    168
  );
  assert.equal(
    contract.providerLockedEarningEpochs({ probation: { successful_sessions: 49 } }, params),
    168
  );
  assert.equal(
    contract.providerLockedEarningEpochs({ probation: { successful_sessions: 50 } }, params),
    24
  );

  const refreshed = contract.refreshEarningHoldback(
    {
      provider: 'provider-a',
      rail: 'fiat',
      denom: 'au_usd',
      total_au: '1700',
      held_au: '1700',
      paid_cum_au: '0',
      updated_epoch: 2,
      holdbacks: [
        { epoch: 1, au: '850', locked_epochs: 168 },
        { epoch: 2, au: '850', locked_epochs: 24 },
      ],
    },
    26,
    24
  );
  assert.equal(refreshed.held_au, '850');
  assert.deepEqual(refreshed.holdbacks, [{ epoch: 1, au: '850', locked_epochs: 168 }]);
  assert.equal(refreshed.last_holdback_release_epoch, 26);
});

test('MayhemContract epochApply is deterministic and payment key growth stays flat over 100 epochs', async () => {
  const identities = {
    admin: await makeIdentity(),
    provider: await makeIdentity(),
    provider2: await makeIdentity(),
    user: await makeIdentity(),
    outsider: await makeIdentity(),
  };
  const left = await setupLedgerContract(identities);
  const right = await setupLedgerContract(identities);
  let expectedDebited = 0n;
  let expectedFee = 0n;
  const netEarningsByEpoch = [];

  let totalKeysAfterFirst = null;
  let paymentKeysAfterFirst = null;
  for (let epoch = 1; epoch <= 100; epoch++) {
    const grossAu = 1_000 + (epoch % 7);
    const feeAu = (BigInt(grossAu) * 1_500n) / 10_000n;
    expectedDebited += BigInt(grossAu);
    expectedFee += feeAu;
    netEarningsByEpoch.push(BigInt(grossAu) - feeAu);
    for (const ctx of [left, right]) {
      const result = await executeEpochApplyFeature(
        ctx.contract,
        ctx.storage,
        makeEpochApply(epoch, identities.user.publicKey, identities.provider.publicKey, grossAu),
        identities.admin.publicKey
      );
      assert.equal(result.ok, true, result.message);
      assert.equal(result.epoch, epoch);
    }

    if (epoch === 1) {
      totalKeysAfterFirst = left.storage.values.size;
      paymentKeysAfterFirst = paymentKeys(left.storage);
    }
  }

  assert.equal(left.storage.snapshotBytes(), right.storage.snapshotBytes());
  assert.equal(left.storage.values.size, totalKeysAfterFirst);
  assert.deepEqual(paymentKeys(left.storage), paymentKeysAfterFirst);
  assert.deepEqual(paymentKeysAfterFirst, [
    `bal/${identities.user.publicKey}/fiat`,
    `earn/fiat/${identities.provider.publicKey}`,
    'fee/fiat/cum',
  ].sort());

  const balance = (await left.storage.get(`bal/${identities.user.publicKey}/fiat`)).value;
  const earning = (await left.storage.get(`earn/fiat/${identities.provider.publicKey}`)).value;
  const fee = (await left.storage.get('fee/fiat/cum')).value;
  const expectedHeld = netEarningsByEpoch.slice(-168).reduce((sum, au) => sum + au, 0n);
  assert.equal(balance.au, (1_000_000n - expectedDebited).toString());
  assert.equal(earning.total_au, (expectedDebited - expectedFee).toString());
  assert.equal(earning.held_au, expectedHeld.toString());
  assert.equal(earning.paid_cum_au, '0');
  assert.equal(fee.cum_au, expectedFee.toString());
  assert.equal(fee.updated_epoch, 100);
});

test('MayhemContract epochApply replays codepoint-sorted varied keys deterministically', async () => {
  const identities = {
    admin: await makeIdentity(),
    provider: await makeIdentity(),
    provider2: await makeIdentity(),
    user: await makeIdentity(),
    outsider: await makeIdentity(),
  };
  const left = await setupLedgerContract(identities);
  const right = await setupLedgerContract(identities);
  const providers = ['ProviderA', 'providera'];
  const users = ['UserA', 'usera'];

  for (const ctx of [left, right]) {
    for (const provider of providers) {
      await ctx.storage.put(`prov/${provider}`, {
        provider,
        status: 'active',
        accepted_rails: ['fiat'],
      });
    }
    await ctx.storage.put(`bal/${users[0]}/fiat`, seededBalance(users[0], 500));
    await ctx.storage.put(`bal/${users[1]}/fiat`, seededBalance(users[1], 700));
  }

  const leftApply = {
    op: 'epoch_apply',
    epoch: 1,
    at: 3600,
    debits: [
      { rail: 'fiat', user: users[1], au: '700' },
      { rail: 'fiat', user: users[0], au: '500' },
    ],
    earnings: [
      { rail: 'fiat', provider: providers[1], gross_au: '700' },
      { rail: 'fiat', provider: providers[0], gross_au: '500' },
    ],
  };
  const rightApply = {
    ...leftApply,
    debits: [...leftApply.debits].reverse(),
    earnings: [...leftApply.earnings].reverse(),
  };

  for (const [ctx, value] of [[left, leftApply], [right, rightApply]]) {
    const result = await executeEpochApplyFeature(ctx.contract, ctx.storage, value, identities.admin.publicKey);
    assert.equal(result.ok, true, result.message);
    assert.equal(result.fee_au, '180');
    assert.equal(result.earned_au, '1020');
  }

  const stripTxStamps = (record) => {
    const value = { ...record };
    delete value.updated_at;
    return value;
  };
  assert.equal(
    (await left.storage.get('epoch/apply/state')).value.last_apply_hash,
    (await right.storage.get('epoch/apply/state')).value.last_apply_hash
  );
  for (const key of [
    `bal/${users[0]}/fiat`,
    `bal/${users[1]}/fiat`,
    `earn/fiat/${providers[0]}`,
    `earn/fiat/${providers[1]}`,
    'fee/fiat/cum',
  ]) {
    assert.deepEqual(stripTxStamps((await left.storage.get(key)).value), stripTxStamps((await right.storage.get(key)).value));
  }
});

test('MayhemContract epochApply computes large fee bps with exact BigInt math', async () => {
  const { admin, provider, user, storage, contract } = await setupLedgerContract();
  const grossAu = '2000000000000000000000000';
  await storage.put(`bal/${user.publicKey}/fiat`, seededBalance(user.publicKey, grossAu));

  const result = await executeEpochApplyFeature(
    contract,
    storage,
    makeEpochApply(1, user.publicKey, provider.publicKey, grossAu),
    admin.publicKey
  );
  assert.equal(result.ok, true, result.message);

  const expectedFee = (BigInt(grossAu) * 1_500n) / 10_000n;
  const expectedProvider = BigInt(grossAu) - expectedFee;
  assert.equal(result.fee_au, expectedFee.toString());
  assert.equal(result.earned_au, expectedProvider.toString());
  assert.equal((await storage.get(`bal/${user.publicKey}/fiat`)).value.au, '0');
  assert.equal((await storage.get(`earn/fiat/${provider.publicKey}`)).value.total_au, expectedProvider.toString());
  assert.equal((await storage.get('fee/fiat/cum')).value.cum_au, expectedFee.toString());
});

test('MayhemContract au helpers reject numeric money inputs', async () => {
  const { contract } = await setupLedgerContract();
  assert.match(contract.safeAddAu(Number.MAX_SAFE_INTEGER, '1').message, /canonical decimal string/i);
  assert.match(contract.safeMulDivAu(Number.MAX_SAFE_INTEGER, 10_000, 1).message, /canonical decimal string/i);
  assert.equal(contract.safeAddAu('9007199254740993', '1'), '9007199254740994');
});

test('MayhemContract epochApply enforces max_apply_batch before writing', async () => {
  const { admin, provider, storage, contract } = await setupLedgerContract();
  const before = storage.snapshotBytes();
  const tooManyDebits = Array.from({ length: 2_001 }, (_, i) => ({
    rail: 'fiat',
    user: `user-${i}`,
    au: '1',
  }));
  const tooLarge = await executeEpochApplyFeature(
    contract,
    storage,
    {
      op: 'epoch_apply',
      epoch: 1,
      at: 3600,
      debits: tooManyDebits,
      earnings: [{ rail: 'fiat', provider: provider.publicKey, gross_au: '2001' }],
    },
    admin.publicKey
  );
  assert.match(tooLarge.message, /max_apply_batch/i);
  assert.equal(storage.snapshotBytes(), before);
});

test('MayhemContract epochApply uses the active admin max_apply_batch param', async () => {
  const { admin, provider, storage, contract } = await setupLedgerContract();
  const tuned = await execute(
    contract,
    storage,
    'setParams',
    {
      op: 'set_params',
      submitted_at: 0,
      effective_at: 86_400,
      values: { max_apply_batch: 3 },
    },
    admin.publicKey,
    4
  );
  assert.equal(tuned.ok, true, tuned.message);

  const before = storage.snapshotBytes();
  const tunedTooLarge = await executeEpochApplyFeature(
    contract,
    storage,
    {
      op: 'epoch_apply',
      epoch: 1,
      at: 86_400,
      debits: [
        { rail: 'fiat', user: 'user-a', au: '1' },
        { rail: 'fiat', user: 'user-b', au: '1' },
        { rail: 'fiat', user: 'user-c', au: '1' },
      ],
      earnings: [{ rail: 'fiat', provider: provider.publicKey, gross_au: '3' }],
    },
    admin.publicKey
  );
  assert.match(tunedTooLarge.message, /max_apply_batch/i);
  assert.equal(storage.snapshotBytes(), before);
});

test('MayhemContract epochApply accepts admin-raised max_apply_batch above default schema size', async () => {
  const { admin, provider, user, storage, contract } = await setupLedgerContract();
  await storage.put(`bal/${user.publicKey}/fiat`, seededBalance(user.publicKey, 10_000));
  const raised = await execute(
    contract,
    storage,
    'setParams',
    {
      op: 'set_params',
      submitted_at: 0,
      effective_at: 86_400,
      values: { max_apply_batch: 5_501 },
    },
    admin.publicKey,
    4
  );
  assert.equal(raised.ok, true, raised.message);

  const manyDebits = Array.from({ length: 5_500 }, () => ({
    rail: 'fiat',
    user: user.publicKey,
    au: '1',
  }));
  const applied = await executeEpochApplyFeature(
    contract,
    storage,
    {
      op: 'epoch_apply',
      epoch: 1,
      at: 86_400,
      debits: manyDebits,
      earnings: [{ rail: 'fiat', provider: provider.publicKey, gross_au: '5500' }],
    },
    admin.publicKey
  );
  assert.equal(applied.ok, true, applied.message);
  assert.equal(applied.debited_au, '5500');
  assert.equal(applied.earned_au, '4675');
  assert.equal(applied.fee_au, '825');
});

test('MayhemContract epochApply uses the active admin max_market_usage_entries param', async () => {
  const { admin, provider, user, storage, contract } = await setupLedgerContract();
  const tuned = await execute(
    contract,
    storage,
    'setParams',
    {
      op: 'set_params',
      submitted_at: 0,
      effective_at: 86_400,
      values: { max_market_usage_entries: 1 },
    },
    admin.publicKey,
    4
  );
  assert.equal(tuned.ok, true, tuned.message);

  const before = storage.snapshotBytes();
  const tooManyMarketEntries = await executeEpochApplyFeature(
    contract,
    storage,
    {
      op: 'epoch_apply',
      epoch: 1,
      at: 86_400,
      debits: [{ rail: 'fiat', user: user.publicKey, au: '2' }],
      earnings: [{ rail: 'fiat', provider: provider.publicKey, gross_au: '2' }],
      market_usage: [{}, {}],
    },
    admin.publicKey
  );
  assert.match(tooManyMarketEntries.message, /max_market_usage_entries/i);
  assert.equal(storage.snapshotBytes(), before);
});

test('MayhemContract epochApply paginates settlement entries across free pages', async () => {
  const { admin, provider, user, storage, contract } = await setupLedgerContract();
  await storage.put(`bal/${user.publicKey}/fiat`, seededBalance(user.publicKey, 10_000));
  const firstDebits = Array.from({ length: 1_999 }, () => ({
    rail: 'fiat',
    user: user.publicKey,
    au: '1',
  }));
  const secondDebits = Array.from({ length: 501 }, () => ({
    rail: 'fiat',
    user: user.publicKey,
    au: '1',
  }));

  const firstPage = await executeEpochApplyFeature(
    contract,
    storage,
    {
      op: 'epoch_apply',
      epoch: 1,
      page: 0,
      last_page: false,
      at: 3600,
      debits: firstDebits,
      earnings: [{ rail: 'fiat', provider: provider.publicKey, gross_au: '1999' }],
    },
    admin.publicKey
  );
  assert.equal(firstPage.ok, true, firstPage.message);
  assert.equal(firstPage.page, 0);
  assert.equal(firstPage.last_page, false);
  assert.equal(firstPage.debited_au, '1999');
  let applyState = (await storage.get('epoch/apply/state')).value;
  assert.equal(applyState.updated_epoch, 0);
  assert.equal(applyState.pending_epoch, 1);
  assert.equal(applyState.pending_next_page, 1);

  const secondPage = await executeEpochApplyFeature(
    contract,
    storage,
    {
      op: 'epoch_apply',
      epoch: 1,
      page: 1,
      last_page: true,
      at: 3600,
      debits: secondDebits,
      earnings: [{ rail: 'fiat', provider: provider.publicKey, gross_au: '501' }],
    },
    admin.publicKey
  );
  assert.equal(secondPage.ok, true, secondPage.message);
  assert.equal(secondPage.page, 1);
  assert.equal(secondPage.last_page, true);
  assert.equal(secondPage.debited_au, '501');
  applyState = (await storage.get('epoch/apply/state')).value;
  assert.equal(applyState.updated_epoch, 1);
  assert.equal(applyState.pending_epoch, null);
  assert.equal(applyState.pending_next_page, 0);

  assert.equal((await storage.get(`bal/${user.publicKey}/fiat`)).value.au, '7500');
  assert.equal((await storage.get(`earn/fiat/${provider.publicKey}`)).value.total_au, '2126');
  assert.equal((await storage.get('fee/fiat/cum')).value.cum_au, '374');
});

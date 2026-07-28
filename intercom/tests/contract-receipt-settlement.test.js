import assert from 'node:assert/strict';
import test from 'node:test';
import b4a from 'b4a';
import PeerWallet from 'trac-wallet';
import MayhemContract, {
  CONTRACT_VERSION,
  SESSION_RECEIPT_SCHEMA_VERSION,
  SPEND_VOUCHER_SCHEMA_VERSION,
  closeUsageReservationMessage,
  expireUsageReservationMessage,
  receiptMessage,
  recordUsageReceiptMessage,
  spendVoucherMessage,
  targetedPayoutControlMessage,
  targetedSpendReservationMessage,
} from '../contract/contract.js';
import {
  MemoryStorage,
  execute,
  executeFeature,
  makeIdentity,
  makeTxKey,
  makeVerifier,
  seedCurrentAdminPrice,
  signConsent,
} from './helpers/contract.js';

const RULES_HASH = '31'.repeat(32);
const ENCLAVE_ID = '41'.repeat(32);
const MODEL_ID = 'mayhem/receipt-settlement-test';
const PAYOUT_REVISION = '51'.repeat(32);
const LOCKED_RATE_MAP = [
  { unit: 'input_token', per_unit_au: '10', granularity: 1 },
];

const signHex = (wallet, message) =>
  b4a.toString(wallet.sign(b4a.from(message)), 'hex');

const changedHex = (value) =>
  `${value.slice(0, -1)}${value.endsWith('0') ? '1' : '0'}`;

async function setupContract() {
  const admin = await makeIdentity();
  const provider = await makeIdentity();
  const user = await makeIdentity();
  const enclave = await makeIdentity();
  const submitter = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const contract = new MayhemContract({
    peer: { wallet: makeVerifier(provider.wallet) },
  }, {});

  let result = await execute(
    contract,
    storage,
    'setRules',
    { op: 'set_rules', ver: 1, hash: RULES_HASH },
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
      hash: RULES_HASH,
      sig: signConsent(provider.wallet, 1, RULES_HASH),
    },
    provider.publicKey,
    2
  );
  assert.equal(result.ok, true, result.message);
  result = await execute(
    contract,
    storage,
    'registerProvider',
    { op: 'register_provider' },
    provider.publicKey,
    3
  );
  assert.equal(result.ok, true, result.message);
  result = await execute(
    contract,
    storage,
    'setProviderRails',
    { op: 'set_provider_rails', rails: ['tnk'] },
    provider.publicKey,
    4
  );
  assert.equal(result.ok, true, result.message);

  await storage.put(`bal/${user.publicKey}/tnk`, {
    user: user.publicKey,
    rail: 'tnk',
    denom: 'au_usd',
    au: '100000',
    updated_epoch: 0,
    updated_at: null,
  });
  await storage.put(`enclave/${ENCLAVE_ID}`, {
    enclave_id: ENCLAVE_ID,
    model_id: MODEL_ID,
    model_class: 'text-generation',
    backend: 'llama.cpp',
    artifact_root: '61'.repeat(32),
    artifact_root_kind: 'blake3_merkle_v1',
    artifact_source: 'huggingface://mayhem/receipt-settlement-test.gguf',
    manifest_hash: '62'.repeat(32),
    binary_hash: '63'.repeat(32),
    att_tier: 1,
    caps: {
      chat: true,
      tools: true,
      json: true,
      ctx: 8192,
      ctx_max: 8192,
      modality_set: ['text'],
      speciality_levels: {},
    },
    status: 'active',
    created_by: admin.publicKey,
    created_by_role: 'admin',
    created_at: makeTxKey(5),
    updated_at: makeTxKey(5),
  });
  await storage.put(`serve/${provider.publicKey}/${ENCLAVE_ID}`, {
    provider: provider.publicKey,
    enclave_id: ENCLAVE_ID,
    model_id: MODEL_ID,
    status: 'active',
    served_ctx: 8192,
    served_modalities: ['text'],
    served_specialities: {},
    ctx_bracket: 'le8k',
    ctx_bracket_table_ver: 1,
    joined_at: makeTxKey(6),
    updated_at: makeTxKey(6),
    via: 'feature',
  });
  await seedCurrentAdminPrice(storage, {
    enclaveId: ENCLAVE_ID,
    modelId: MODEL_ID,
    admin: admin.publicKey,
    txNo: 7,
    ver: 1,
    rateMap: LOCKED_RATE_MAP,
    ctxBracket: 'le8k',
    ctxBracketTableVer: 1,
  });
  await storage.put(
    `payout/binding/tnk/${provider.publicKey}/${PAYOUT_REVISION}`,
    {
      type: 'provider_payout_binding',
      provider: provider.publicKey,
      rail: 'tnk',
      revision: PAYOUT_REVISION,
      verified: true,
      activation_epoch: 1,
      target: 'trac1receiptsettlementtest',
      target_wallet: 'trac1receiptsettlementtest',
      currency: null,
      chain_id: null,
    }
  );
  await storage.put(`payout/current/tnk/${provider.publicKey}`, {
    provider: provider.publicKey,
    rail: 'tnk',
    latest_revision: PAYOUT_REVISION,
    current_revision: PAYOUT_REVISION,
    pending_revision: null,
    pending_activation_epoch: null,
    updated_at: makeTxKey(8),
  });
  return { admin, provider, user, enclave, submitter, storage, contract };
}

function reservationValue(ctx, {
  sessionId = '71'.repeat(32),
  billingId = '72'.repeat(32),
  billingAttempt = 0,
  priorUsage = {},
  priorAu = '0',
  epoch = 1,
  reservationId = '73'.repeat(32),
  reservationExpiresAfterEpoch = epoch + 24,
  reservationReceiptGraceEpochs = 6,
  maxSpendAu = '1000',
} = {}) {
  const voucherBody = {
    schema_version: SPEND_VOUCHER_SCHEMA_VERSION,
    session_id: sessionId,
    billing_id: billingId,
    billing_attempt: billingAttempt,
    billing_prior_usage: priorUsage,
    billing_prior_au_owed_cum: priorAu,
    billing_epoch: epoch,
    reservation_id: reservationId,
    reservation_expires_after_epoch: reservationExpiresAfterEpoch,
    reservation_receipt_grace_epochs: reservationReceiptGraceEpochs,
    user: ctx.user.publicKey,
    provider: ctx.provider.publicKey,
    payout_revision: PAYOUT_REVISION,
    rail: 'tnk',
    enclave_id: ENCLAVE_ID,
    model_id: MODEL_ID,
    price_ver: 1,
    locked_rate_map: LOCKED_RATE_MAP,
    locked_per_req_au: '0',
    locked_min_session_au: '0',
    served_ctx: 8192,
    required_modalities: ['text'],
    ctx_bracket: 'le8k',
    ctx_bracket_table_ver: 1,
    rules_ver: 1,
    max_spend_au: maxSpendAu,
    checkpoint_every: { tokens: 128, ms: 1000 },
  };
  const unsigned = {
    op: 'spend_reserve_targeted',
    payout_revision: PAYOUT_REVISION,
    contract_version: CONTRACT_VERSION,
    session_id: sessionId,
    reservation_id: reservationId,
    reservation_expires_after_epoch: reservationExpiresAfterEpoch,
    reservation_receipt_grace_epochs: reservationReceiptGraceEpochs,
    epoch,
    at: epoch * 3600,
    rail: 'tnk',
    user: ctx.user.publicKey,
    provider: ctx.provider.publicKey,
    enclave_id: ENCLAVE_ID,
    enclave_pubkey: ctx.enclave.publicKey,
    model_id: MODEL_ID,
    price_ver: 1,
    rules_ver: 1,
    served_ctx: 8192,
    required_modalities: ['text'],
    ctx_bracket: 'le8k',
    ctx_bracket_table_ver: 1,
    max_spend_au: maxSpendAu,
    voucher: {
      ...voucherBody,
      user_sig: signHex(ctx.user.wallet, spendVoucherMessage(voucherBody)),
    },
    provider_sig: '',
  };
  return {
    ...unsigned,
    provider_sig: signHex(
      ctx.provider.wallet,
      targetedSpendReservationMessage(unsigned)
    ),
  };
}

async function submitReservation(ctx, options = {}) {
  const value = reservationValue(ctx, options);
  const previousStorage = ctx.contract.storage;
  ctx.contract.storage = ctx.storage;
  let key;
  try {
    key = await ctx.contract.targetedSpendReservationFeatureKey(value);
  } finally {
    ctx.contract.storage = previousStorage;
  }
  if (key instanceof Error) {
    return { key: null, value, result: key };
  }
  const result = await executeFeature(
    ctx.contract,
    ctx.storage,
    'mayhem_feature',
    key,
    value,
    ctx.provider.publicKey
  );
  return { key, value, result: result ?? ctx.contract._mayhemLastFeatureResult };
}

function receiptValue(ctx, reservation, {
  seq = 1,
  final = false,
  usage = { input_token: 10 },
  auOwedCum = '100',
  bodyOverrides = {},
  receiptOverrides = {},
  outerOverrides = {},
} = {}) {
  const voucher = reservation.value.voucher;
  const body = {
    schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
    session_id: voucher.session_id,
    billing_id: voucher.billing_id,
    billing_attempt: voucher.billing_attempt,
    billing_prior_usage: voucher.billing_prior_usage,
    billing_prior_au_owed_cum: voucher.billing_prior_au_owed_cum,
    billing_epoch: voucher.billing_epoch,
    reservation_id: voucher.reservation_id,
    reservation_expires_after_epoch: voucher.reservation_expires_after_epoch,
    reservation_receipt_grace_epochs: voucher.reservation_receipt_grace_epochs,
    payout_revision: voucher.payout_revision,
    seq,
    final,
    rail: voucher.rail,
    user: voucher.user,
    provider: voucher.provider,
    enclave_id: voucher.enclave_id,
    model_id: voucher.model_id,
    price_ver: voucher.price_ver,
    locked_rate_map: voucher.locked_rate_map,
    locked_per_req_au: voucher.locked_per_req_au,
    locked_min_session_au: voucher.locked_min_session_au,
    served_ctx: voucher.served_ctx,
    ctx_bracket: voucher.ctx_bracket,
    ctx_bracket_table_ver: voucher.ctx_bracket_table_ver,
    rules_ver: voucher.rules_ver,
    usage,
    au_owed_cum: auOwedCum,
    prompt_hash: '81'.repeat(32),
    ts: 3600 + seq,
    ...bodyOverrides,
  };
  const message = receiptMessage(body);
  const receipt = {
    body,
    enclave_sig: signHex(ctx.enclave.wallet, message),
    enclave_pubkey: ctx.enclave.publicKey,
    user_sig: signHex(ctx.user.wallet, message),
    ...receiptOverrides,
  };
  const unsigned = {
    op: 'record_usage_receipt',
    contract_version: CONTRACT_VERSION,
    epoch: body.billing_epoch,
    payout_revision: body.payout_revision,
    receipt,
    provider_sig: '',
    ...outerOverrides,
  };
  return {
    ...unsigned,
    provider_sig: signHex(
      ctx.provider.wallet,
      recordUsageReceiptMessage(unsigned)
    ),
  };
}

async function submitReceipt(ctx, value) {
  const previousStorage = ctx.contract.storage;
  ctx.contract.storage = ctx.storage;
  let key;
  try {
    key = await ctx.contract.recordUsageReceiptFeatureKey(value);
  } finally {
    ctx.contract.storage = previousStorage;
  }
  if (key instanceof Error) {
    return { key: null, value, result: key };
  }
  const result = await executeFeature(
    ctx.contract,
    ctx.storage,
    'mayhem_feature',
    key,
    value,
    ctx.provider.publicKey
  );
  return { key, value, result: result ?? ctx.contract._mayhemLastFeatureResult };
}

async function commitEpoch(ctx, {
  epoch = 1,
  count,
  useAu,
  txNo = 100,
} = {}) {
  const roots = {
    dep: '91'.repeat(32),
    use: '92'.repeat(32),
    earn: '93'.repeat(32),
    fee: '94'.repeat(32),
    price: '95'.repeat(32),
  };
  const totals = {
    dep_count: 0,
    dep_au: '0',
    use_count: count,
    use_au: useAu,
    provider_count: 1,
    earn_au: '0',
    fee_au: '0',
    fee_cum_au: '0',
    burn_au: '0',
    burn_cum_au: '0',
    price_count: 0,
  };
  const result = await execute(
    ctx.contract,
    ctx.storage,
    'epochCommit',
    { op: 'epoch_commit', epoch, at: epoch * 3600, roots, totals },
    ctx.submitter.publicKey,
    txNo
  );
  assert.equal(result.ok, true, result.message);
  return result;
}

const allocationFor = (ctx, head) => ({
  session_id: head.session_id,
  billing_id: head.billing_id,
  billing_attempt: head.billing_attempt,
  billing_epoch: head.billing_epoch,
  receipt_seq: head.receipt_seq,
  receipt_hash: head.receipt_hash,
  user: head.user,
  rail: head.rail,
  provider: head.provider,
  payout_revision: head.payout_revision,
  au: head.incremental_au,
});

async function targetedApplyValue(ctx, {
  heads,
  commitHash,
  epoch = heads[0].settlement_epoch,
  page = 0,
  lastPage = true,
} = {}) {
  const allocations = heads.map((head) => allocationFor(ctx, head));
  const grossAu = allocations
    .reduce((sum, allocation) => sum + BigInt(allocation.au), 0n)
    .toString();
  return {
    op: 'apply_targeted_epoch',
    epoch,
    at: epoch * 3600,
    epoch_commit_hash: commitHash,
    receipt_index: (await ctx.storage.get(`receipt/epoch/${epoch}/index`)).value,
    debits: [{ rail: 'tnk', user: ctx.user.publicKey, au: grossAu }],
    earnings: [{
      rail: 'tnk',
      provider: ctx.provider.publicKey,
      gross_au: grossAu,
      payout_revision: PAYOUT_REVISION,
    }],
    allocations,
    page,
    last_page: lastPage,
  };
}

function closeValue(ctx, reservation, {
  actorRole = 'provider',
  head = null,
  reason = 'session_closed',
  at = 3700,
  expiry = false,
} = {}) {
  const voucher = reservation.value.voucher;
  const actor = actorRole === 'provider' ? ctx.provider : ctx.user;
  const unsigned = {
    op: expiry ? 'expire_usage_reservation' : 'close_usage_reservation',
    contract_version: CONTRACT_VERSION,
    billing_epoch: voucher.billing_epoch,
    reservation_id: voucher.reservation_id,
    reservation_expires_after_epoch: voucher.reservation_expires_after_epoch,
    reservation_receipt_grace_epochs: voucher.reservation_receipt_grace_epochs,
    billing_id: voucher.billing_id,
    billing_attempt: voucher.billing_attempt,
    session_id: voucher.session_id,
    user: voucher.user,
    rail: voucher.rail,
    provider: voucher.provider,
    payout_revision: voucher.payout_revision,
    latest_receipt_seq: head?.receipt_seq ?? null,
    latest_receipt_hash: head?.receipt_hash ?? null,
    at,
    reason,
    actor: actor.publicKey,
    actor_role: actorRole,
    actor_sig: '',
  };
  return {
    ...unsigned,
    actor_sig: signHex(
      actor.wallet,
      expiry
        ? expireUsageReservationMessage(unsigned)
        : closeUsageReservationMessage(unsigned)
    ),
  };
}

async function submitClose(ctx, value, { expiry = false } = {}) {
  const previousStorage = ctx.contract.storage;
  ctx.contract.storage = ctx.storage;
  let key;
  try {
    key = await ctx.contract.closeUsageReservationFeatureKey(value, { expiry });
  } finally {
    ctx.contract.storage = previousStorage;
  }
  if (key instanceof Error) return { key: null, value, result: key };
  const result = await executeFeature(
    ctx.contract,
    ctx.storage,
    'mayhem_feature',
    key,
    value,
    value.actor
  );
  return { key, value, result: result ?? ctx.contract._mayhemLastFeatureResult };
}

async function submitTargetedApply(ctx, value) {
  const previousStorage = ctx.contract.storage;
  ctx.contract.storage = ctx.storage;
  let key;
  try {
    key = await ctx.contract.targetedEpochFeatureKey(value);
  } finally {
    ctx.contract.storage = previousStorage;
  }
  assert.equal(key instanceof Error, false, key.message);
  const result = await executeFeature(
    ctx.contract,
    ctx.storage,
    'mayhem_feature',
    key,
    value,
    ctx.admin.publicKey
  );
  return { key, result: result ?? ctx.contract._mayhemLastFeatureResult };
}

test('only the provider can immediately close an outstanding reservation', async () => {
  const ctx = await setupContract();
  const reservation = await submitReservation(ctx);
  assert.equal(reservation.result.ok, true, reservation.result.message);

  const buyerClose = await submitClose(
    ctx,
    closeValue(ctx, reservation, { actorRole: 'user' })
  );
  assert.match(
    buyerClose.result.message,
    /invalid close usage reservation|must be signed by its provider/i
  );
  assert.equal(
    (await ctx.storage.get(
      `receipt/reservation/${reservation.value.reservation_id}`
    )).value.status,
    'active'
  );

  const providerCloseValue = closeValue(ctx, reservation);
  const forgedClose = await submitClose(ctx, {
    ...providerCloseValue,
    actor_sig: changedHex(providerCloseValue.actor_sig),
  });
  assert.match(forgedClose.result.message, /signature/i);

  const providerClose = await submitClose(ctx, providerCloseValue);
  assert.equal(providerClose.result.ok, true, providerClose.result.message);
  assert.equal(providerClose.result.settlement_epoch, null);
  assert.equal(providerClose.result.retained_au, '0');
  assert.equal(providerClose.result.released_au, '1000');

  const replay = await submitClose(ctx, providerCloseValue);
  assert.equal(replay.result.ok, true, replay.result.message);
  assert.equal(replay.result.idempotent, true);

  const conflict = await submitClose(
    ctx,
    closeValue(ctx, reservation, { reason: 'different_reason', at: 3701 })
  );
  assert.match(conflict.result.message, /conflicts with its canonical close/i);
});

test('buyer expiry close is unavailable before grace and releases only after canonical expiry', async () => {
  const ctx = await setupContract();
  const reservation = await submitReservation(ctx);
  assert.equal(reservation.result.ok, true, reservation.result.message);
  const expiry = closeValue(ctx, reservation, {
    actorRole: 'user',
    expiry: true,
    reason: 'reservation_expired',
    at: 120_000,
  });

  const premature = await submitClose(ctx, expiry, { expiry: true });
  assert.match(premature.result.message, /not yet past canonical expiry/i);
  assert.equal(
    (
      await ctx.storage.get(
        `receipt/reservation/${reservation.value.reservation_id}`
      )
    ).value.status,
    'active'
  );

  await ctx.storage.put('epoch/apply/state', {
    updated_epoch: 31,
    pending_epoch: null,
    last_apply_hash: 'af'.repeat(32),
    last_settlement_unix: 111_600,
    updated_at: makeTxKey(89),
  });
  const closed = await submitClose(ctx, expiry, { expiry: true });
  assert.equal(closed.result.ok, true, closed.result.message);
  assert.equal(closed.result.released_au, '1000');
  assert.equal(closed.result.retained_au, '0');

  const replay = await submitClose(ctx, expiry, { expiry: true });
  assert.equal(replay.result.ok, true, replay.result.message);
  assert.equal(replay.result.idempotent, true);
});

test('v18 rejects forged receipt signatures and mismatched signed bindings', async () => {
  const ctx = await setupContract();
  const reservation = await submitReservation(ctx);
  assert.equal(reservation.result.ok, true, reservation.result.message);
  const valid = receiptValue(ctx, reservation);

  const forgedProvider = {
    ...valid,
    provider_sig: changedHex(valid.provider_sig),
  };
  assert.match(
    (await submitReceipt(ctx, forgedProvider)).result.message,
    /provider signature/i
  );

  for (const field of ['user_sig', 'enclave_sig']) {
    const receipt = { ...valid.receipt, [field]: changedHex(valid.receipt[field]) };
    const unsigned = { ...valid, receipt, provider_sig: '' };
    const forged = {
      ...unsigned,
      provider_sig: signHex(ctx.provider.wallet, recordUsageReceiptMessage(unsigned)),
    };
    assert.match(
      (await submitReceipt(ctx, forged)).result.message,
      /user or enclave signature/i
    );
  }

  const wrongOuter = receiptValue(ctx, reservation, {
    outerOverrides: { epoch: 2 },
  });
  assert.match(
    (await submitReceipt(ctx, wrongOuter)).result.message,
    /outer epoch/i
  );
  const wrongHold = receiptValue(ctx, reservation, {
    bodyOverrides: { reservation_id: '82'.repeat(32) },
  });
  assert.match(
    (await submitReceipt(ctx, wrongHold)).result.message,
    /exact targeted spend hold/i
  );
  const wrongPayout = receiptValue(ctx, reservation, {
    bodyOverrides: { payout_revision: '83'.repeat(32) },
  });
  assert.match(
    (await submitReceipt(ctx, wrongPayout)).result.message,
    /exact targeted spend hold|terms do not match/i
  );
});

test('v18 receipt verification does not depend on wallet hex coercion', async () => {
  const ctx = await setupContract();
  const reservation = await submitReservation(ctx);
  assert.equal(reservation.result.ok, true, reservation.result.message);
  const receipt = receiptValue(ctx, reservation);

  ctx.contract.protocol.peer.wallet = {
    verify(signature, message, publicKey) {
      if (!b4a.isBuffer(signature) ||
          !b4a.isBuffer(message) ||
          !b4a.isBuffer(publicKey)) {
        return false;
      }
      return PeerWallet.verify(signature, message, publicKey);
    },
  };

  const submitted = await submitReceipt(ctx, receipt);
  assert.equal(submitted.result.ok, true, submitted.result.message);
});

test('v18 receipt verification rejects forged signatures despite a permissive wallet wrapper', async () => {
  const ctx = await setupContract();
  const reservation = await submitReservation(ctx);
  assert.equal(reservation.result.ok, true, reservation.result.message);
  const valid = receiptValue(ctx, reservation);
  ctx.contract.protocol.peer.wallet = { verify: () => true };

  const forged = {
    ...valid,
    provider_sig: changedHex(valid.provider_sig),
  };
  assert.match(
    (await submitReceipt(ctx, forged)).result.message,
    /provider signature/i
  );
});

test('non-final receipt heads stay unindexed until the final head becomes settlement-ready', async () => {
  const ctx = await setupContract();
  const reservation = await submitReservation(ctx);
  const checkpoint = receiptValue(ctx, reservation);
  const first = await submitReceipt(ctx, checkpoint);
  assert.equal(first.result.ok, true, first.result.message);
  assert.equal(first.result.idempotent, false);
  assert.equal(await ctx.storage.get('receipt/epoch/1/index'), null);
  assert.equal(await ctx.storage.get('receipt/epoch/1'), null);
  assert.equal(await ctx.storage.get('receipt/epoch/1/page/0'), null);

  const duplicate = await submitReceipt(ctx, checkpoint);
  assert.equal(duplicate.result.idempotent, true);
  assert.equal(await ctx.storage.get('receipt/epoch/1/index'), null);

  const conflict = receiptValue(ctx, reservation, {
    seq: 1,
    usage: { input_token: 11 },
    auOwedCum: '110',
  });
  assert.match(
    (await submitReceipt(ctx, conflict)).result.message,
    /sequence conflicts/i
  );

  const advanced = receiptValue(ctx, reservation, {
    seq: 2,
    usage: { input_token: 15 },
    auOwedCum: '150',
  });
  const second = await submitReceipt(ctx, advanced);
  assert.equal(second.result.ok, true, second.result.message);
  assert.equal(await ctx.storage.get('receipt/epoch/1/index'), null);
  assert.equal(
    (await ctx.storage.get(`receipt/head/${'72'.repeat(32)}/0`)).value.receipt_seq,
    2
  );

  const final = receiptValue(ctx, reservation, {
    seq: 3,
    final: true,
    usage: { input_token: 20 },
    auOwedCum: '200',
  });
  const finalized = await submitReceipt(ctx, final);
  assert.equal(finalized.result.ok, true, finalized.result.message);
  assert.deepEqual((await ctx.storage.get('receipt/epoch/1/index')).value, {
    type: 'canonical_receipt_epoch_index',
    epoch: 1,
    count: 1,
    page_size: 128,
    page_count: 1,
    revision: 1,
    updated_at: finalized.key,
  });
  assert.deepEqual((await ctx.storage.get('receipt/epoch/1/page/0')).value, {
    type: 'canonical_receipt_epoch_page',
    epoch: 1,
    page: 0,
    identities: [{ billing_id: '72'.repeat(32), billing_attempt: 0 }],
  });
});

test('final receipt heads reject higher sequence updates', async () => {
  const ctx = await setupContract();
  const reservation = await submitReservation(ctx);
  assert.equal(
    (await submitReceipt(ctx, receiptValue(ctx, reservation, { final: true }))).result.ok,
    true
  );
  const higher = receiptValue(ctx, reservation, {
    seq: 2,
    final: true,
    usage: { input_token: 11 },
    auOwedCum: '110',
  });
  assert.match(
    (await submitReceipt(ctx, higher)).result.message,
    /finalized canonical receipt head cannot advance/i
  );
});

test('redispatch attempts must exactly chain and freeze older attempts', async () => {
  const ctx = await setupContract();
  const firstReservation = await submitReservation(ctx);
  const firstReceipt = receiptValue(ctx, firstReservation, {
    seq: 1,
    usage: { input_token: 10 },
    auOwedCum: '100',
  });
  assert.equal((await submitReceipt(ctx, firstReceipt)).result.ok, true);

  const badRedispatch = await submitReservation(ctx, {
    sessionId: '74'.repeat(32),
    billingId: '72'.repeat(32),
    billingAttempt: 1,
    priorUsage: { input_token: 9 },
    priorAu: '90',
    reservationId: '75'.repeat(32),
  });
  assert.match(badRedispatch.result.message, /exactly chain/i);

  const redispatch = await submitReservation(ctx, {
    sessionId: '74'.repeat(32),
    billingId: '72'.repeat(32),
    billingAttempt: 1,
    priorUsage: { input_token: 10 },
    priorAu: '100',
    reservationId: '75'.repeat(32),
  });
  assert.equal(redispatch.result.ok, true, redispatch.result.message);

  const oldAdvance = receiptValue(ctx, firstReservation, {
    seq: 2,
    usage: { input_token: 11 },
    auOwedCum: '110',
  });
  assert.match(
    (await submitReceipt(ctx, oldAdvance)).result.message,
    /older billing attempt/i
  );
  const redispatchReceipt = receiptValue(ctx, redispatch, {
    final: true,
    usage: { input_token: 15 },
    auOwedCum: '150',
  });
  assert.equal((await submitReceipt(ctx, redispatchReceipt)).result.ok, true);
  assert.equal((await ctx.storage.get('receipt/epoch/1/index')).value.count, 1);
});

test('billing identities cannot cross users, rails, or epochs', async () => {
  const ctx = await setupContract();
  const firstReservation = await submitReservation(ctx);
  const firstReceipt = receiptValue(ctx, firstReservation);
  assert.equal((await submitReceipt(ctx, firstReceipt)).result.ok, true);
  await ctx.storage.put('epoch/apply/state', {
    updated_epoch: 1,
    updated_at: makeTxKey(90),
    last_apply_hash: '96'.repeat(32),
  });
  const crossEpoch = await submitReservation(ctx, {
    sessionId: '76'.repeat(32),
    billingId: '72'.repeat(32),
    billingAttempt: 1,
    priorUsage: { input_token: 10 },
    priorAu: '100',
    epoch: 2,
    reservationId: '77'.repeat(32),
  });
  assert.match(crossEpoch.result.message, /cannot move across user, rail, or epoch/i);
});

test('receipt-bound targeted apply consumes a final head once and rejects later advancement', async () => {
  const ctx = await setupContract();
  const reservation = await submitReservation(ctx);
  const receipt = receiptValue(ctx, reservation, { final: true });
  const recorded = await submitReceipt(ctx, receipt);
  assert.equal(recorded.result.ok, true, recorded.result.message);
  const head = (
    await ctx.storage.get(`receipt/head/${receipt.receipt.body.billing_id}/0`)
  ).value;
  const commit = await commitEpoch(ctx, { count: 1, useAu: '100' });
  const applyValue = await targetedApplyValue(ctx, {
    heads: [head],
    commitHash: commit.commit_hash,
  });
  const applied = await submitTargetedApply(ctx, applyValue);
  assert.equal(applied.result.ok, true, applied.result.message);
  assert.equal(applied.result.idempotent, false);
  const consumed = (
    await ctx.storage.get(`receipt/consumed/${head.billing_id}/0`)
  ).value;
  assert.equal(consumed.receipt_hash, head.receipt_hash);

  const replay = await submitTargetedApply(ctx, applyValue);
  assert.equal(replay.result.ok, true, replay.result.message);
  assert.equal(replay.result.idempotent, true);

  const duplicateReceipt = await submitReceipt(ctx, receipt);
  assert.equal(duplicateReceipt.result.idempotent, true);
  const higher = receiptValue(ctx, reservation, {
    seq: 2,
    usage: { input_token: 11 },
    auOwedCum: '110',
  });
  assert.match(
    (await submitReceipt(ctx, higher)).result.message,
    /consumed canonical receipt head cannot advance/i
  );
});

test('targeted payout planning consumes a liability created by targeted epoch apply', async () => {
  const ctx = await setupContract();
  const reservation = await submitReservation(ctx);
  const receipt = receiptValue(ctx, reservation, { final: true });
  const recorded = await submitReceipt(ctx, receipt);
  assert.equal(recorded.result.ok, true, recorded.result.message);
  const head = (
    await ctx.storage.get(`receipt/head/${receipt.receipt.body.billing_id}/0`)
  ).value;
  const commit = await commitEpoch(ctx, { count: 1, useAu: '100' });
  const applyValue = await targetedApplyValue(ctx, {
    heads: [head],
    commitHash: commit.commit_hash,
  });
  const applied = await submitTargetedApply(ctx, applyValue);
  assert.equal(applied.result.ok, true, applied.result.message);

  const liability = (
    await ctx.storage.get(
      `payout/liability/tnk/${ctx.provider.publicKey}/${PAYOUT_REVISION}`
    )
  ).value;
  assert.equal(liability.type, 'provider_payout_liability');
  assert.equal(Object.hasOwn(liability, 'denom'), false);

  const applyState = (await ctx.storage.get('epoch/apply/state')).value;
  const carry = [{
    provider: ctx.provider.publicKey,
    payout_revision: PAYOUT_REVISION,
    liability_au: liability.total_au,
    held_au: liability.held_au,
    payable_au: (
      BigInt(liability.total_au) -
      BigInt(liability.held_au) -
      BigInt(liability.paid_cum_au)
    ).toString(),
    payout_min_au: '1000000000000000000',
    reason: 'held',
  }];
  const fee = (await ctx.storage.get('fee/tnk/cum')).value;
  const outputs = [{
    economic_op_id: 'a1'.repeat(32),
    output_index: 0,
    role: 'operator_fee',
    to: 'trac1receiptsettlementoperator',
    au: fee.cum_au,
    tnk_e18: fee.cum_au,
  }];
  const outputsRoot = await ctx.contract.opaqueHash(
    'mayhem-targeted-payout-epoch-outputs-v1',
    { rail: 'tnk', epoch: 1, outputs }
  );
  const carryRoot = await ctx.contract.opaqueHash(
    'mayhem-targeted-payout-epoch-carry-v1',
    { rail: 'tnk', epoch: 1, carry }
  );
  const unsigned = {
    op: 'prepare_targeted_payout_epoch',
    contract_version: CONTRACT_VERSION,
    rail: 'tnk',
    epoch: 1,
    at: 3600,
    epoch_apply_hash: applyState.last_apply_hash,
    snapshot_signed_length: 1,
    outcome: 'payouts',
    outputs,
    carry,
    outputs_root: outputsRoot,
    carry_root: carryRoot,
    plan_root: await ctx.contract.opaqueHash(
      'mayhem-targeted-payout-epoch-plan-v1',
      {
        rail: 'tnk',
        epoch: 1,
        at: 3600,
        epoch_apply_hash: applyState.last_apply_hash,
        snapshot_signed_length: 1,
        outcome: 'payouts',
        outputs_root: outputsRoot,
        carry_root: carryRoot,
      }
    ),
    admin: ctx.admin.publicKey,
    admin_sig: '',
  };
  const plan = {
    ...unsigned,
    admin_sig: signHex(
      ctx.admin.wallet,
      targetedPayoutControlMessage(unsigned)
    ),
  };
  const previousStorage = ctx.contract.storage;
  ctx.contract.storage = ctx.storage;
  let key;
  try {
    key = await ctx.contract.targetedPayoutEpochFeatureKey(plan);
  } finally {
    ctx.contract.storage = previousStorage;
  }
  assert.equal(key instanceof Error, false, key.message);
  const prepared = await executeFeature(
    ctx.contract,
    ctx.storage,
    'mayhem_feature',
    key,
    plan,
    ctx.admin.publicKey
  );
  const result = prepared ?? ctx.contract._mayhemLastFeatureResult;
  assert.equal(result.ok, true, result.message);
});

test('paged targeted apply is bounded, commit-bound, complete, and idempotent', async () => {
  const ctx = await setupContract();
  const firstReservation = await submitReservation(ctx);
  const secondReservation = await submitReservation(ctx, {
    sessionId: '78'.repeat(32),
    billingId: '79'.repeat(32),
    reservationId: '7a'.repeat(32),
  });
  const firstReceipt = await submitReceipt(
    ctx,
    receiptValue(ctx, firstReservation, { final: true })
  );
  const secondReceipt = await submitReceipt(ctx, receiptValue(ctx, secondReservation, {
    final: true,
    usage: { input_token: 20 },
    auOwedCum: '200',
  }));
  assert.equal(firstReceipt.result.ok, true);
  assert.equal(secondReceipt.result.ok, true);
  const firstHead = (
    await ctx.storage.get(`receipt/head/${firstReservation.value.voucher.billing_id}/0`)
  ).value;
  const secondHead = (
    await ctx.storage.get(`receipt/head/${secondReservation.value.voucher.billing_id}/0`)
  ).value;
  const commit = await commitEpoch(ctx, { count: 2, useAu: '300' });

  const incompleteClose = await targetedApplyValue(ctx, {
    heads: [firstHead],
    commitHash: commit.commit_hash,
    lastPage: true,
  });
  assert.match(
    (await submitTargetedApply(ctx, incompleteClose)).result.message,
    /complete canonical receipt index/i
  );
  const wrongCommit = {
    ...incompleteClose,
    epoch_commit_hash: 'ab'.repeat(32),
    last_page: false,
  };
  assert.match(
    (await submitTargetedApply(ctx, wrongCommit)).result.message,
    /matching provisional epoch commit/i
  );

  const page0 = await targetedApplyValue(ctx, {
    heads: [firstHead],
    commitHash: commit.commit_hash,
    page: 0,
    lastPage: false,
  });
  const firstPage = await submitTargetedApply(ctx, page0);
  assert.equal(firstPage.result.ok, true, firstPage.result.message);
  assert.equal(firstPage.result.idempotent, false);
  const page0Replay = await submitTargetedApply(ctx, page0);
  assert.equal(page0Replay.result.ok, true, page0Replay.result.message);
  assert.equal(page0Replay.result.idempotent, true);
  const pending = (await ctx.storage.get('epoch/apply/state')).value;
  assert.equal(pending.pending_receipt_allocation_count, 1);
  assert.equal(pending.pending_receipt_index_count, 2);
  assert.equal(pending.pending_receipt_index_revision, 2);

  const receiptIndexKey = 'receipt/epoch/1/index';
  const frozenIndex = (await ctx.storage.get(receiptIndexKey)).value;
  await ctx.storage.put(receiptIndexKey, {
    ...frozenIndex,
    revision: frozenIndex.revision + 1,
    updated_at: makeTxKey(101),
  });
  const changedSnapshotPage = await targetedApplyValue(ctx, {
    heads: [secondHead],
    commitHash: commit.commit_hash,
    page: 1,
    lastPage: true,
  });
  assert.match(
    (await submitTargetedApply(ctx, changedSnapshotPage)).result.message,
    /receipt snapshot changed between pages/i
  );
  await ctx.storage.put(receiptIndexKey, frozenIndex);

  const duplicateAcrossPages = await targetedApplyValue(ctx, {
    heads: [firstHead],
    commitHash: commit.commit_hash,
    page: 1,
    lastPage: true,
  });
  assert.match(
    (await submitTargetedApply(ctx, duplicateAcrossPages)).result.message,
    /already consumed/i
  );

  const page1 = await targetedApplyValue(ctx, {
    heads: [secondHead],
    commitHash: commit.commit_hash,
    page: 1,
    lastPage: true,
  });
  const secondPage = await submitTargetedApply(ctx, page1);
  assert.equal(secondPage.result.ok, true, secondPage.result.message);
  assert.equal(secondPage.result.idempotent, false);
  const page1Replay = await submitTargetedApply(ctx, page1);
  assert.equal(page1Replay.result.ok, true, page1Replay.result.message);
  assert.equal(page1Replay.result.idempotent, true);
  const closed = (await ctx.storage.get('epoch/apply/state')).value;
  assert.equal(closed.updated_epoch, 1);
  assert.equal(closed.last_receipt_allocation_count, 2);
});

test('empty epoch seal treats absent metadata as empty and rejects indexed receipts', async () => {
  const empty = await setupContract();
  const sealed = await execute(
    empty.contract,
    empty.storage,
    'epochSealEmpty',
    {
      op: 'epoch_seal_empty',
      epoch: 1,
      at: 3600,
      reason_hash: 'ac'.repeat(32),
    },
    empty.admin.publicKey,
    110
  );
  assert.equal(sealed.ok, true, sealed.message);
  assert.equal(await empty.storage.get('receipt/epoch/1/index'), null);

  const nonempty = await setupContract();
  const reservation = await submitReservation(nonempty);
  assert.equal(
    (await submitReceipt(
      nonempty,
      receiptValue(nonempty, reservation, { final: true })
    )).result.ok,
    true
  );
  const rejected = await execute(
    nonempty.contract,
    nonempty.storage,
    'epochSealEmpty',
    {
      op: 'epoch_seal_empty',
      epoch: 1,
      at: 3600,
      reason_hash: 'ad'.repeat(32),
    },
    nonempty.admin.publicKey,
    111
  );
  assert.match(rejected.message, /canonical receipts exist/i);

  const malformed = await setupContract();
  await malformed.storage.put('receipt/epoch/1/index', {
    type: 'canonical_receipt_epoch_index',
    epoch: 1,
    count: 0,
    page_size: 128,
    page_count: 0,
    revision: 1,
    updated_at: makeTxKey(112),
  });
  const malformedSeal = await execute(
    malformed.contract,
    malformed.storage,
    'epochSealEmpty',
    {
      op: 'epoch_seal_empty',
      epoch: 1,
      at: 3600,
      reason_hash: 'ae'.repeat(32),
    },
    malformed.admin.publicKey,
    112
  );
  assert.match(malformedSeal.message, /canonical receipts exist/i);
});

test('v18 rejects aggregate reservations and aggregate epoch apply', async () => {
  const ctx = await setupContract();
  assert.match(
    (await ctx.contract.normalizeSpendReserveValue({})).message,
    /aggregate spend reservations are disabled/i
  );
  assert.match(
    ctx.contract.validateEpochApplyFeatureValue({}).message,
    /aggregate epoch_apply is disabled/i
  );
});

test('canonical receipt metadata rejects count and revision overflow', async () => {
  const countOverflow = await setupContract();
  await countOverflow.storage.put('receipt/epoch/1/index', {
    type: 'canonical_receipt_epoch_index',
    epoch: 1,
    count: Number.MAX_SAFE_INTEGER,
    page_size: 128,
    page_count: Math.ceil(Number.MAX_SAFE_INTEGER / 128),
    revision: Number.MAX_SAFE_INTEGER,
    updated_at: makeTxKey(120),
  });
  countOverflow.contract.storage = countOverflow.storage;
  const countError = await countOverflow.contract.nextReceiptEpochIndex(
    1,
    'ba'.repeat(32),
    0
  );
  assert.match(countError.message, /count overflow/i);

  const revisionOverflow = await setupContract();
  const indexedReservation = await submitReservation(revisionOverflow);
  assert.equal(
    (await submitReceipt(
      revisionOverflow,
      receiptValue(revisionOverflow, indexedReservation, { final: true })
    )).result.ok,
    true
  );
  const reservation = await submitReservation(revisionOverflow, {
    sessionId: 'bb'.repeat(32),
    billingId: 'bc'.repeat(32),
    reservationId: 'bd'.repeat(32),
  });
  assert.equal(
    (await submitReceipt(
      revisionOverflow,
      receiptValue(revisionOverflow, reservation)
    )).result.ok,
    true
  );
  const metadata = (
    await revisionOverflow.storage.get('receipt/epoch/1/index')
  ).value;
  await revisionOverflow.storage.put('receipt/epoch/1/index', {
    ...metadata,
    revision: Number.MAX_SAFE_INTEGER,
    updated_at: makeTxKey(121),
  });
  const higher = receiptValue(revisionOverflow, reservation, {
    seq: 2,
    final: true,
    usage: { input_token: 11 },
    auOwedCum: '110',
  });
  assert.match(
    (await submitReceipt(revisionOverflow, higher)).result.message,
    /revision overflow/i
  );
});

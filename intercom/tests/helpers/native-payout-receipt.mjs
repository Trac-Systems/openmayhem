// Isolated native counterpart for mixed payout acceptance. Uses the same real
// reservation/receipt contract calls as contract-receipt-settlement.test.js;
// only canonical registration/funding setup is seeded, with generated keys.
import assert from 'node:assert/strict';
import b4a from 'b4a';
import MayhemContract, { CONTRACT_VERSION, SESSION_RECEIPT_SCHEMA_VERSION, SPEND_VOUCHER_SCHEMA_VERSION,
  spendVoucherMessage, targetedSpendReservationMessage, receiptMessage, recordUsageReceiptMessage } from '../../contract/contract.js';
import { MemoryStorage, execute, executeFeature, makeIdentity, makeFeatureOperation, makeTxKey, makeVerifier,
  seedCurrentAdminPrice, signConsent } from './contract.js';
const RULES_HASH = '31'.repeat(32), ENCLAVE_ID = '41'.repeat(32), MODEL_ID = 'mayhem/mixed-payout-test', PAYOUT_REVISION = '51'.repeat(32);
const LOCKED_RATE_MAP = [{ unit: 'input_token', per_unit_au: '2000000000000000000', granularity: 1 }];
const signHex = (wallet, message) => b4a.toString(wallet.sign(b4a.from(message)), 'hex');
export async function nativePayoutFixture(rail) {
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
    { op: 'set_provider_rails', rails: [rail] },
    provider.publicKey,
    4
  );
  assert.equal(result.ok, true, result.message);

  await storage.put(`bal/${user.publicKey}/${rail}`, {
    user: user.publicKey,
    rail,
    denom: 'au_usd',
    au: '100000000000000000000',
    ...(rail === 'tap' ? { chain_id: 1, pool_address: '0x' + '1'.repeat(40) } : {}),
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
  await storage.put(`modelref/${MODEL_ID}`, {
    id: MODEL_ID,
    model_class: 'text-generation',
    rate_map: LOCKED_RATE_MAP,
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
    `payout/binding/${rail}/${provider.publicKey}/${PAYOUT_REVISION}`,
    {
      type: 'provider_payout_binding',
      provider: provider.publicKey,
      rail,
      revision: PAYOUT_REVISION,
      verified: true,
      activation_epoch: 1,
      target: rail === 'fiat' ? 'acct_native_fixture' : rail === 'tap' ? '0x' + '3'.repeat(40) : 'trac1receiptsettlementtest',
      target_wallet: 'trac1receiptsettlementtest',
      currency: rail === 'fiat' ? 'eur' : null,
      chain_id: rail === 'tap' ? 1 : null,
      stripe_processor_revision: '52'.repeat(32),
    }
  );
  await storage.put(`payout/current/${rail}/${provider.publicKey}`, {
    provider: provider.publicKey,
    rail,
    latest_revision: PAYOUT_REVISION,
    current_revision: PAYOUT_REVISION,
    pending_revision: null,
    pending_activation_epoch: null,
    updated_at: makeTxKey(8),
  });
  await storage.put('payments/current', { set_by_role: 'admin', tap: { chain_id: 1, pool_address: '0x' + '1'.repeat(40) } });
  if (rail === 'fiat') {
    const verification = { type: 'stripe_payout_verification', provider: provider.publicKey, target: 'acct_native_fixture',
      revision: '53'.repeat(32), processor_revision: '52'.repeat(32), ready: true };
    await storage.put('payout/stripe-verified/native-fixture', verification);
    await storage.put(contract.providerStripePayoutVerificationTargetKey(provider.publicKey, verification.target),
      { ...verification, record_key: 'payout/stripe-verified/native-fixture' });
  }
  return { admin, provider, user, enclave, submitter, storage, contract, rail };
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
  maxSpendAu = '10000000000000000000',
  workflow = null,
} = {}) {
  const rail = ctx.rail;
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
    rail,
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
    ...(workflow ? { workflow } : {}),
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
    rail,
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
    ...(workflow ? { workflow } : {}),
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

export async function submitNativeReservation(ctx, options = {}) {
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


export function nativeReceiptValue(ctx, reservation, {
  schemaVersion = SESSION_RECEIPT_SCHEMA_VERSION,
  seq = 1,
  final = false,
  usage = { input_token: 1 },
  auOwedCum = '2000000000000000000',
  workflowOutput = null,
  bodyOverrides = {},
  receiptOverrides = {},
  outerOverrides = {},
} = {}) {
  const voucher = reservation.value.voucher;
  const body = {
    schema_version: schemaVersion,
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
    ...(schemaVersion >= 12 ? { compute_ms: 1_000, capacity_slots: 1 } : {}),
    ctx_bracket: voucher.ctx_bracket,
    ctx_bracket_table_ver: voucher.ctx_bracket_table_ver,
    rules_ver: voucher.rules_ver,
    usage,
    au_owed_cum: auOwedCum,
    prompt_hash: '81'.repeat(32),
    ts: 3600 + seq,
    ...(voucher.workflow ? {
      workflow: voucher.workflow,
      workflow_output: workflowOutput ?? {
        output_modalities: ['image'],
        metrics: { image: 1 },
      },
    } : {}),
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

export async function submitNativeReceipt(ctx, value) {
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
  // Exercise the same current-version dispatch the canonical relay writes,
  // including when its nested receipt evidence was signed under v23.
  const operation = makeFeatureOperation('mayhem_feature', key, value, ctx.provider.publicKey);
  operation.value.dispatch.contract_version = CONTRACT_VERSION;
  ctx.contract._mayhemLastFeatureResult = undefined;
  const result = await ctx.contract.execute(operation, ctx.storage);
  return { key, value, result: result ?? ctx.contract._mayhemLastFeatureResult };
}

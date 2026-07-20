import assert from 'node:assert/strict';
import test from 'node:test';
import b4a from 'b4a';
import { keccak256 } from 'ethereum-cryptography/keccak';
import { secp256k1 } from 'ethereum-cryptography/secp256k1';
import MayhemContract, {
  CONTRACT_VERSION,
  SESSION_RECEIPT_SCHEMA_VERSION,
  providerPayoutBindingMessage,
  providerPayoutTargetBindingMessage,
  stripePayoutProcessorRevision,
  stripePayoutVerificationFeatureKey,
  targetedSpendReservationMessage,
} from '../contract/contract.js';
import { recomputeEpoch } from '../scripts/recompute-epoch-roots.mjs';
import './contract-fiat-settlement.test.js';
import './contract-tnk-settlement.test.js';
import {
  MemoryStorage,
  execute,
  executeEpochApplyFeature,
  executeFeature,
  epochApplyFeatureKey,
  makeEthereumIdentity,
  makeIdentity,
  makeTxKey,
  makeVerifier,
  seedCurrentAdminPrice,
  seedSpendHoldsForApply,
  signConsent,
  signSpendVoucher,
} from './helpers/contract.js';

const rulesHash = '9'.repeat(64);
const enclaveId = '7'.repeat(64);
const modelId = 'mayhem/payout-rollup-test@q4';
const payoutLockedRateMap = [
  { unit: 'input_token', per_unit_au: '2000000', granularity: 350 },
  { unit: 'output_token', per_unit_au: '2000000', granularity: 350 },
];
const payoutBootstrap = 'b'.repeat(64);
const payoutPayments = {
  denom: 'au_usd',
  rails: ['fiat', 'tap', 'tnk'],
  fiat: { processor: 'stripe', currencies: ['usd', 'eur'], locale: 'en' },
  tap: {
    chain_id: 61_000,
    token_address: `0x${'1'.repeat(40)}`,
    pool_address: `0x${'2'.repeat(40)}`,
  },
  tnk: {
    network: 'testnet1',
    treasury_address: `testtrac1${'1'.repeat(40)}`,
  },
  ver: 7,
};

const signHex = (wallet, message) =>
  b4a.toString(wallet.sign(b4a.from(message)), 'hex');

const signEthereumPersonalMessage = (identity, message) => {
  const body = b4a.from(message, 'utf8');
  const prefix = b4a.from(`\x19Ethereum Signed Message:\n${body.length}`, 'utf8');
  const signature = secp256k1.sign(
    keccak256(b4a.concat([prefix, body])),
    identity.privateKey,
    { lowS: true }
  );
  const bytes = b4a.alloc(65);
  bytes.set(signature.toCompactRawBytes(), 0);
  bytes[64] = signature.recovery + 27;
  return `0x${b4a.toString(bytes, 'hex')}`;
};

async function payoutBindingValue({
  contract,
  provider,
  targetOwner,
  admin,
  rail,
  contextRevision,
  previousRevision = null,
  nonce = '1'.repeat(64),
  expiresAfterEpoch = 10,
  fiatTarget = 'acct_test_provider',
  fiatCurrency = 'usd',
}) {
  const isTap = rail === 'tap';
  const isFiat = rail === 'fiat';
  const target = isFiat
    ? fiatTarget
    : isTap
      ? targetOwner.address.toLowerCase()
      : contract.msbAddressForPublicKey(targetOwner.publicKey, payoutPayments.tnk.network);
  const intent = {
    op: 'bind_provider_payout',
    network: payoutPayments.tnk.network,
    admin,
    bootstrap: payoutBootstrap,
    context_revision: contextRevision,
    provider: provider.publicKey,
    rail,
    currency: isFiat ? fiatCurrency : null,
    chain_id: isTap ? payoutPayments.tap.chain_id : null,
    target,
    target_wallet: isTap || isFiat ? null : targetOwner.publicKey,
    target_signature: null,
    previous_revision: previousRevision,
    payment_config_version: payoutPayments.ver,
    nonce,
    expires_after_epoch: expiresAfterEpoch,
  };
  intent.target_signature = isFiat
    ? null
    : isTap
      ? signEthereumPersonalMessage(
          targetOwner,
          providerPayoutTargetBindingMessage(intent)
        )
      : signHex(
          targetOwner.wallet,
          providerPayoutTargetBindingMessage(intent)
        );
  const value = {
    op: 'bind_provider_payout',
    intent,
    provider_signature: signHex(
      provider.wallet,
      providerPayoutBindingMessage(intent)
    ),
  };
  const revision = await contract.providerPayoutBindingRevision(intent);
  return {
    value,
    revision,
    key: contract.providerPayoutBindingFeatureKey(rail, provider.publicKey, revision),
  };
}

async function payoutContextValue(
  contract,
  storage,
  admin,
  bootstrap = payoutBootstrap
) {
  const payments = (await storage.get('payments/current')).value;
  return {
    op: 'publish_payout_context',
    network: payments.tnk.network,
    admin,
    bootstrap,
    payment_config_version: payments.ver,
    payment_config_hash: await contract.providerPayoutPaymentConfigHash(payments),
  };
}

async function publishPayoutContext(
  contract,
  storage,
  admin,
  bootstrap = payoutBootstrap
) {
  const value = await payoutContextValue(contract, storage, admin, bootstrap);
  const key = await contract.providerPayoutContextFeatureKey(value);
  contract._mayhemLastFeatureResult = undefined;
  const result = await executeFeature(
    contract,
    storage,
    'mayhem_feature',
    key,
    value,
    admin
  );
  assert.equal((result ?? contract._mayhemLastFeatureResult).ok, true);
  return await contract.providerPayoutContextRevision(value);
}

async function setupBindingContract({ publishContext = true } = {}) {
  const ctx = await setupPayoutContract();
  const payments = await execute(
    ctx.contract,
    ctx.storage,
    'setPayments',
    {
      op: 'set_payments',
      ver: payoutPayments.ver,
      fiat: payoutPayments.fiat,
      tap: payoutPayments.tap,
      tnk: payoutPayments.tnk,
    },
    ctx.admin.publicKey,
    10
  );
  assert.equal(payments.ok, true, payments.message);
  const rails = await execute(
    ctx.contract,
    ctx.storage,
    'setProviderRails',
    { op: 'set_provider_rails', rails: payoutPayments.rails },
    ctx.provider.publicKey,
    11
  );
  assert.equal(rails.ok, true, rails.message);
  const contextRevision = publishContext
    ? await publishPayoutContext(
        ctx.contract,
        ctx.storage,
        ctx.admin.publicKey
      )
    : null;
  return { ...ctx, contextRevision };
}

async function seedTargetedServing(ctx) {
  await ctx.storage.put(`enclave/${enclaveId}`, {
    enclave_id: enclaveId,
    model_id: modelId,
    model_class: 'text-generation',
    backend: 'llama.cpp',
    artifact_root: 'a1'.repeat(32),
    artifact_root_kind: 'blake3_merkle_v1',
    artifact_source: 'huggingface://mayhem/payout-rollup-test.gguf',
    manifest_hash: 'b1'.repeat(32),
    binary_hash: 'c1'.repeat(32),
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
    created_by: ctx.admin.publicKey,
    created_by_role: 'admin',
    created_at: makeTxKey(20),
    updated_at: makeTxKey(20),
  });
  await ctx.storage.put(`serve/${ctx.provider.publicKey}/${enclaveId}`, {
    provider: ctx.provider.publicKey,
    enclave_id: enclaveId,
    model_id: modelId,
    status: 'active',
    served_ctx: 8192,
    served_modalities: ['text'],
    served_specialities: {},
    ctx_bracket: 'le8k',
    ctx_bracket_table_ver: 1,
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
    rateMap: payoutLockedRateMap,
    minSessionAu: 0,
    effectiveAt: 0,
    ctxBracket: 'le8k',
    ctxBracketTableVer: 1,
  });
}

function targetedSpendReservation(ctx, {
  payoutRevision,
  epoch,
  sessionId,
  maxSpendAu = '1000000',
  at = 90_000 + epoch,
}) {
  const voucherBody = {
    session_id: sessionId,
    billing_id: sessionId,
    billing_attempt: 0,
    billing_prior_usage: {},
    billing_prior_au_owed_cum: '0',
    rail: 'fiat',
    enclave_id: enclaveId,
    price_ver: 1,
    locked_rate_map: payoutLockedRateMap,
    locked_per_req_au: '0',
    locked_min_session_au: '0',
    served_ctx: 8192,
    required_modalities: ['text'],
    ctx_bracket: 'le8k',
    ctx_bracket_table_ver: 1,
    max_spend_au: String(maxSpendAu),
    checkpoint_every: { tokens: 8192, ms: 30_000 },
  };
  const unsigned = {
    op: 'spend_reserve_targeted',
    payout_revision: payoutRevision,
    contract_version: CONTRACT_VERSION,
    session_id: sessionId,
    epoch,
    at,
    rail: 'fiat',
    user: ctx.user.publicKey,
    provider: ctx.provider.publicKey,
    enclave_id: enclaveId,
    price_ver: 1,
    rules_ver: 1,
    served_ctx: 8192,
    required_modalities: ['text'],
    ctx_bracket: 'le8k',
    ctx_bracket_table_ver: 1,
    max_spend_au: String(maxSpendAu),
    voucher: {
      ...voucherBody,
      user_sig: signSpendVoucher(ctx.user.wallet, voucherBody),
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

async function executeTargetedSpend(ctx, value, contract = ctx.contract) {
  contract._mayhemLastFeatureResult = undefined;
  const previousStorage = contract.storage;
  contract.storage = ctx.storage;
  let key;
  try {
    key = await contract.targetedSpendReservationFeatureKey(value);
  } finally {
    contract.storage = previousStorage;
  }
  const result = await executeFeature(
    contract,
    ctx.storage,
    'mayhem_feature',
    key,
    value,
    ctx.provider.publicKey
  );
  return {
    key,
    result: result ?? contract._mayhemLastFeatureResult,
  };
}

async function executeTargetedEpoch(ctx, value, contract = ctx.contract) {
  contract._mayhemLastFeatureResult = undefined;
  const key = await contract.targetedEpochFeatureKey(value);
  const result = await executeFeature(
    contract,
    ctx.storage,
    'mayhem_feature',
    key,
    value,
    ctx.admin.publicKey
  );
  return {
    key,
    result: result ?? contract._mayhemLastFeatureResult,
  };
}

async function executePayoutBinding(contract, storage, binding, admin) {
  contract._mayhemLastFeatureResult = undefined;
  const result = await executeFeature(
    contract,
    storage,
    'mayhem_feature',
    binding.key,
    binding.value,
    admin
  );
  return result ?? contract._mayhemLastFeatureResult;
}

async function stripeVerificationValue(ctx, overrides = {}) {
  const contextPointer = (await ctx.storage.get('payout/context/current')).value;
  const currentPointer = (
    await ctx.storage.get(`payout/stripe-verified/current/${ctx.provider.publicKey}`)
  )?.value;
  const value = {
    op: 'verify_stripe_payout',
    provider: ctx.provider.publicKey,
    account_id: 'acct_test_provider',
    account_type: 'express',
    country: 'DE',
    currency: 'usd',
    mode: 'test',
    verification_kind: 'status',
    source_provider: null,
    processor_revision: null,
    previous_verification: currentPointer?.revision ?? null,
    details_submitted: true,
    payouts_enabled: true,
    transfers_enabled: true,
    network: payoutPayments.tnk.network,
    admin: ctx.admin.publicKey,
    bootstrap: payoutBootstrap,
    context_revision: contextPointer.revision,
    payment_config_version: contextPointer.payment_config_version,
    request_nonce: 'a'.repeat(64),
    ...overrides,
  };
  value.processor_revision = await stripePayoutProcessorRevision(value);
  return {
    value,
    key: await stripePayoutVerificationFeatureKey(value),
  };
}

async function executeStripeVerification(ctx, verification, contract = ctx.contract) {
  contract._mayhemLastFeatureResult = undefined;
  const result = await executeFeature(
    contract,
    ctx.storage,
    'mayhem_feature',
    verification.key,
    verification.value,
    ctx.admin.publicKey
  );
  return result ?? contract._mayhemLastFeatureResult;
}

async function setupPayoutContract() {
  const admin = await makeIdentity();
  const provider = await makeIdentity();
  const user = await makeIdentity();
  const submitter = await makeIdentity();
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
      value: { op: 'register_provider' },
      sender: provider.publicKey,
      txNo: 3,
    },
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }

  await storage.put(`bal/${user.publicKey}/fiat`, {
    user: user.publicKey,
    rail: 'fiat',
    denom: 'au_usd',
    au: '5000000',
    updated_epoch: 0,
    updated_at: null,
  });
  return { admin, provider, user, submitter, storage, contract };
}

const epochApply = (epoch, user, provider, grossAu, overrides = {}) => ({
  op: 'epoch_apply',
  epoch,
  at: epoch * 3_600,
  debits: [{ rail: 'fiat', user, au: String(grossAu) }],
  earnings: [{ rail: 'fiat', provider, gross_au: String(grossAu) }],
  ...overrides,
});

const receiptBundle = (user, provider, overrides = {}) => ({
  epoch: 1,
  prior_burn_cum_au: '0',
  params: {
    fee_bps: 1_500,
  },
  deposits: [],
  receipts: [
    {
      schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
      session_id: 'session-payout-rollup-1',
      billing_id: 'd'.repeat(64),
      billing_attempt: 0,
      billing_prior_usage: {},
      billing_prior_au_owed_cum: '0',
      seq: 1,
      final: true,
      rail: 'fiat',
      user,
      provider,
      enclave_id: enclaveId,
      model_id: modelId,
      price_ver: 1,
      locked_rate_map: payoutLockedRateMap,
      locked_per_req_au: '0',
      locked_min_session_au: '0',
      served_ctx: 8192,
      ctx_bracket: 'le8k',
      ctx_bracket_table_ver: 1,
      rules_ver: 1,
      usage: { input_token: 100, output_token: 250 },
      au_owed_cum: '2000000',
      prompt_hash: 'a'.repeat(64),
      ts: 3_600,
      enclave_sig: 'b'.repeat(128),
      user_sig: 'c'.repeat(128),
    },
  ],
  payouts: [],
  ...overrides,
});

test('provider payout context is admin-published, immutable, and version-addressed', async () => {
  const ctx = await setupBindingContract({ publishContext: false });
  const targetOwner = await makeIdentity();
  const contextValue = await payoutContextValue(
    ctx.contract,
    ctx.storage,
    ctx.admin.publicKey
  );
  const unpublishedRevision = await ctx.contract.providerPayoutContextRevision(contextValue);
  const binding = await payoutBindingValue({
    contract: ctx.contract,
    provider: ctx.provider,
    targetOwner,
    admin: ctx.admin.publicKey,
    rail: 'tnk',
    contextRevision: unpublishedRevision,
  });
  const rejected = await executePayoutBinding(
    ctx.contract,
    ctx.storage,
    binding,
    ctx.admin.publicKey
  );
  assert.match(rejected.message, /context is not published/i);
  assert.equal(await ctx.storage.get('payout/context/current'), null);

  const firstRevision = await publishPayoutContext(
    ctx.contract,
    ctx.storage,
    ctx.admin.publicKey
  );
  const firstKey = `payout/context/${payoutPayments.ver}/${firstRevision}`;
  const firstRecord = (await ctx.storage.get(firstKey)).value;
  assert.equal(firstRecord.bootstrap, payoutBootstrap);
  assert.equal(firstRecord.published_by_role, 'admin');

  const duplicate = await executeFeature(
    ctx.contract,
    ctx.storage,
    'mayhem_feature',
    firstKey,
    contextValue,
    ctx.admin.publicKey
  );
  assert.equal((duplicate ?? ctx.contract._mayhemLastFeatureResult).idempotent, true);

  const nextPayments = await execute(
    ctx.contract,
    ctx.storage,
    'setPayments',
    {
      op: 'set_payments',
      ver: payoutPayments.ver + 1,
      fiat: payoutPayments.fiat,
      tap: payoutPayments.tap,
      tnk: payoutPayments.tnk,
    },
    ctx.admin.publicKey,
    12
  );
  assert.equal(nextPayments.ok, true, nextPayments.message);
  const nextRevision = await publishPayoutContext(
    ctx.contract,
    ctx.storage,
    ctx.admin.publicKey,
    'c'.repeat(64)
  );
  assert.notEqual(nextRevision, firstRevision);
  assert.deepEqual((await ctx.storage.get(firstKey)).value, firstRecord);
  assert.equal(
    (await ctx.storage.get('payout/context/current')).value.revision,
    nextRevision
  );

  const stale = await executeFeature(
    ctx.contract,
    ctx.storage,
    'mayhem_feature',
    firstKey,
    contextValue,
    ctx.admin.publicKey
  );
  assert.match(
    (stale ?? ctx.contract._mayhemLastFeatureResult).message,
    /does not match canonical payment configuration|not current/i
  );
});

test('provider payout binding enforces signatures, CAS, nonce, expiry, and idempotency', async () => {
  const ctx = await setupBindingContract();
  const targetOwner = await makeIdentity();
  const first = await payoutBindingValue({
    contract: ctx.contract,
    provider: ctx.provider,
    targetOwner,
    admin: ctx.admin.publicKey,
    rail: 'tnk',
    contextRevision: ctx.contextRevision,
  });
  const accepted = await executePayoutBinding(
    ctx.contract,
    ctx.storage,
    first,
    ctx.admin.publicKey
  );
  assert.equal(accepted.ok, true, accepted.message);
  assert.equal(accepted.activation_epoch, 1);
  assert.equal(accepted.idempotent, false);
  assert.equal(
    (await ctx.storage.get(
      `payout/current/tnk/${ctx.provider.publicKey}`
    )).value.current_revision,
    first.revision
  );

  const replayed = await executePayoutBinding(
    ctx.contract,
    ctx.storage,
    first,
    ctx.admin.publicKey
  );
  assert.equal(replayed.ok, true, replayed.message);
  assert.equal(replayed.idempotent, true);

  const replacementOwner = await makeIdentity();
  const nonceReplay = await payoutBindingValue({
    contract: ctx.contract,
    provider: ctx.provider,
    targetOwner: replacementOwner,
    admin: ctx.admin.publicKey,
    rail: 'tnk',
    contextRevision: ctx.contextRevision,
    previousRevision: first.revision,
    nonce: first.value.intent.nonce,
  });
  assert.match(
    (await executePayoutBinding(
      ctx.contract,
      ctx.storage,
      nonceReplay,
      ctx.admin.publicKey
    )).message,
    /nonce already consumed/i
  );

  const stale = await payoutBindingValue({
    contract: ctx.contract,
    provider: ctx.provider,
    targetOwner: replacementOwner,
    admin: ctx.admin.publicKey,
    rail: 'tnk',
    contextRevision: ctx.contextRevision,
    nonce: '2'.repeat(64),
  });
  assert.match(
    (await executePayoutBinding(
      ctx.contract,
      ctx.storage,
      stale,
      ctx.admin.publicKey
    )).message,
    /revision is stale/i
  );

  const expired = await payoutBindingValue({
    contract: ctx.contract,
    provider: ctx.provider,
    targetOwner: replacementOwner,
    admin: ctx.admin.publicKey,
    rail: 'tnk',
    contextRevision: ctx.contextRevision,
    previousRevision: first.revision,
    nonce: '3'.repeat(64),
    expiresAfterEpoch: 0,
  });
  assert.match(
    (await executePayoutBinding(
      ctx.contract,
      ctx.storage,
      expired,
      ctx.admin.publicKey
    )).message,
    /expiry epoch/i
  );

  const substituted = structuredClone(first);
  substituted.value.intent.target = ctx.contract.msbAddressForPublicKey(
    replacementOwner.publicKey,
    payoutPayments.tnk.network
  );
  substituted.value.provider_signature = signHex(
    ctx.provider.wallet,
    providerPayoutBindingMessage(substituted.value.intent)
  );
  substituted.revision = await ctx.contract.providerPayoutBindingRevision(
    substituted.value.intent
  );
  substituted.key = ctx.contract.providerPayoutBindingFeatureKey(
    'tnk',
    ctx.provider.publicKey,
    substituted.revision
  );
  assert.match(
    (await executePayoutBinding(
      ctx.contract,
      ctx.storage,
      substituted,
      ctx.admin.publicKey
    )).message,
    /target does not match target wallet|ownership signature/i
  );

  const crossRail = structuredClone(first);
  crossRail.value.intent.rail = 'tap';
  crossRail.value.provider_signature = signHex(
    ctx.provider.wallet,
    providerPayoutBindingMessage(crossRail.value.intent)
  );
  crossRail.revision = await ctx.contract.providerPayoutBindingRevision(
    crossRail.value.intent
  );
  crossRail.key = ctx.contract.providerPayoutBindingFeatureKey(
    'tap',
    ctx.provider.publicKey,
    crossRail.revision
  );
  assert.match(
    (await executePayoutBinding(
      ctx.contract,
      ctx.storage,
      crossRail,
      ctx.admin.publicKey
    )).message,
    /TAP payout binding|TAP payout target/i
  );

  const crossProvider = structuredClone(first);
  crossProvider.value.intent.provider = ctx.submitter.publicKey;
  crossProvider.revision = await ctx.contract.providerPayoutBindingRevision(
    crossProvider.value.intent
  );
  crossProvider.key = ctx.contract.providerPayoutBindingFeatureKey(
    'tnk',
    ctx.submitter.publicKey,
    crossProvider.revision
  );
  assert.match(
    (await executePayoutBinding(
      ctx.contract,
      ctx.storage,
      crossProvider,
      ctx.admin.publicKey
    )).message,
    /signature|registration/i
  );

  const providerRecord = (await ctx.storage.get(`prov/${ctx.provider.publicKey}`)).value;
  await ctx.storage.put(`prov/${ctx.provider.publicKey}`, {
    ...providerRecord,
    status: 'stopped',
  });
  assert.equal(
    (await executePayoutBinding(
      ctx.contract,
      ctx.storage,
      first,
      ctx.admin.publicKey
    )).idempotent,
    true
  );
  await ctx.storage.put(`prov/${ctx.provider.publicKey}`, providerRecord);

  const contextPointer = (await ctx.storage.get('payout/context/current')).value;
  await ctx.storage.put('payout/context/current', {
    ...contextPointer,
    revision: 'f'.repeat(64),
    record_key: `payout/context/8/${'f'.repeat(64)}`,
  });
  assert.equal(
    (await executePayoutBinding(
      ctx.contract,
      ctx.storage,
      first,
      ctx.admin.publicKey
    )).idempotent,
    true
  );
  await ctx.storage.put('payout/context/current', contextPointer);

  await ctx.storage.put('epoch/apply/state', {
    updated_epoch: first.value.intent.expires_after_epoch,
    updated_at: 'expired',
    last_apply_hash: 'f'.repeat(64),
  });
  assert.equal(
    (await executePayoutBinding(
      ctx.contract,
      ctx.storage,
      first,
      ctx.admin.publicKey
    )).idempotent,
    true
  );
});

test('shared TAP and TNK targets require a fresh wallet co-signature per provider', async () => {
  const ctx = await setupBindingContract();
  const secondProvider = await makeIdentity();
  for (const op of [
    {
      type: 'consent',
      value: {
        op: 'consent',
        ver: 1,
        hash: rulesHash,
        sig: signConsent(secondProvider.wallet, 1, rulesHash),
      },
      sender: secondProvider.publicKey,
      txNo: 30,
    },
    {
      type: 'registerProvider',
      value: { op: 'register_provider' },
      sender: secondProvider.publicKey,
      txNo: 31,
    },
    {
      type: 'setProviderRails',
      value: { op: 'set_provider_rails', rails: payoutPayments.rails },
      sender: secondProvider.publicKey,
      txNo: 32,
    },
  ]) {
    const result = await execute(
      ctx.contract,
      ctx.storage,
      op.type,
      op.value,
      op.sender,
      op.txNo
    );
    assert.equal(result.ok, true, result.message);
  }

  const tnkTarget = await makeIdentity();
  const tapTarget = makeEthereumIdentity();
  const bindings = [];
  for (const [provider, rail, targetOwner, nonce] of [
    [ctx.provider, 'tnk', tnkTarget, '7'.repeat(64)],
    [secondProvider, 'tnk', tnkTarget, '8'.repeat(64)],
    [ctx.provider, 'tap', tapTarget, '9'.repeat(64)],
    [secondProvider, 'tap', tapTarget, 'a'.repeat(64)],
  ]) {
    const binding = await payoutBindingValue({
      contract: ctx.contract,
      provider,
      targetOwner,
      admin: ctx.admin.publicKey,
      rail,
      contextRevision: ctx.contextRevision,
      nonce,
    });
    const accepted = await executePayoutBinding(
      ctx.contract,
      ctx.storage,
      binding,
      ctx.admin.publicKey
    );
    assert.equal(accepted.ok, true, accepted.message);
    bindings.push(binding);
  }

  assert.equal(bindings[0].value.intent.target, bindings[1].value.intent.target);
  assert.equal(bindings[2].value.intent.target, bindings[3].value.intent.target);
  assert.notEqual(bindings[0].revision, bindings[1].revision);
  assert.notEqual(bindings[2].revision, bindings[3].revision);

  const substitutedProvider = structuredClone(bindings[2]);
  substitutedProvider.value.intent.provider = secondProvider.publicKey;
  substitutedProvider.value.intent.previous_revision = bindings[3].revision;
  substitutedProvider.value.intent.nonce = 'b'.repeat(64);
  substitutedProvider.value.provider_signature = signHex(
    secondProvider.wallet,
    providerPayoutBindingMessage(substitutedProvider.value.intent)
  );
  substitutedProvider.revision = await ctx.contract.providerPayoutBindingRevision(
    substitutedProvider.value.intent
  );
  substitutedProvider.key = ctx.contract.providerPayoutBindingFeatureKey(
    'tap',
    secondProvider.publicKey,
    substitutedProvider.revision
  );
  assert.match(
    (await executePayoutBinding(
      ctx.contract,
      ctx.storage,
      substitutedProvider,
      ctx.admin.publicKey
    )).message,
    /target ownership signature/i
  );

  const modelScoped = structuredClone(bindings[1]);
  modelScoped.value.intent.model_id = modelId;
  assert.match(
    (await executePayoutBinding(
      ctx.contract,
      ctx.storage,
      modelScoped,
      ctx.admin.publicKey
    )).message,
    /does not accept fields/i
  );
});

test('Stripe verification is immutable, CAS-ordered, and restart-idempotent', async () => {
  const ctx = await setupBindingContract();
  const delayed = await stripeVerificationValue(ctx, {
    account_id: 'acct_delayed_standard',
    account_type: 'standard',
    verification_kind: 'relink',
    source_provider: ctx.submitter.publicKey,
    request_nonce: 'b'.repeat(64),
  });
  const newer = await stripeVerificationValue(ctx, {
    account_id: 'acct_newer_express',
    request_nonce: 'c'.repeat(64),
  });
  const applied = await executeStripeVerification(ctx, newer);
  assert.equal(applied.ok, true, applied.message);
  assert.equal(applied.idempotent, false);

  const outOfOrder = await executeStripeVerification(ctx, delayed);
  assert.match(outOfOrder.message, /revision is stale/i);
  assert.equal(
    (await ctx.storage.get(
      `payout/stripe-verified/current/${ctx.provider.publicKey}`
    )).value.revision,
    applied.revision
  );

  const restarted = new MayhemContract(
    { peer: { wallet: makeVerifier(ctx.provider.wallet) } },
    {}
  );
  const replayed = await executeStripeVerification(ctx, newer, restarted);
  assert.equal(replayed.ok, true, replayed.message);
  assert.equal(replayed.idempotent, true);

  const providerRecord = (await ctx.storage.get(`prov/${ctx.provider.publicKey}`)).value;
  await ctx.storage.put(`prov/${ctx.provider.publicKey}`, {
    ...providerRecord,
    status: 'stopped',
  });
  assert.equal((await executeStripeVerification(ctx, newer)).idempotent, true);
  await ctx.storage.put(`prov/${ctx.provider.publicKey}`, providerRecord);

  const contextPointer = (await ctx.storage.get('payout/context/current')).value;
  await ctx.storage.put('payout/context/current', {
    ...contextPointer,
    revision: 'e'.repeat(64),
  });
  assert.equal((await executeStripeVerification(ctx, newer)).idempotent, true);
  await ctx.storage.put('payout/context/current', contextPointer);

  const nonceCollision = await stripeVerificationValue(ctx, {
    account_id: 'acct_collision',
    previous_verification: applied.revision,
    request_nonce: newer.value.request_nonce,
  });
  assert.match(
    (await executeStripeVerification(ctx, nonceCollision)).message,
    /nonce already consumed/i
  );
});

test('Stripe readiness deactivates and reactivates fiat admission without changing liabilities', async () => {
  const ctx = await setupBindingContract();
  await seedTargetedServing(ctx);
  const ready = await stripeVerificationValue(ctx, {
    account_id: 'acct_readiness_lifecycle',
    request_nonce: '1'.repeat(64),
  });
  assert.equal((await executeStripeVerification(ctx, ready)).ok, true);
  const binding = await payoutBindingValue({
    contract: ctx.contract,
    provider: ctx.provider,
    targetOwner: ctx.provider,
    admin: ctx.admin.publicKey,
    rail: 'fiat',
    contextRevision: ctx.contextRevision,
    fiatTarget: ready.value.account_id,
    nonce: '2'.repeat(64),
  });
  assert.equal(
    (await executePayoutBinding(
      ctx.contract,
      ctx.storage,
      binding,
      ctx.admin.publicKey
    )).ok,
    true
  );
  const immutableBinding = structuredClone(
    (await ctx.storage.get(binding.key)).value
  );

  const unready = await stripeVerificationValue(ctx, {
    account_id: ready.value.account_id,
    details_submitted: true,
    payouts_enabled: false,
    transfers_enabled: true,
    request_nonce: '3'.repeat(64),
  });
  ctx.contract._mayhemLastFeatureResult = undefined;
  const forged = await executeFeature(
    ctx.contract,
    ctx.storage,
    'mayhem_feature',
    unready.key,
    unready.value,
    ctx.provider.publicKey
  );
  assert.match(
    (forged ?? ctx.contract._mayhemLastFeatureResult).message,
    /admin/i
  );
  assert.equal((await executeStripeVerification(ctx, unready)).ok, true);
  assert.equal(
    (await ctx.storage.get(
      `payout/stripe-verified/current/${ctx.provider.publicKey}`
    )).value.ready,
    false
  );
  assert.deepEqual((await ctx.storage.get(binding.key)).value, immutableBinding);

  const whileUnready = targetedSpendReservation(ctx, {
    payoutRevision: binding.revision,
    epoch: 1,
    sessionId: '8'.repeat(64),
  });
  assert.match(
    (await executeTargetedSpend(ctx, whileUnready)).result.message,
    /not currently ready/i
  );

  const readyAgain = await stripeVerificationValue(ctx, {
    account_id: ready.value.account_id,
    request_nonce: '4'.repeat(64),
  });
  assert.equal((await executeStripeVerification(ctx, readyAgain)).ok, true);
  const afterRecovery = targetedSpendReservation(ctx, {
    payoutRevision: binding.revision,
    epoch: 1,
    sessionId: '9'.repeat(64),
  });
  assert.equal(
    (await executeTargetedSpend(ctx, afterRecovery)).result.ok,
    true
  );
  assert.deepEqual((await ctx.storage.get(binding.key)).value, immutableBinding);
});

test('Stripe payout rotation freezes reserved and earned liabilities by revision across restart', async () => {
  const ctx = await setupBindingContract();
  await seedTargetedServing(ctx);
  const params = await execute(
    ctx.contract,
    ctx.storage,
    'setParams',
    {
      op: 'set_params',
      ver: 1,
      values: {
        holdback_epochs: 0,
        new_provider_holdback_epochs: 0,
        challenge_epochs: 0,
      },
      submitted_at: 0,
      effective_at: 86_400,
    },
    ctx.admin.publicKey,
    23
  );
  assert.equal(params.ok, true, params.message);

  const firstVerification = await stripeVerificationValue(ctx, {
    account_id: 'acct_original_express',
    request_nonce: 'd'.repeat(64),
  });
  assert.equal((await executeStripeVerification(ctx, firstVerification)).ok, true);
  const firstBinding = await payoutBindingValue({
    contract: ctx.contract,
    provider: ctx.provider,
    targetOwner: ctx.provider,
    admin: ctx.admin.publicKey,
    rail: 'fiat',
    contextRevision: ctx.contextRevision,
    fiatTarget: 'acct_original_express',
    nonce: '1'.repeat(64),
  });
  const firstBound = await executePayoutBinding(
    ctx.contract,
    ctx.storage,
    firstBinding,
    ctx.admin.publicKey
  );
  assert.equal(firstBound.ok, true, firstBound.message);
  assert.equal(firstBound.activation_epoch, 1);

  const nextVerification = await stripeVerificationValue(ctx, {
    account_id: 'acct_relinked_standard',
    account_type: 'standard',
    verification_kind: 'relink',
    source_provider: ctx.submitter.publicKey,
    request_nonce: 'e'.repeat(64),
  });
  assert.equal((await executeStripeVerification(ctx, nextVerification)).ok, true);
  assert.equal(
    (await ctx.storage.get(
      ctx.contract.providerStripePayoutVerificationTargetKey(
        ctx.provider.publicKey,
        firstVerification.value.account_id
      )
    )).value.revision,
    firstVerification.key.split('/').at(-1)
  );
  assert.equal(
    (await ctx.storage.get(
      ctx.contract.providerStripePayoutVerificationTargetKey(
        ctx.provider.publicKey,
        nextVerification.value.account_id
      )
    )).value.revision,
    nextVerification.key.split('/').at(-1)
  );

  const reservedWhileReplacementVerifies = targetedSpendReservation(ctx, {
    payoutRevision: firstBinding.revision,
    epoch: 1,
    sessionId: 'a'.repeat(64),
  });
  const firstReserve = await executeTargetedSpend(
    ctx,
    reservedWhileReplacementVerifies
  );
  assert.equal(firstReserve.result.ok, true, firstReserve.result.message);

  const nextBinding = await payoutBindingValue({
    contract: ctx.contract,
    provider: ctx.provider,
    targetOwner: ctx.provider,
    admin: ctx.admin.publicKey,
    rail: 'fiat',
    contextRevision: ctx.contextRevision,
    previousRevision: firstBinding.revision,
    fiatTarget: 'acct_relinked_standard',
    nonce: '2'.repeat(64),
  });
  const accountSubstitution = structuredClone(nextBinding);
  accountSubstitution.value.intent.target = firstVerification.value.account_id;
  accountSubstitution.revision = await ctx.contract.providerPayoutBindingRevision(
    accountSubstitution.value.intent
  );
  accountSubstitution.key = ctx.contract.providerPayoutBindingFeatureKey(
    'fiat',
    ctx.provider.publicKey,
    accountSubstitution.revision
  );
  assert.match(
    (await executePayoutBinding(
      ctx.contract,
      ctx.storage,
      accountSubstitution,
      ctx.admin.publicKey
    )).message,
    /signature/i
  );

  const rotated = await executePayoutBinding(
    ctx.contract,
    ctx.storage,
    nextBinding,
    ctx.admin.publicKey
  );
  assert.equal(rotated.ok, true, rotated.message);
  assert.equal(rotated.activation_epoch, 2);

  const premature = targetedSpendReservation(ctx, {
    payoutRevision: nextBinding.revision,
    epoch: 1,
    sessionId: 'b'.repeat(64),
  });
  assert.match(
    (await executeTargetedSpend(ctx, premature)).result.message,
    /not active|revision is not active/i
  );

  const firstEpoch = {
    op: 'apply_targeted_epoch',
    epoch: 1,
    at: 90_001,
    debits: [{ rail: 'fiat', user: ctx.user.publicKey, au: '1000000' }],
    earnings: [{
      rail: 'fiat',
      provider: ctx.provider.publicKey,
      gross_au: '1000000',
      payout_revision: firstBinding.revision,
    }],
    allocations: [{
      session_id: 'a'.repeat(64),
      user: ctx.user.publicKey,
      rail: 'fiat',
      provider: ctx.provider.publicKey,
      payout_revision: firstBinding.revision,
      au: '1000000',
    }],
  };
  const firstApplied = await executeTargetedEpoch(ctx, firstEpoch);
  assert.equal(firstApplied.result.ok, true, firstApplied.result.message);

  const restarted = new MayhemContract(
    { peer: { wallet: makeVerifier(ctx.provider.wallet) } },
    {}
  );
  const replayedRotation = await executePayoutBinding(
    restarted,
    ctx.storage,
    nextBinding,
    ctx.admin.publicKey
  );
  assert.equal(replayedRotation.ok, true, replayedRotation.message);
  assert.equal(replayedRotation.idempotent, true);

  const oldAfterActivation = targetedSpendReservation(ctx, {
    payoutRevision: firstBinding.revision,
    epoch: 2,
    sessionId: 'd'.repeat(64),
  });
  assert.match(
    (await executeTargetedSpend(ctx, oldAfterActivation, restarted)).result.message,
    /revision is not active/i
  );

  const reservedAfterRotation = targetedSpendReservation(ctx, {
    payoutRevision: nextBinding.revision,
    epoch: 2,
    sessionId: 'c'.repeat(64),
  });
  const secondReserve = await executeTargetedSpend(
    ctx,
    reservedAfterRotation,
    restarted
  );
  assert.equal(secondReserve.result.ok, true, secondReserve.result.message);
  const secondEpoch = {
    op: 'apply_targeted_epoch',
    epoch: 2,
    at: 90_002,
    debits: [{ rail: 'fiat', user: ctx.user.publicKey, au: '1000000' }],
    earnings: [{
      rail: 'fiat',
      provider: ctx.provider.publicKey,
      gross_au: '1000000',
      payout_revision: nextBinding.revision,
    }],
    allocations: [{
      session_id: 'c'.repeat(64),
      user: ctx.user.publicKey,
      rail: 'fiat',
      provider: ctx.provider.publicKey,
      payout_revision: nextBinding.revision,
      au: '1000000',
    }],
  };
  const secondApplied = await executeTargetedEpoch(ctx, secondEpoch, restarted);
  assert.equal(secondApplied.result.ok, true, secondApplied.result.message);

  const firstLiability = (
    await ctx.storage.get(
      `payout/liability/fiat/${ctx.provider.publicKey}/${firstBinding.revision}`
    )
  ).value;
  const secondLiability = (
    await ctx.storage.get(
      `payout/liability/fiat/${ctx.provider.publicKey}/${nextBinding.revision}`
    )
  ).value;
  assert.equal(firstLiability.target, 'acct_original_express');
  assert.equal(firstLiability.revision, firstBinding.revision);
  assert.equal(firstLiability.total_au, '850000');
  assert.equal(secondLiability.target, 'acct_relinked_standard');
  assert.equal(secondLiability.revision, nextBinding.revision);
  assert.equal(secondLiability.total_au, '850000');
  assert.equal(
    (await ctx.storage.get(
      `hold/fiat/${ctx.user.publicKey}/1`
    )).value.sessions[0].payout_revision,
    firstBinding.revision
  );
  assert.equal(
    (await ctx.storage.get(
      `hold/fiat/${ctx.user.publicKey}/2`
    )).value.sessions[0].payout_revision,
    nextBinding.revision
  );
});

test('targeted epoch rejects cross-provider session amount substitution', async () => {
  const ctx = await setupBindingContract();
  const providerB = await makeIdentity();
  for (const [type, value, sender, txNo] of [
    [
      'consent',
      {
        op: 'consent',
        ver: 1,
        hash: rulesHash,
        sig: signConsent(providerB.wallet, 1, rulesHash),
      },
      providerB.publicKey,
      30,
    ],
    ['registerProvider', { op: 'register_provider' }, providerB.publicKey, 31],
    [
      'setProviderRails',
      { op: 'set_provider_rails', rails: payoutPayments.rails },
      providerB.publicKey,
      32,
    ],
  ]) {
    const result = await execute(ctx.contract, ctx.storage, type, value, sender, txNo);
    assert.equal(result.ok, true, result.message);
  }
  const providers = [
    { ...ctx, provider: ctx.provider, account: 'acct_small_provider' },
    { ...ctx, provider: providerB, account: 'acct_large_provider' },
  ];
  for (const [index, providerCtx] of providers.entries()) {
    await seedTargetedServing(providerCtx);
    const verification = await stripeVerificationValue(providerCtx, {
      account_id: providerCtx.account,
      request_nonce: (index + 4).toString(16).repeat(64),
    });
    assert.equal(
      (await executeStripeVerification(providerCtx, verification)).ok,
      true
    );
    providerCtx.binding = await payoutBindingValue({
      contract: ctx.contract,
      provider: providerCtx.provider,
      targetOwner: providerCtx.provider,
      admin: ctx.admin.publicKey,
      rail: 'fiat',
      contextRevision: ctx.contextRevision,
      fiatTarget: providerCtx.account,
      nonce: (index + 6).toString(16).repeat(64),
    });
    const bound = await executePayoutBinding(
      ctx.contract,
      ctx.storage,
      providerCtx.binding,
      ctx.admin.publicKey
    );
    assert.equal(bound.ok, true, bound.message);
  }
  const small = targetedSpendReservation(providers[0], {
    payoutRevision: providers[0].binding.revision,
    epoch: 1,
    sessionId: 'a'.repeat(64),
    maxSpendAu: '1000000',
  });
  const large = targetedSpendReservation(providers[1], {
    payoutRevision: providers[1].binding.revision,
    epoch: 1,
    sessionId: 'b'.repeat(64),
    maxSpendAu: '4000000',
  });
  assert.equal((await executeTargetedSpend(providers[0], small)).result.ok, true);
  assert.equal((await executeTargetedSpend(providers[1], large)).result.ok, true);

  const earnings = providers
    .map((providerCtx, index) => ({
      rail: 'fiat',
      provider: providerCtx.provider.publicKey,
      gross_au: index === 0 ? '4000000' : '1000000',
      payout_revision: providerCtx.binding.revision,
    }))
    .sort((left, right) => left.provider.localeCompare(right.provider));
  const substituted = {
    op: 'apply_targeted_epoch',
    epoch: 1,
    at: 90_001,
    debits: [{ rail: 'fiat', user: ctx.user.publicKey, au: '5000000' }],
    earnings,
    allocations: [
      {
        session_id: 'a'.repeat(64),
        user: ctx.user.publicKey,
        rail: 'fiat',
        provider: providers[0].provider.publicKey,
        payout_revision: providers[0].binding.revision,
        au: '4000000',
      },
      {
        session_id: 'b'.repeat(64),
        user: ctx.user.publicKey,
        rail: 'fiat',
        provider: providers[1].provider.publicKey,
        payout_revision: providers[1].binding.revision,
        au: '1000000',
      },
    ],
  };
  const rejected = await executeTargetedEpoch(ctx, substituted);
  assert.match(rejected.result.message, /exceeds its session reservation/i);
  assert.equal((await ctx.storage.get('epoch/apply/state')).value.updated_epoch, 0);
  assert.equal(
    await ctx.storage.get(`payout/allocation/1/${'a'.repeat(64)}`),
    null
  );
});

test('payout expiry limit is admin-scheduled and changes exactly at its epoch boundary', async () => {
  const ctx = await setupBindingContract();
  const firstOwner = await makeIdentity();
  const first = await payoutBindingValue({
    contract: ctx.contract,
    provider: ctx.provider,
    targetOwner: firstOwner,
    admin: ctx.admin.publicKey,
    rail: 'tnk',
    contextRevision: ctx.contextRevision,
    expiresAfterEpoch: 100,
  });
  assert.equal(
    (await executePayoutBinding(
      ctx.contract,
      ctx.storage,
      first,
      ctx.admin.publicKey
    )).ok,
    true
  );
  const schedule = {
    op: 'schedule_payout_parameter',
    key: 'payout_intent_max_expiry_epochs',
    value: 2,
    effective_epoch: 2,
  };
  const scheduleKey = await ctx.contract.payoutParameterFeatureKey(schedule);
  const scheduled = await executeFeature(
    ctx.contract,
    ctx.storage,
    'mayhem_feature',
    scheduleKey,
    schedule,
    ctx.admin.publicKey
  );
  assert.equal((scheduled ?? ctx.contract._mayhemLastFeatureResult).ok, true);

  await ctx.storage.put('epoch/apply/state', {
    updated_epoch: 1,
    updated_at: 'e1',
    last_apply_hash: 'a'.repeat(64),
  });
  const beforeBoundary = await payoutBindingValue({
    contract: ctx.contract,
    provider: ctx.provider,
    targetOwner: await makeIdentity(),
    admin: ctx.admin.publicKey,
    rail: 'tnk',
    contextRevision: ctx.contextRevision,
    previousRevision: first.revision,
    nonce: '4'.repeat(64),
    expiresAfterEpoch: 100,
  });
  const before = await executePayoutBinding(
    ctx.contract,
    ctx.storage,
    beforeBoundary,
    ctx.admin.publicKey
  );
  assert.equal(before.ok, true, before.message);

  await ctx.storage.put('epoch/apply/state', {
    updated_epoch: 2,
    updated_at: 'e2',
    last_apply_hash: 'b'.repeat(64),
  });
  const beyondLimit = await payoutBindingValue({
    contract: ctx.contract,
    provider: ctx.provider,
    targetOwner: await makeIdentity(),
    admin: ctx.admin.publicKey,
    rail: 'tnk',
    contextRevision: ctx.contextRevision,
    previousRevision: beforeBoundary.revision,
    nonce: '5'.repeat(64),
    expiresAfterEpoch: 5,
  });
  assert.match(
    (await executePayoutBinding(
      ctx.contract,
      ctx.storage,
      beyondLimit,
      ctx.admin.publicKey
    )).message,
    /too far in the future/i
  );
  const atLimit = await payoutBindingValue({
    contract: ctx.contract,
    provider: ctx.provider,
    targetOwner: await makeIdentity(),
    admin: ctx.admin.publicKey,
    rail: 'tnk',
    contextRevision: ctx.contextRevision,
    previousRevision: beforeBoundary.revision,
    nonce: '6'.repeat(64),
    expiresAfterEpoch: 4,
  });
  assert.equal(
    (await executePayoutBinding(
      ctx.contract,
      ctx.storage,
      atLimit,
      ctx.admin.publicKey
    )).ok,
    true
  );
});

test('many payout rotations keep a bounded pointer and immutable revision records', async () => {
  const ctx = await setupBindingContract();
  let previousRevision = null;
  const revisions = [];
  for (let index = 1; index <= 128; index += 1) {
    const binding = await payoutBindingValue({
      contract: ctx.contract,
      provider: ctx.provider,
      targetOwner: await makeIdentity(),
      admin: ctx.admin.publicKey,
      rail: 'tnk',
      contextRevision: ctx.contextRevision,
      previousRevision,
      nonce: index.toString(16).padStart(64, '0'),
    });
    const result = await executePayoutBinding(
      ctx.contract,
      ctx.storage,
      binding,
      ctx.admin.publicKey
    );
    assert.equal(result.ok, true, result.message);
    previousRevision = binding.revision;
    revisions.push(binding.revision);
  }
  const pointer = (
    await ctx.storage.get(`payout/current/tnk/${ctx.provider.publicKey}`)
  ).value;
  assert.equal(pointer.latest_revision, revisions.at(-1));
  assert.equal(pointer.current_revision, revisions[0]);
  assert.equal(pointer.pending_revision, revisions.at(-1));
  assert.equal('history' in pointer, false);
  assert.ok(JSON.stringify(pointer).length < 700);
  const bindingKeys = Array.from(ctx.storage.values.keys())
    .filter((key) => key.startsWith(`payout/binding/tnk/${ctx.provider.publicKey}/`));
  assert.equal(bindingKeys.length, 128);
  for (const revision of revisions) {
    assert.equal(
      (await ctx.storage.get(
        `payout/binding/tnk/${ctx.provider.publicKey}/${revision}`
      )).value.revision,
      revision
    );
  }
});

test('MayhemContract exposes no mutable admin provider payout operation', async () => {
  const { contract } = await setupPayoutContract();
  assert.equal(typeof contract.setProviderPayout, 'undefined');
  assert.equal(contract.schemas?.has?.('setProviderPayout') ?? false, false);
});

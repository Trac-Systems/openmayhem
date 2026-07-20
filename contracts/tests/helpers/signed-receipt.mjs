import crypto from 'node:crypto';

import { receiptMessage } from '../../scripts/tap-settlement-roller.mjs';

const ED25519_SPKI_PREFIX = Buffer.from('302a300506032b6570032100', 'hex');

export function makeReceiptIdentity() {
  const { publicKey, privateKey } = crypto.generateKeyPairSync('ed25519');
  const publicDer = publicKey.export({ format: 'der', type: 'spki' });
  if (!Buffer.from(publicDer.subarray(0, ED25519_SPKI_PREFIX.length)).equals(ED25519_SPKI_PREFIX)) {
    throw new Error('unexpected Ed25519 public key DER prefix');
  }
  return {
    publicKeyHex: Buffer.from(publicDer.subarray(ED25519_SPKI_PREFIX.length)).toString('hex'),
    privateKey,
  };
}

export function signedTapReceipt({
  session,
  provider,
  user = makeReceiptIdentity(),
  enclave = makeReceiptIdentity(),
  au,
  seq = 1,
  epoch,
  extraBody = {},
  extra = {},
}) {
  const providerIdentity = provider ?? makeReceiptIdentity();
  const billingId = crypto.createHash('sha256').update(String(session)).digest('hex');
  const body = {
    schema_version: 9,
    session_id: session,
    billing_id: billingId,
    billing_attempt: 0,
    billing_prior_usage: {},
    billing_prior_au_owed_cum: '0',
    seq,
    final: true,
    rail: 'tap',
    user: user.publicKeyHex,
    provider: providerIdentity.publicKeyHex,
    enclave_id: 'e'.repeat(64),
    model_id: 'mayhem/test-tap-model',
    price_ver: 1,
    locked_rate_map: [{ unit: 'input_token', per_unit_au: String(au), granularity: 1 }],
    locked_per_req_au: '0',
    locked_min_session_au: '0',
    served_ctx: 32768,
    ctx_bracket: 'le32k',
    ctx_bracket_table_ver: 1,
    rules_ver: 1,
    usage: { input_token: 1 },
    au_owed_cum: String(au),
    prompt_hash: 'a'.repeat(64),
    ts: 3_600,
    ...extraBody,
  };
  const message = Buffer.from(receiptMessage(body));
  const entry = {
    receipt: {
      body,
      enclave_sig: crypto.sign(null, message, enclave.privateKey).toString('hex'),
      enclave_pubkey: enclave.publicKeyHex,
      user_sig: crypto.sign(null, message, user.privateKey).toString('hex'),
    },
    ...extra,
  };
  if (epoch !== undefined) entry.epoch = epoch;
  return entry;
}

export function targetedTapBindingsFor(bundle, providerAccounts, revisions = {}) {
  const bindings = {};
  const billingTotals = new Map();
  const entries = [...(bundle.receipts ?? [])].sort((left, right) => {
    const a = left.receipt?.body ?? left.body ?? left;
    const b = right.receipt?.body ?? right.body ?? right;
    return String(a.billing_id).localeCompare(String(b.billing_id)) ||
      Number(a.billing_attempt) - Number(b.billing_attempt) ||
      Number(a.seq) - Number(b.seq);
  });

  for (const entry of entries) {
    const body = entry.receipt?.body ?? entry.body ?? entry;
    const epoch = entry.receipt_epoch ?? entry.epoch ??
      body.receipt_epoch ?? body.epoch ?? bundle.epoch;
    const key = `${epoch}/${body.user.toLowerCase()}/${body.session_id}`;
    const currentAu = BigInt(body.au_owed_cum);
    const previousAu = billingTotals.get(body.billing_id) ??
      BigInt(body.billing_prior_au_owed_cum);
    const deltaAu = currentAu - previousAu;
    billingTotals.set(body.billing_id, currentAu);
    const existing = bindings[key];
    const account = providerAccounts[body.provider];
    if (!account) {
      throw new Error(`missing TAP payout account for provider ${body.provider}`);
    }
    bindings[key] = {
      epoch,
      session_id: body.session_id,
      user: body.user,
      provider: body.provider,
      payout_revision: revisions[body.provider] ?? '11'.repeat(32),
      account,
      chain_id: 61_000,
      context_revision: '22'.repeat(32),
      payment_config_version: 1,
      au: ((existing ? BigInt(existing.au) : 0n) + deltaAu).toString(),
    };
  }

  return bindings;
}

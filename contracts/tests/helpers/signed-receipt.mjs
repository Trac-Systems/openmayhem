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
  const body = {
    schema_version: 8,
    session_id: session,
    seq,
    final: true,
    rail: 'tap',
    user: user.publicKeyHex,
    provider: providerIdentity.publicKeyHex,
    enclave_id: enclave.publicKeyHex,
    model_id: 'mayhem/test-tap-model',
    price_ver: 1,
    locked_rate_map: [{ unit: 'input_token', per_unit_au: String(au), granularity: 1 }],
    locked_per_req_au: '0',
    locked_min_session_au: '0',
    served_ctx: 32768,
    ctx_bracket: 'le32k',
    ctx_bracket_table_ver: 1,
    rules_ver: 1,
    usage: { input_token: 1, output_token: 0 },
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
      user_sig: crypto.sign(null, message, user.privateKey).toString('hex'),
    },
    ...extra,
  };
  if (epoch !== undefined) entry.epoch = epoch;
  return entry;
}

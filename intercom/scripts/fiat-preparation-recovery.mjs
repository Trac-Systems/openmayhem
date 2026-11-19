// Offline preparation only. The result is an ordinary signed Mayhem feature.
import fs from 'node:fs';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import b4a from 'b4a';
import Wallet from 'trac-wallet';
import Contract, { CONTRACT_VERSION, payoutPreparationMessage, targetedPayoutControlMessage } from '../contract/contract.js';
import { applyFiatRecovery, fiatRecoveryFeatureKey, fiatResolutionKey, observeFiatOverlap } from '../contract/fiat-recovery.js';
import { runWalletHelper, readWalletHelperSecretsFromStdin } from '../src/wallet-helper.js';

const check = (v, message) => { if (!v) throw new Error(message); };
const jsonFile = path => JSON.parse(fs.readFileSync(path, 'utf8'));
const verify = (sig, message, pub) => Wallet.verify(b4a.from(sig, 'hex'), b4a.from(message), b4a.from(pub, 'hex'));

// Never treat an omitted export as proof of a missing ledger record.
export function snapshotContract(snapshot) {
  check(Number.isSafeInteger(snapshot.signed_length) && snapshot.signed_length > 0 &&
    Array.isArray(snapshot.rows), 'Expected a pinned confirmed state export.');
  const rows = new Map();
  for (const row of snapshot.rows) {
    check(row?.confirmed === true && row.signed_length === snapshot.signed_length &&
      typeof row.key === 'string' && Object.hasOwn(row, 'value') && !rows.has(row.key),
      'Duplicate, unconfirmed or inconsistent state row.');
    rows.set(row.key, row.value);
  }
  const contract = new Contract({ peer: { wallet: { verify } } }, {});
  contract.get = async key => {
    check(rows.has(key), 'Missing explicit confirmed state row in the private snapshot.');
    return rows.get(key);
  };
  // Used exclusively to preflight the full signed transition in memory.
  contract.put = async (key, value) => { rows.set(key, value); };
  return contract;
}

export async function inspectOverlap(contract, signedLength) {
  const state = await observeFiatOverlap(contract, CONTRACT_VERSION);
  if (state instanceof Error) throw state;
  return {
    signed_length: signedLength, state_hash: state.state_hash,
    old_operation: state.id, new_operation: state.nextId, admin: await contract.get('admin'),
    old_output: state.prep.payload.output, new_output: state.nextPreparation.payload.output,
    last_attempt: state.latest, verified_sibling_ref: state.siblingRef, retained_fee_ref: state.retainedFeeRef, fence_key: state.fence?.feature_key ?? null,
  };
}

export async function buildRecoveryFeature(contract, signedLength, review, evidenceDigest, sign) {
  const state = await observeFiatOverlap(contract, CONTRACT_VERSION);
  if (state instanceof Error) throw state;
  check(review.signed_length === signedLength && review.state_hash === state.state_hash &&
    review.old_operation === state.id && review.new_operation === state.nextId &&
    review.admin === await contract.get('admin'), 'Review does not match this confirmed snapshot.');
  const { stage, at, admin, attestation } = review;
  const allowed = ['stage', 'at', 'admin', 'attestation', 'signed_length', 'state_hash', 'old_operation', 'new_operation'];
  if (stage === 'reconcile_verified') allowed.push('stripe_transfer');
  check(Object.keys(review).length === allowed.length && allowed.every(k => Object.hasOwn(review, k)),
    'Review fields do not match the selected recovery stage.');
  check(stage === 'fence' ? attestation === null && evidenceDigest === null :
    evidenceDigest && attestation?.evidence_sha256 === evidenceDigest, 'Evidence file differs from review.');
  let instruction = null;
  if (stage === 'supersede_absent') {
    instruction = { ...state.nextPreparation, contract_version: CONTRACT_VERSION, admin };
    instruction.admin_sig = await sign(payoutPreparationMessage(instruction));
  } else if (stage === 'reconcile_verified') {
    check(state.attempt && review.stripe_transfer?.ref === attestation.old_effect,
      'Verified resolution needs the exact observed Stripe transfer.');
    instruction = {
      op: 'finalize_targeted_fiat_attempt', contract_version: CONTRACT_VERSION,
      rail: 'fiat', epoch: 508, economic_op_id: state.id, attempt_id: state.attempt.attempt_id,
      status: 'succeeded', at, evidence: review.stripe_transfer, admin
    };
    instruction.admin_sig = await sign(targetedPayoutControlMessage(instruction));
  }
  const value = {
    op: 'recover_fiat_preparation', contract_version: CONTRACT_VERSION, rail: 'fiat',
    stage, old_operation: state.id, new_operation: state.nextId, state_hash: state.state_hash,
    signed_length: signedLength, at, attestation, instruction, admin
  };
  value.admin_sig = await sign(targetedPayoutControlMessage(value));
  const key = await fiatRecoveryFeatureKey(contract, value);
  contract.address = admin;
  const result = await applyFiatRecovery(contract, key, value, CONTRACT_VERSION);
  if (result instanceof Error) throw result;
  return { feature: 'mayhem', key, value };
}

// A separate ordinary settlement records a positively identified 508 transfer.
// Its signed finalization must already be visible in a later confirmed view.
export async function buildVerifiedSettlement(contract, signedLength, recovery, sign) {
  const previous = recovery?.value;
  check(previous?.op === 'recover_fiat_preparation' && previous.stage === 'reconcile_verified' &&
    recovery.key === await fiatRecoveryFeatureKey(contract, previous) &&
    signedLength > previous.signed_length, 'Expected the confirmed signed verified resolution.');
  const record = await contract.get(fiatResolutionKey(previous.old_operation));
  check(record?.feature_key === recovery.key &&
    await fiatRecoveryFeatureKey(contract, record.value) === recovery.key,
    'Verified resolution differs from its confirmed record.');
  const result = previous.instruction;
  const id = previous.old_operation;
  const attempt = await contract.get(contract.targetedFiatAttemptRecordKey(id, result.attempt_id));
  const admin = await contract.get('admin');
  check(admin === previous.admin && attempt?.status === 'succeeded' &&
    await contract.finalizeTargetedFiatAttemptFeatureKey(attempt.result) ===
    await contract.finalizeTargetedFiatAttemptFeatureKey(result), 'Verified attempt readback differs.');
  const prepared = attempt.preparation;
  const value = {
    op: 'settle_targeted_fiat_output', contract_version: CONTRACT_VERSION, rail: 'fiat', epoch: 508,
    at: result.at, epoch_apply_hash: prepared.epoch_apply_hash, plan_root: prepared.plan_root,
    economic_op_id: id, output_index: prepared.output_index, preparation_id: id,
    attempt_id: result.attempt_id, stripe_transfer: result.evidence, admin,
  };
  value.admin_sig = await sign(targetedPayoutControlMessage(value));
  const key = await contract.targetedFiatOutputFeatureKey(value);
  if (key instanceof Error) throw key;
  contract.address = admin;
  const replay = await contract.applyTargetedFiatOutputFeature(key, value);
  if (replay instanceof Error) throw replay;
  return { feature: 'mayhem', key, value };
}

async function main([command, snapshotFile, reviewFile, outputFile, keypair, evidenceFile, ...extra]) {
  if (!snapshotFile || extra.length || !(command === 'inspect' && reviewFile && !outputFile ||
    (command === 'prepare' || command === 'verified-settlement' && !evidenceFile) && reviewFile && outputFile && keypair)) {
    throw new Error('Usage: inspect SNAPSHOT NEW_INSPECTION_FILE or prepare SNAPSHOT REVIEW NEW_FEATURE_FILE KEYPAIR [EVIDENCE_FILE] or verified-settlement SNAPSHOT SIGNED_RESOLUTION NEW_FEATURE_FILE KEYPAIR (wallet password JSON on stdin)');
  }
  const snapshot = jsonFile(snapshotFile);
  const contract = snapshotContract(snapshot);
  if (command === 'inspect') {
    const inspection = await inspectOverlap(contract, snapshot.signed_length);
    fs.writeFileSync(reviewFile, JSON.stringify(inspection, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
    console.log('FIAT recovery observation written locally.');
    return;
  }
  const review = jsonFile(reviewFile);
  const digest = evidenceFile ? createHash('sha256').update(fs.readFileSync(evidenceFile)).digest('hex') : null;
  const secrets = await readWalletHelperSecretsFromStdin();
  const sign = async message =>
    (await runWalletHelper({ 'wallet-helper': 'sign', keypair, message }, secrets)).signature;
  const feature = command === 'verified-settlement'
    ? await buildVerifiedSettlement(contract, snapshot.signed_length, review, sign)
    : await buildRecoveryFeature(contract, snapshot.signed_length, review, digest, sign);
  fs.writeFileSync(outputFile, JSON.stringify(feature) + '\n', { flag: 'wx', mode: 0o600 });
  console.log('Signed recovery feature prepared locally.');
}
if (process.argv[1] && fs.realpathSync(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2)).catch(e => {
    // Parser and filesystem errors can include private input bytes or paths.
    const message = e.message;
    const publicMessage = typeof message === 'string' &&
      (message.startsWith('Usage: inspect SNAPSHOT') || message.startsWith('Missing explicit confirmed state row'))
      ? message : 'FIAT recovery validation failed. Check the confirmed snapshot, review, evidence and output path.';
    console.error(publicMessage);
    process.exitCode = 1;
  });
}

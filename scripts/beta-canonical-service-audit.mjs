#!/usr/bin/env node
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';
import { blake3 } from '../intercom/node_modules/@tracsystems/blake3/dist/wasm/blake3.mjs';
import { keccak256 } from '../intercom/node_modules/ethereum-cryptography/keccak.js';
import { secp256k1 } from '../intercom/node_modules/ethereum-cryptography/secp256k1.js';
import PeerWallet from '../intercom/node_modules/trac-wallet/index.js';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const defaultOut = 'config/beta/canonical-service-audit.json';
const defaultCatalog = 'catalog/models.json';
const defaultCatalogSignature = 'catalog/signatures/models.json.sig';
const defaultCatalogKeyDir = 'catalog/keys';
const pubkey64 = /^[0-9a-fA-F]{64}$/;
const hex64 = /^[0-9a-fA-F]{64}$/;
const hex40 = /^[0-9a-fA-F]{40}$/;
const roomIdHex = /^[0-9a-fA-F]{32}$/;
const payoutRails = new Set(['fiat', 'tap', 'tnk']);
const ed25519SpkiPrefix = Buffer.from('302a300506032b6570032100', 'hex');

function usage() {
  console.log(`Usage: node scripts/beta-canonical-service-audit.mjs --snapshot PATH [--admin-pubkey HEX] [--catalog PATH] [--catalog-signature PATH] [--catalog-key-dir PATH] [--out PATH] [--json]

Audits a contract-state snapshot for P8.5 canonical service evidence. The output
is suitable for beta-metrics-collect --canonical-service PATH.

Accepted snapshot shapes:
- sorted [key, value] arrays from tests or exported state
- single RPC state responses: { key, value }
- RPC prefix responses: { prefix, values: [{ key, value }] }
- grouped service exports plus direct payments/current and payout/* records
- direct maps: { "enclave/<id>": {...}, "room/<id>": {...}, ... }`);
}

function parseArgs(argv) {
  const args = {
    catalog: defaultCatalog,
    catalogSignature: defaultCatalogSignature,
    catalogKeyDir: defaultCatalogKeyDir,
    out: defaultOut,
    json: false,
  };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--snapshot') {
      i += 1;
      if (!argv[i]) throw new Error('--snapshot requires a path');
      args.snapshot = argv[i];
    } else if (arg === '--admin-pubkey') {
      i += 1;
      if (!argv[i]) throw new Error('--admin-pubkey requires a value');
      args.adminPubkey = argv[i];
    } else if (arg === '--catalog') {
      i += 1;
      if (!argv[i]) throw new Error('--catalog requires a path');
      args.catalog = argv[i];
    } else if (arg === '--catalog-signature') {
      i += 1;
      if (!argv[i]) throw new Error('--catalog-signature requires a path');
      args.catalogSignature = argv[i];
    } else if (arg === '--catalog-key-dir') {
      i += 1;
      if (!argv[i]) throw new Error('--catalog-key-dir requires a path');
      args.catalogKeyDir = argv[i];
    } else if (arg === '--out') {
      i += 1;
      if (!argv[i]) throw new Error('--out requires a path');
      args.out = argv[i];
    } else if (arg === '--json') {
      args.json = true;
    } else if (arg === '-h' || arg === '--help') {
      usage();
      process.exit(0);
    } else {
      throw new Error(`unknown argument: ${arg}`);
    }
  }
  if (!args.snapshot) throw new Error('--snapshot is required');
  return args;
}

function resolvePath(filePath) {
  return path.isAbsolute(filePath) ? filePath : path.resolve(repoRoot, filePath);
}

function relativeFile(filePath) {
  const rel = path.relative(repoRoot, filePath);
  return rel.startsWith('..') ? filePath : rel;
}

function readJsonEvidence(filePath) {
  const resolved = resolvePath(filePath);
  const bytes = fs.readFileSync(resolved);
  return {
    path: resolved,
    bytes,
    value: JSON.parse(bytes.toString('utf8')),
    evidence: `file:${relativeFile(resolved)}#sha256:${crypto.createHash('sha256').update(bytes).digest('hex')}`,
  };
}

function writeJson(filePath, value) {
  const resolved = resolvePath(filePath);
  fs.mkdirSync(path.dirname(resolved), { recursive: true });
  fs.writeFileSync(resolved, `${JSON.stringify(value, null, 2)}\n`);
  return resolved;
}

function stableJson(value) {
  if (Array.isArray(value)) return `[${value.map(stableJson).join(',')}]`;
  if (value && typeof value === 'object') {
    return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${stableJson(value[key])}`).join(',')}}`;
  }
  return JSON.stringify(value);
}

function hasOwn(value, key) {
  return Object.prototype.hasOwnProperty.call(value, key);
}

function deriveGroupedKey(group, value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  if (group === 'enclaves') return value.enclave_id ? `enclave/${value.enclave_id}` : null;
  if (group === 'rooms') return value.room_id ? `room/${value.room_id}` : null;
  if (group === 'serves') return value.provider && value.enclave_id ? `serve/${value.provider}/${value.enclave_id}` : null;
  if (group === 'roomserve' || group === 'roomserves') {
    return value.room_id && value.provider && value.enclave_id
      ? `roomserve/${value.room_id}/${value.provider}/${value.enclave_id}`
      : null;
  }
  if (group === 'providers') return value.provider ? `prov/${value.provider}` : null;
  if (group === 'prices') return value.enclave_id ? `price/${value.enclave_id}` : null;
  return null;
}

function addRecord(records, key, value, context) {
  if (typeof key !== 'string' || key.length === 0) {
    throw new Error(`${context} record has no state key`);
  }
  const prior = records.get(key);
  if (prior !== undefined && stableJson(prior) !== stableJson(value)) {
    throw new Error(`${context} contains duplicate conflicting key ${key}`);
  }
  records.set(key, value);
}

function addEntry(records, entry, context, deriveKey = null) {
  if (Array.isArray(entry) && entry.length === 2) {
    addRecord(records, String(entry[0]), entry[1], context);
    return;
  }
  if (entry && typeof entry === 'object' && !Array.isArray(entry) && hasOwn(entry, 'key') && hasOwn(entry, 'value')) {
    addRecord(records, String(entry.key), entry.value, context);
    return;
  }
  const derived = deriveKey ? deriveKey(entry) : null;
  if (derived) {
    addRecord(records, derived, entry, context);
    return;
  }
  throw new Error(`${context} entry must be [key,value], {key,value}, or a derivable record`);
}

function addEntries(records, entries, context, deriveKey = null) {
  if (!Array.isArray(entries)) throw new Error(`${context} must be an array`);
  for (const [index, entry] of entries.entries()) {
    addEntry(records, entry, `${context}[${index}]`, deriveKey);
  }
}

function addSnapshot(records, value, context = 'snapshot') {
  if (Array.isArray(value)) {
    addEntries(records, value, context);
    return;
  }
  if (!value || typeof value !== 'object') {
    throw new Error(`${context} must be an object or array`);
  }

  if (hasOwn(value, 'key') && hasOwn(value, 'value')) {
    addEntry(records, value, context);
    return;
  }

  if (Array.isArray(value.values)) addEntries(records, value.values, `${context}.values`);
  if (value.admin !== undefined) addRecord(records, 'admin', value.admin, `${context}.admin`);

  for (const group of ['enclaves', 'rooms', 'serves', 'roomserve', 'roomserves', 'providers', 'prices']) {
    if (Array.isArray(value[group])) {
      addEntries(records, value[group], `${context}.${group}`, (record) => deriveGroupedKey(group, record));
    }
  }

  for (const nested of ['snapshots', 'prefixes', 'responses']) {
    if (Array.isArray(value[nested])) {
      for (const [index, child] of value[nested].entries()) addSnapshot(records, child, `${context}.${nested}[${index}]`);
    }
  }

  for (const [key, record] of Object.entries(value)) {
    if (key === 'admin') continue;
    if (key.includes('/')) addRecord(records, key, record, context);
  }
}

function buildRecordMap(snapshot) {
  const records = new Map();
  addSnapshot(records, snapshot);
  return records;
}

function entriesByPrefix(records, prefix) {
  return Array.from(records.entries())
    .filter(([key]) => key.startsWith(prefix))
    .map(([key, value]) => ({ key, value }))
    .sort((a, b) => a.key.localeCompare(b.key));
}

function indexByTail(records, prefix) {
  const indexed = new Map();
  for (const entry of entriesByPrefix(records, prefix)) {
    const tail = entry.key.slice(prefix.length);
    if (!tail.includes('/')) indexed.set(tail, entry.value);
  }
  return indexed;
}

function parseServeKey(key) {
  const parts = key.split('/');
  if (parts.length !== 3 || parts[0] !== 'serve') return null;
  return { provider: parts[1], enclave_id: parts[2] };
}

function parseRoomServeKey(key) {
  const parts = key.split('/');
  if (parts.length !== 4 || parts[0] !== 'roomserve') return null;
  return { room_id: parts[1], provider: parts[2], enclave_id: parts[3] };
}

function roomServeIndexEntries(room) {
  if (!Array.isArray(room?.serves)) return null;
  const invalid = [];
  const entries = new Map();
  for (const [index, entry] of room.serves.entries()) {
    if (
      !entry
      || typeof entry !== 'object'
      || Array.isArray(entry)
      || typeof entry.provider !== 'string'
      || entry.provider.length === 0
      || typeof entry.enclave_id !== 'string'
      || entry.enclave_id.length === 0
    ) {
      invalid.push(index);
      continue;
    }
    entries.set(JSON.stringify([entry.provider, entry.enclave_id]), {
      provider: entry.provider,
      enclave_id: entry.enclave_id,
    });
  }
  return {
    entries: Array.from(entries.values()).sort((a, b) => (
      a.provider.localeCompare(b.provider) || a.enclave_id.localeCompare(b.enclave_id)
    )),
    invalid,
  };
}

function roomServeIndexKey(roomId, entry) {
  return `roomserve/${roomId}/${entry.provider}/${entry.enclave_id}`;
}

function adminFromRecords(records, override) {
  if (override) return override;
  const raw = records.get('admin');
  if (typeof raw === 'string') return raw;
  if (raw && typeof raw === 'object' && !Array.isArray(raw)) {
    for (const key of ['pubkey', 'public_key', 'peer_pubkey', 'admin']) {
      if (typeof raw[key] === 'string') return raw[key];
    }
  }
  return null;
}

function currentPriceFor(prices, enclaveId) {
  const schedule = prices.get(enclaveId);
  if (!isRecord(schedule)) return null;
  if (hasOwn(schedule, 'current') || hasOwn(schedule, 'pending')) {
    return isRecord(schedule.current) ? schedule.current : null;
  }
  return schedule;
}

function isRecord(value) {
  return value && typeof value === 'object' && !Array.isArray(value);
}

function relativePosix(filePath) {
  return relativeFile(filePath).replaceAll(path.sep, '/');
}

function ed25519PublicKeyFromRawHex(publicKeyHex) {
  const raw = Buffer.from(publicKeyHex, 'hex');
  if (raw.length !== 32) throw new Error('public key must be a 32-byte hex Ed25519 key');
  return crypto.createPublicKey({
    key: Buffer.concat([ed25519SpkiPrefix, raw]),
    format: 'der',
    type: 'spki',
  });
}

function isLowerHex(value, chars) {
  return typeof value === 'string'
    && value.length === chars
    && /^[0-9a-f]+$/.test(value);
}

function parsePayoutPointerKey(key) {
  const parts = key.split('/');
  if (parts.length !== 4 || parts[0] !== 'payout' || parts[1] !== 'current') {
    return null;
  }
  return { rail: parts[2], provider: parts[3] };
}

function providerPayoutTargetBindingEvidence(intent) {
  return {
    admin: intent.admin,
    bootstrap: intent.bootstrap,
    chain_id: intent.chain_id,
    context_revision: intent.context_revision,
    currency: intent.currency,
    expires_after_epoch: intent.expires_after_epoch,
    network: intent.network,
    nonce: intent.nonce,
    payment_config_version: intent.payment_config_version,
    previous_revision: intent.previous_revision,
    provider: intent.provider,
    rail: intent.rail,
    target: intent.target,
    target_wallet: intent.target_wallet,
  };
}

function providerPayoutTargetBindingMessage(intent) {
  return `mayhem-provider-payout-target-binding-v1${stableJson(
    providerPayoutTargetBindingEvidence(intent),
  )}`;
}

function providerPayoutBindingMessage(intent) {
  return `mayhem-provider-payout-binding-v1${stableJson(intent)}`;
}

async function opaqueHash(domain, value) {
  const digest = await blake3(Buffer.from(stableJson({ domain, value })));
  return Buffer.from(digest).toString('hex');
}

function verifyEd25519(publicKeyHex, message, signatureHex) {
  if (!isLowerHex(publicKeyHex, 64) || !isLowerHex(signatureHex, 128)) {
    return false;
  }
  try {
    return crypto.verify(
      null,
      Buffer.from(message),
      ed25519PublicKeyFromRawHex(publicKeyHex),
      Buffer.from(signatureHex, 'hex'),
    );
  } catch {
    return false;
  }
}

function ethereumPersonalMessageHash(message) {
  const body = Buffer.from(message, 'utf8');
  const prefix = Buffer.from(`\x19Ethereum Signed Message:\n${body.length}`, 'utf8');
  return keccak256(Buffer.concat([prefix, body]));
}

function verifyTapTargetSignature(intent) {
  if (
    typeof intent.target !== 'string'
    || !/^0x[0-9a-f]{40}$/.test(intent.target)
    || typeof intent.target_signature !== 'string'
    || !/^0x[0-9a-f]{130}$/.test(intent.target_signature)
  ) {
    return false;
  }
  try {
    const bytes = Buffer.from(intent.target_signature.slice(2), 'hex');
    let recovery = bytes[64];
    if (recovery === 27 || recovery === 28) recovery -= 27;
    if (recovery !== 0 && recovery !== 1) return false;
    const signature = secp256k1.Signature
      .fromCompact(bytes.subarray(0, 64))
      .addRecoveryBit(recovery);
    if (signature.hasHighS()) return false;
    const publicKey = signature
      .recoverPublicKey(ethereumPersonalMessageHash(
        providerPayoutTargetBindingMessage(intent),
      ))
      .toRawBytes(false);
    const address = `0x${Buffer.from(
      keccak256(publicKey.subarray(1)).subarray(12),
    ).toString('hex')}`;
    return address === intent.target;
  } catch {
    return false;
  }
}

function payoutBindingIntent(binding) {
  return {
    op: 'bind_provider_payout',
    network: binding.network,
    admin: binding.admin,
    bootstrap: binding.bootstrap,
    context_revision: binding.context_revision,
    provider: binding.provider,
    rail: binding.rail,
    currency: binding.currency,
    chain_id: binding.chain_id,
    target: binding.target,
    target_wallet: binding.target_wallet,
    target_signature: binding.target_signature,
    previous_revision: binding.previous_revision,
    payment_config_version: binding.payment_config_version,
    nonce: binding.nonce,
    expires_after_epoch: binding.expires_after_epoch,
  };
}

async function readCatalogProof(args) {
  const catalog = readJsonEvidence(args.catalog);
  const signature = readJsonEvidence(args.catalogSignature);
  const errors = [];
  const sig = signature.value;
  if (!isRecord(sig)) {
    errors.push('catalog signature must be an object');
  } else {
    if (sig.schema_version !== 1) errors.push('catalog signature schema_version must be 1');
    if (sig.alg !== 'ed25519') errors.push('catalog signature alg must be ed25519');
    if (sig.signed_path !== relativePosix(catalog.path)) {
      errors.push(`catalog signature signed_path must be ${relativePosix(catalog.path)}`);
    }
    if (!pubkey64.test(sig.public_key || '')) errors.push('catalog signature public_key must be 64 hex chars');
    if (typeof sig.sig !== 'string' || !/^[0-9a-fA-F]{128}$/.test(sig.sig)) {
      errors.push('catalog signature sig must be 64 bytes of hex');
    }
    if (!pubkey64.test(sig.blake3 || '')) errors.push('catalog signature blake3 must be 64 hex chars');
  }

  let keyEvidence = null;
  if (isRecord(sig) && typeof sig.key_id === 'string' && sig.key_id.length > 0) {
    const keyPath = path.join(resolvePath(args.catalogKeyDir), `${sig.key_id}.json`);
    const key = readJsonEvidence(keyPath);
    keyEvidence = key.evidence;
    if (!isRecord(key.value)) {
      errors.push(`catalog key ${sig.key_id} must be an object`);
    } else {
      if (key.value.status !== 'active') errors.push(`catalog key ${sig.key_id} is not active`);
      if (key.value.alg !== 'ed25519') errors.push(`catalog key ${sig.key_id} alg must be ed25519`);
      if (key.value.public_key !== sig.public_key) {
        errors.push(`catalog key ${sig.key_id} public_key does not match signature`);
      }
    }
  } else {
    errors.push('catalog signature key_id is required');
  }

  const digest = Buffer.from(await blake3(catalog.bytes)).toString('hex');
  if (isRecord(sig) && sig.blake3 && sig.blake3.toLowerCase() !== digest) {
    errors.push('catalog signature blake3 does not match catalog bytes');
  }
  if (
    isRecord(sig)
    && pubkey64.test(sig.public_key || '')
    && typeof sig.sig === 'string'
    && /^[0-9a-fA-F]{128}$/.test(sig.sig)
  ) {
    const publicKey = ed25519PublicKeyFromRawHex(sig.public_key);
    const ok = crypto.verify(null, catalog.bytes, publicKey, Buffer.from(sig.sig, 'hex'));
    if (!ok) errors.push('catalog signature verification failed');
  }

  const modelIds = new Set();
  const models = new Map();
  let artifactCount = 0;
  if (!Array.isArray(catalog.value?.models)) {
    errors.push('catalog.models must be an array');
  } else {
    for (const [index, model] of catalog.value.models.entries()) {
      if (!isRecord(model) || typeof model.model_id !== 'string' || model.model_id.length === 0) {
        errors.push(`catalog.models[${index}].model_id is required`);
        continue;
      }
      if (modelIds.has(model.model_id)) errors.push(`catalog model_id ${model.model_id} is duplicated`);
      modelIds.add(model.model_id);
      models.set(model.model_id, model);
      if (!isRecord(model.artifacts)) {
        errors.push(`catalog model ${model.model_id} artifacts must be an object`);
        continue;
      }
      for (const [artifactName, artifact] of Object.entries(model.artifacts)) {
        artifactCount += 1;
        if (!isRecord(artifact)) {
          errors.push(`catalog model ${model.model_id} artifact ${artifactName} must be an object`);
          continue;
        }
        if (typeof artifact.engine !== 'string' || artifact.engine.length === 0) {
          errors.push(`catalog model ${model.model_id} artifact ${artifactName}.engine is required`);
        }
        if (typeof artifact.artifact_root !== 'string' || !hex64.test(artifact.artifact_root)) {
          errors.push(`catalog model ${model.model_id} artifact ${artifactName}.artifact_root must be 64 hex chars`);
        }
      }
    }
  }

  return {
    ok: errors.length === 0,
    errors,
    modelIds,
    models,
    artifactCount,
    catalog_path: relativePosix(catalog.path),
    signature_path: relativePosix(signature.path),
    blake3: digest,
    evidence: [
      catalog.evidence,
      signature.evidence,
      keyEvidence,
    ].filter(Boolean),
  };
}

function isSafeHuggingFaceComponent(value) {
  return typeof value === 'string'
    && /^[A-Za-z0-9][A-Za-z0-9._-]{0,95}$/.test(value)
    && !value.endsWith('.')
    && !value.endsWith('-')
    && !value.includes('..')
    && !value.includes('--');
}

function isSafeHuggingFaceRepo(value) {
  if (typeof value !== 'string') return false;
  const parts = value.split('/');
  return parts.length === 2 && parts.every(isSafeHuggingFaceComponent);
}

function isSafeHuggingFacePath(value) {
  return typeof value === 'string'
    && value.length > 0
    && !value.startsWith('/')
    && !value.startsWith('\\')
    && !value.includes('\\')
    && !value.includes('?')
    && !value.includes('#')
    && !value.includes('%')
    && !/[\x00-\x1f\x7f]/.test(value)
    && value.split('/').every((part) => (
      part.length > 0
      && part !== '.'
      && part !== '..'
      && /^[A-Za-z0-9._+-]+$/.test(part)
    ));
}

function artifactSourceMatches(enclave, artifact) {
  const source = enclave?.artifact_source;
  return source
    && artifact?.source
    && source.kind === artifact.source.kind
    && source.repo === artifact.source.repo
    && source.revision === artifact.source.revision
    && source.path === artifact.path;
}

function catalogArtifactForEnclave(catalogProof, enclave) {
  const model = catalogProof?.models?.get(enclave?.model_id);
  if (!isRecord(model?.artifacts)) return null;
  for (const [artifactName, artifact] of Object.entries(model.artifacts)) {
    if (
      isRecord(artifact)
      && artifact.engine === enclave.backend
      && typeof artifact.artifact_root === 'string'
      && artifact.artifact_root.toLowerCase() === String(enclave.artifact_root || '').toLowerCase()
      && artifact.artifact_root_kind === enclave.artifact_root_kind
      && artifactSourceMatches(enclave, artifact)
      && (artifact.source_sha256 ?? null) === (enclave.source_sha256 ?? null)
    ) {
      return { artifactName, artifact };
    }
  }
  return null;
}

async function deriveCatalogEnclaveId(admin, enclave) {
  const sidecarBinding = Object.entries(enclave.artifact_sidecars || {})
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([name, sidecar]) => `${name}${sidecar.artifact_root}`)
    .join('');
  const digest = await blake3(Buffer.from(
    `${admin}${enclave.model_id}${enclave.artifact_root}${sidecarBinding}${enclave.manifest_hash}`
  ));
  return Buffer.from(digest).toString('hex');
}

function verifyAdminStampedRecord(record, key, admin, fail) {
  if (!isRecord(record)) {
    fail(`${key} admin-stamped record is missing`);
    return false;
  }
  let ok = true;
  if (record.set_by !== admin) {
    fail(`${key} was not set by admin ${admin}`);
    ok = false;
  }
  if (record.set_by_role !== 'admin') {
    fail(`${key}.set_by_role must be admin`);
    ok = false;
  }
  return ok;
}

function verifyOptionalAdminMutation(record, key, admin, fail, byField, roleField) {
  let ok = true;
  if (hasOwn(record, byField) && record[byField] !== admin) {
    fail(`${key}.${byField} was not set by admin ${admin}`);
    ok = false;
  }
  if (hasOwn(record, roleField) && record[roleField] !== 'admin') {
    fail(`${key}.${roleField} must be admin`);
    ok = false;
  }
  return ok;
}

function verifyAdminProviderBanProvenance(provider, key, admin, fail) {
  if (provider?.status !== 'banned') return true;
  let ok = true;
  if (provider.banned_by_role !== 'admin') {
    fail(`${key}.banned_by_role must be admin`);
    ok = false;
  }
  if (provider.banned_by !== admin) {
    fail(`${key}.banned_by must match current admin ${admin}`);
    ok = false;
  }
  return ok;
}

function priceScheduleRecords(schedule) {
  if (!isRecord(schedule)) return [];
  if (hasOwn(schedule, 'current') || hasOwn(schedule, 'pending')) {
    return [
      ['current', schedule.current],
      ['pending', schedule.pending],
    ].filter(([, record]) => record !== undefined && record !== null);
  }
  return [['current', schedule]];
}

function verifyPriceRecord(record, key, enclaveId, enclave, admin, fail) {
  if (!isRecord(record)) {
    fail(`${key} price record is missing`);
    return false;
  }
  let ok = true;
  if (record.denom !== 'au_usd') {
    fail(`${key} is not denominated in au_usd`);
    ok = false;
  }
  if (record.enclave_id !== enclaveId) {
    fail(`${key} value.enclave_id mismatch`);
    ok = false;
  }
  if (enclave && record.model_id !== enclave.model_id) {
    fail(`${key} model_id does not match enclave ${enclaveId}`);
    ok = false;
  }
  if (record.set_by !== admin) {
    fail(`${key} was not set by admin ${admin}`);
    ok = false;
  }
  if (record.set_by_role !== 'admin') {
    fail(`${key}.set_by_role must be admin`);
    ok = false;
  }
  for (const field of ['in_per_1k_au', 'out_per_1k_au', 'per_req_au', 'min_session_au', 'effective_at']) {
    if (!Number.isInteger(record[field]) || record[field] < 0) {
      fail(`${key}.${field} must be a non-negative integer`);
      ok = false;
    }
  }
  if (!Number.isInteger(record.ver) || record.ver < 1) {
    fail(`${key}.ver must be a positive integer`);
    ok = false;
  }
  return ok;
}

function verifyPriceSchedule(schedule, enclaveId, enclave, admin, fail) {
  if (!isRecord(schedule)) {
    fail(`price/${enclaveId} schedule is missing or invalid`);
    return false;
  }
  let ok = true;
  const structured = hasOwn(schedule, 'current') || hasOwn(schedule, 'pending');
  if (structured) {
    if (schedule.denom !== 'au_usd') {
      fail(`price/${enclaveId} schedule is not denominated in au_usd`);
      ok = false;
    }
    if (schedule.enclave_id !== enclaveId) {
      fail(`price/${enclaveId} schedule value.enclave_id mismatch`);
      ok = false;
    }
    if (enclave && schedule.model_id !== enclave.model_id) {
      fail(`price/${enclaveId} schedule model_id does not match enclave ${enclaveId}`);
      ok = false;
    }
    if (!isRecord(schedule.current)) {
      fail(`price/${enclaveId}.current price record is missing`);
      ok = false;
    }
  }
  for (const [slot, record] of priceScheduleRecords(schedule)) {
    const key = structured ? `price/${enclaveId}.${slot}` : `price/${enclaveId}`;
    if (!verifyPriceRecord(record, key, enclaveId, enclave, admin, fail)) ok = false;
  }
  return ok;
}

async function verifyStripePayoutReadiness(records, binding, admin, fail, prefix) {
  const pointerKey =
    `payout/stripe-verified/target/${binding.provider}/${binding.target}`;
  const pointer = records.get(pointerKey);
  let ok = true;
  if (!isRecord(pointer)) {
    fail(`${prefix} has no account-scoped Stripe readiness pointer`);
    return false;
  }
  const expectedRecordKey = `payout/stripe-verified/${binding.provider}/${pointer.revision}`;
  if (
    pointer.provider !== binding.provider
    || !isLowerHex(pointer.revision, 64)
    || pointer.record_key !== expectedRecordKey
    || pointer.target !== binding.target
    || pointer.currency !== binding.currency
    || pointer.processor_revision !== binding.stripe_processor_revision
    || pointer.ready !== true
    || pointer.details_submitted !== true
    || pointer.payouts_enabled !== true
    || pointer.transfers_enabled !== true
  ) {
    fail(`${pointerKey} does not prove current ready Stripe ownership for ${prefix}`);
    ok = false;
  }
  const verification = records.get(pointer.record_key);
  if (
    !isRecord(verification)
    || verification.type !== 'stripe_payout_verification'
    || verification.revision !== pointer.revision
    || verification.provider !== binding.provider
    || verification.target !== binding.target
    || verification.currency !== binding.currency
    || verification.processor_revision !== binding.stripe_processor_revision
    || verification.revision !== binding.stripe_verification_revision
    || verification.context_revision !== binding.context_revision
    || verification.payment_config_version !== binding.payment_config_version
    || verification.ready !== true
    || verification.details_submitted !== true
    || verification.payouts_enabled !== true
    || verification.transfers_enabled !== true
    || verification.verified_by !== admin
    || verification.verified_by_role !== 'admin'
  ) {
    fail(`${prefix} has no matching admin-appended ready Stripe verification record`);
    ok = false;
  } else {
    const expectedProcessorRevision = await opaqueHash(
      'mayhem-stripe-payout-processor-evidence-v1',
      {
        account_id: verification.target,
        account_type: verification.account_type,
        country: verification.country,
        currency: verification.currency,
        mode: verification.mode,
        provider: verification.provider,
      },
    );
    const verificationValue = {
      op: 'verify_stripe_payout',
      provider: verification.provider,
      account_id: verification.target,
      account_type: verification.account_type,
      country: verification.country,
      currency: verification.currency,
      mode: verification.mode,
      verification_kind: verification.verification_kind,
      source_provider: verification.source_provider,
      processor_revision: verification.processor_revision,
      previous_verification: verification.previous_verification,
      details_submitted: verification.details_submitted,
      payouts_enabled: verification.payouts_enabled,
      transfers_enabled: verification.transfers_enabled,
      network: verification.network,
      admin: verification.admin,
      bootstrap: verification.bootstrap,
      context_revision: verification.context_revision,
      payment_config_version: verification.payment_config_version,
      request_nonce: verification.request_nonce,
    };
    const expectedVerificationRevision = await opaqueHash(
      'mayhem-stripe-payout-verification-feature-v1',
      verificationValue,
    );
    if (
      verification.processor_revision !== expectedProcessorRevision
      || verification.revision !== expectedVerificationRevision
      || pointer.record_key !==
        `payout/stripe-verified/${binding.provider}/${expectedVerificationRevision}`
    ) {
      fail(`${prefix} Stripe processor or readiness revision is not content-addressed`);
      ok = false;
    }
  }
  return ok;
}

async function verifyProviderPayoutBinding({
  records,
  key,
  binding,
  provider,
  rail,
  admin,
  verifyContext,
  fail,
}) {
  const prefix = key;
  if (!isRecord(binding)) {
    fail(`${prefix} payout binding is missing`);
    return false;
  }
  let ok = true;
  if (
    binding.type !== 'provider_payout_binding'
    || binding.provider !== provider
    || binding.rail !== rail
    || binding.verified !== true
  ) {
    fail(`${prefix} is not an immutable verified ${rail} binding for provider ${provider}`);
    ok = false;
  }
  if (binding.bound_at !== key) {
    fail(`${prefix}.bound_at must match its immutable state key`);
    ok = false;
  }
  if (binding.bound_by !== admin || binding.bound_by_role !== 'admin') {
    fail(`${prefix} was not appended by the sole admin writer ${admin}`);
    ok = false;
  }
  if (
    !isLowerHex(binding.admin, 64)
    || binding.admin !== admin
    || !isLowerHex(binding.bootstrap, 64)
    || !isLowerHex(binding.context_revision, 64)
    || !isLowerHex(binding.nonce, 64)
    || !Number.isSafeInteger(binding.payment_config_version)
    || binding.payment_config_version < 1
    || !Number.isSafeInteger(binding.expires_after_epoch)
    || binding.expires_after_epoch < 1
    || !Number.isSafeInteger(binding.activation_epoch)
    || binding.activation_epoch < 1
    || binding.activation_epoch > binding.expires_after_epoch
  ) {
    fail(`${prefix} has invalid canonical context, nonce, activation, or expiry evidence`);
    ok = false;
  }
  if (
    binding.previous_revision !== null
    && !isLowerHex(binding.previous_revision, 64)
  ) {
    fail(`${prefix}.previous_revision must be null or 64 lowercase hex chars`);
    ok = false;
  }
  if (typeof binding.target !== 'string' || binding.target.length === 0) {
    fail(`${prefix}.target is missing`);
    ok = false;
  }

  const intent = payoutBindingIntent(binding);
  const revisionDigest = await blake3(Buffer.from(providerPayoutBindingMessage(intent)));
  const expectedRevision = Buffer.from(revisionDigest).toString('hex');
  if (
    !isLowerHex(binding.revision, 64)
    || binding.revision !== expectedRevision
    || key !== `payout/binding/${rail}/${provider}/${expectedRevision}`
  ) {
    fail(`${prefix} revision or immutable key does not match the provider-signed intent`);
    ok = false;
  }
  if (!verifyEd25519(
    provider,
    providerPayoutBindingMessage(intent),
    binding.provider_signature,
  )) {
    fail(`${prefix}.provider_signature does not verify for provider ${provider}`);
    ok = false;
  }

  if (rail === 'tnk') {
    const addressPrefix = binding.network === 'mainnet'
      ? 'trac'
      : binding.network === 'testnet1'
        ? 'testtrac'
        : null;
    const expectedTarget = addressPrefix && isLowerHex(binding.target_wallet, 64)
      ? PeerWallet.encodeBech32mSafe(
          addressPrefix,
          Buffer.from(binding.target_wallet, 'hex'),
        )
      : null;
    if (
      binding.currency !== null
      || binding.chain_id !== null
      || !expectedTarget
      || binding.target !== expectedTarget
      || !verifyEd25519(
        binding.target_wallet,
        providerPayoutTargetBindingMessage(intent),
        binding.target_signature,
      )
    ) {
      fail(`${prefix} has invalid TNK target-wallet ownership evidence`);
      ok = false;
    }
    if (
      binding.stripe_processor_revision !== null
      || binding.stripe_verification_revision !== null
    ) {
      fail(`${prefix} TNK binding must not carry Stripe verification revisions`);
      ok = false;
    }
  } else if (rail === 'tap') {
    if (
      binding.currency !== null
      || !Number.isSafeInteger(binding.chain_id)
      || binding.chain_id < 1
      || binding.target_wallet !== null
      || !verifyTapTargetSignature(intent)
    ) {
      fail(`${prefix} has invalid TAP target-wallet ownership evidence`);
      ok = false;
    }
    if (
      binding.stripe_processor_revision !== null
      || binding.stripe_verification_revision !== null
    ) {
      fail(`${prefix} TAP binding must not carry Stripe verification revisions`);
      ok = false;
    }
  } else if (rail === 'fiat') {
    if (
      typeof binding.currency !== 'string'
      || !/^[a-z]{3}$/.test(binding.currency)
      || binding.chain_id !== null
      || binding.target_wallet !== null
      || binding.target_signature !== null
      || !/^acct_[A-Za-z0-9._-]+$/.test(binding.target)
      || !isLowerHex(binding.stripe_processor_revision, 64)
      || !isLowerHex(binding.stripe_verification_revision, 64)
    ) {
      fail(`${prefix} has invalid Stripe payout binding evidence`);
      ok = false;
    } else if (!(await verifyStripePayoutReadiness(
      records,
      binding,
      admin,
      fail,
      prefix,
    ))) {
      ok = false;
    }
  } else {
    fail(`${prefix} uses unsupported payout rail ${rail}`);
    ok = false;
  }

  if (!(await verifyContext(binding, prefix))) ok = false;
  return ok;
}

async function auditCanonicalService({ records, sourceEvidence, adminOverride, catalogProof }) {
  const errors = [];
  const warnings = [];
  const fail = (message) => errors.push(message);
  const warn = (message) => warnings.push(message);
  for (const error of catalogProof?.errors || []) fail(error);

  const admin = adminFromRecords(records, adminOverride);
  if (!isLowerHex(admin, 64)) {
    fail('admin pubkey is missing or invalid; include a lowercase state key admin or pass --admin-pubkey');
  }

  const providers = indexByTail(records, 'prov/');
  const enclaves = indexByTail(records, 'enclave/');
  const rooms = indexByTail(records, 'room/');
  const prices = indexByTail(records, 'price/');
  const activeProviders = new Map(Array.from(providers.entries()).filter(([, value]) => value?.status === 'active'));
  const bannedProviders = new Map(Array.from(providers.entries()).filter(([, value]) => value?.status === 'banned'));
  const activeEnclaves = new Map(Array.from(enclaves.entries()).filter(([, value]) => value?.status === 'active'));
  const openRooms = new Map(Array.from(rooms.entries()).filter(([, value]) => value?.status === 'open'));
  const activeServes = entriesByPrefix(records, 'serve/').filter((entry) => entry.value?.status === 'active');
  const activeRoomServes = entriesByPrefix(records, 'roomserve/').filter((entry) => entry.value?.status === 'active');

  if (activeEnclaves.size === 0) fail('no active admin-created enclave records found');
  if (openRooms.size === 0) fail('no open admin-created room records found');
  if (activeServes.length === 0) fail('no active provider serve records found');
  if (activeRoomServes.length === 0) fail('no active provider room join records found');

  const adminRulesRecordsVerified = verifyAdminStampedRecord(records.get('rules/current'), 'rules/current', admin, fail);
  const adminParamsRecordsVerified = verifyAdminStampedRecord(records.get('params/current'), 'params/current', admin, fail);
  const priceScheduleVerified = new Map();
  const verifyServedPriceSchedule = (enclaveId, enclave) => {
    if (priceScheduleVerified.has(enclaveId)) return priceScheduleVerified.get(enclaveId);
    const verified = verifyPriceSchedule(prices.get(enclaveId), enclaveId, enclave, admin, fail);
    priceScheduleVerified.set(enclaveId, verified);
    return verified;
  };

  let providerBanRecordsVerified = true;
  for (const [providerId, provider] of bannedProviders.entries()) {
    providerBanRecordsVerified =
      verifyAdminProviderBanProvenance(provider, `prov/${providerId}`, admin, fail)
      && providerBanRecordsVerified;
  }

  const contextVerification = new Map();
  const verifyPayoutContext = async (binding, bindingKey) => {
    const contextKey =
      `payout/context/${binding.payment_config_version}/${binding.context_revision}`;
    const cacheKey = stableJson({
      contextKey,
      network: binding.network,
      admin: binding.admin,
      bootstrap: binding.bootstrap,
      rail: binding.rail,
    });
    if (contextVerification.has(cacheKey)) {
      return contextVerification.get(cacheKey);
    }
    let verified = true;
    const context = records.get(contextKey);
    if (
      !isRecord(context)
      || context.type !== 'provider_payout_context'
      || context.revision !== binding.context_revision
      || context.network !== binding.network
      || context.admin !== admin
      || context.bootstrap !== binding.bootstrap
      || context.payment_config_version !== binding.payment_config_version
      || !isLowerHex(context.payment_config_hash, 64)
      || context.published_at !== contextKey
      || context.published_by !== admin
      || context.published_by_role !== 'admin'
    ) {
      fail(`${bindingKey} does not reference a valid immutable admin-published payout context`);
      verified = false;
    }

    const current = records.get('payout/context/current');
    if (
      !isRecord(current)
      || current.type !== 'provider_payout_context_pointer'
      || !isLowerHex(current.revision, 64)
      || current.record_key !==
        `payout/context/${current.payment_config_version}/${current.revision}`
      || current.updated_by !== admin
      || current.updated_by_role !== 'admin'
      || !records.has(current.record_key)
    ) {
      fail('payout/context/current is not a valid sole-admin-appended context pointer');
      verified = false;
    }

    const payments = records.get('payments/current');
    if (
      !isRecord(payments)
      || payments.ver !== binding.payment_config_version
      || payments.set_by !== admin
      || payments.set_by_role !== 'admin'
      || payments.tnk?.network !== binding.network
      || !Array.isArray(payments.rails)
      || !payments.rails.includes(binding.rail)
    ) {
      fail(`${bindingKey} does not match canonical admin payment policy`);
      verified = false;
    } else if (
      isRecord(context)
      && context.payment_config_hash !==
        await opaqueHash('mayhem-payout-payment-config-v1', payments)
    ) {
      fail(`${contextKey}.payment_config_hash does not bind payments/current`);
      verified = false;
    }
    contextVerification.set(cacheKey, verified);
    return verified;
  };

  const payoutErrorStart = errors.length;
  const verifiedPayoutBindingKeys = new Set();
  let providersWithVerifiedPayoutBindings = 0;
  for (const [providerId, provider] of activeProviders.entries()) {
    let providerVerified = true;
    if (!isLowerHex(providerId, 64)) {
      fail(`prov/${providerId} provider id must be 64 lowercase hex chars`);
      providerVerified = false;
    }
    if (hasOwn(provider, 'payouts')) {
      fail(`prov/${providerId}.payouts is a retired admin-set payout-target shape`);
      providerVerified = false;
    }
    if (
      !Array.isArray(provider.accepted_rails)
      || provider.accepted_rails.length === 0
      || new Set(provider.accepted_rails).size !== provider.accepted_rails.length
      || provider.accepted_rails.some((rail) => !payoutRails.has(rail))
    ) {
      fail(`prov/${providerId}.accepted_rails must contain unique fiat, tap, or tnk rails`);
      continue;
    }

    for (const rail of provider.accepted_rails) {
      const pointerKey = `payout/current/${rail}/${providerId}`;
      const pointer = records.get(pointerKey);
      if (
        !isRecord(pointer)
        || pointer.provider !== providerId
        || pointer.rail !== rail
        || !isLowerHex(pointer.current_revision, 64)
        || !isLowerHex(pointer.latest_revision, 64)
      ) {
        fail(`${pointerKey} is missing or invalid for an accepted provider rail`);
        providerVerified = false;
        continue;
      }
      const hasPending = pointer.pending_revision !== null;
      if (
        (hasPending && (
          !isLowerHex(pointer.pending_revision, 64)
          || !Number.isSafeInteger(pointer.pending_activation_epoch)
          || pointer.pending_activation_epoch < 1
          || pointer.latest_revision !== pointer.pending_revision
        ))
        || (!hasPending && (
          pointer.pending_activation_epoch !== null
          || pointer.latest_revision !== pointer.current_revision
        ))
      ) {
        fail(`${pointerKey} has inconsistent current/latest/pending revisions`);
        providerVerified = false;
      }

      const revisions = [
        pointer.current_revision,
        ...(hasPending ? [pointer.pending_revision] : []),
      ];
      for (const revision of revisions) {
        const bindingKey = `payout/binding/${rail}/${providerId}/${revision}`;
        const binding = records.get(bindingKey);
        const bindingVerified = await verifyProviderPayoutBinding({
          records,
          key: bindingKey,
          binding,
          provider: providerId,
          rail,
          admin,
          verifyContext: verifyPayoutContext,
          fail,
        });
        if (bindingVerified) {
          verifiedPayoutBindingKeys.add(bindingKey);
        } else {
          providerVerified = false;
        }
        if (
          hasPending
          && revision === pointer.pending_revision
          && isRecord(binding)
          && binding.activation_epoch !== pointer.pending_activation_epoch
        ) {
          fail(`${pointerKey}.pending_activation_epoch does not match ${bindingKey}`);
          providerVerified = false;
        }
      }
      const latestKey =
        `payout/binding/${rail}/${providerId}/${pointer.latest_revision}`;
      if (pointer.updated_at !== latestKey) {
        fail(`${pointerKey}.updated_at must reference its latest immutable binding`);
        providerVerified = false;
      }
    }
    if (providerVerified) providersWithVerifiedPayoutBindings += 1;
  }
  const providerPayoutBindingsVerified =
    errors.length === payoutErrorStart
    && activeProviders.size > 0
    && providersWithVerifiedPayoutBindings === activeProviders.size;

  for (const [enclaveId, enclave] of activeEnclaves.entries()) {
    if (enclave.enclave_id !== enclaveId) fail(`enclave/${enclaveId} value.enclave_id mismatch`);
    if (!hex64.test(enclaveId)) fail(`enclave/${enclaveId} state key id must be 64 hex chars`);
    if (enclave.created_by !== admin) fail(`enclave/${enclaveId} was not created by admin ${admin}`);
    if (enclave.created_by_role !== 'admin') fail(`enclave/${enclaveId}.created_by_role must be admin`);
    verifyOptionalAdminMutation(
      enclave,
      `enclave/${enclaveId}`,
      admin,
      fail,
      'updated_by',
      'updated_by_role'
    );
    if (typeof enclave.model_id !== 'string' || enclave.model_id.length === 0) {
      fail(`enclave/${enclaveId} is missing model_id`);
    } else if (!catalogProof?.modelIds?.has(enclave.model_id)) {
      fail(`enclave/${enclaveId}.model_id ${enclave.model_id} is not present in the signed admin catalog`);
    } else {
      if (typeof enclave.backend !== 'string' || enclave.backend.length === 0) {
        fail(`enclave/${enclaveId}.backend is required`);
      }
      if (typeof enclave.artifact_root !== 'string' || !hex64.test(enclave.artifact_root)) {
        fail(`enclave/${enclaveId}.artifact_root must be 64 hex chars`);
      }
      if (enclave.artifact_root_kind !== 'blake3_merkle_v1') {
        fail(`enclave/${enclaveId}.artifact_root_kind must be blake3_merkle_v1`);
      }
      if (!isRecord(enclave.artifact_source)) {
        fail(`enclave/${enclaveId}.artifact_source is required`);
      } else {
        if (enclave.artifact_source.kind !== 'huggingface') {
          fail(`enclave/${enclaveId}.artifact_source.kind must be huggingface`);
        }
        if (!isSafeHuggingFaceRepo(enclave.artifact_source.repo)) {
          fail(`enclave/${enclaveId}.artifact_source.repo must be a safe namespace/name Hugging Face repo id`);
        }
        if (!hex40.test(enclave.artifact_source.revision || '')) {
          fail(`enclave/${enclaveId}.artifact_source.revision must be a 40-hex git commit`);
        }
        if (!isSafeHuggingFacePath(enclave.artifact_source.path)) {
          fail(`enclave/${enclaveId}.artifact_source.path must be a safe relative Hugging Face path`);
        }
      }
      if (typeof enclave.source_sha256 !== 'string' || !hex64.test(enclave.source_sha256)) {
        fail(`enclave/${enclaveId}.source_sha256 must be 64 hex chars`);
      }
      if (typeof enclave.manifest_hash !== 'string' || !hex64.test(enclave.manifest_hash)) {
        fail(`enclave/${enclaveId}.manifest_hash must be 64 hex chars`);
      }
      if (typeof enclave.binary_hash !== 'string' || !hex64.test(enclave.binary_hash)) {
        fail(`enclave/${enclaveId}.binary_hash must be 64 hex chars`);
      }
      if (
        typeof enclave.backend === 'string'
        && typeof enclave.artifact_root === 'string'
        && hex64.test(enclave.artifact_root)
        && !catalogArtifactForEnclave(catalogProof, enclave)
      ) {
        fail(`enclave/${enclaveId}.backend/artifact_root/source is not present in the signed admin catalog for ${enclave.model_id}`);
      }
      if (
        pubkey64.test(admin || '')
        && typeof enclave.backend === 'string'
        && typeof enclave.artifact_root === 'string'
        && hex64.test(enclave.artifact_root)
        && typeof enclave.manifest_hash === 'string'
        && hex64.test(enclave.manifest_hash)
        && typeof enclave.binary_hash === 'string'
        && hex64.test(enclave.binary_hash)
      ) {
        const expectedEnclaveId = await deriveCatalogEnclaveId(admin, enclave);
        if (enclaveId !== expectedEnclaveId) {
          fail(`enclave/${enclaveId}.enclave_id must equal derived catalog enclave id ${expectedEnclaveId}`);
        }
      }
    }
  }

  for (const [roomId, room] of openRooms.entries()) {
    if (!roomIdHex.test(roomId)) fail(`room/${roomId} state key id must be 32 hex chars`);
    if (room.room_id !== roomId) fail(`room/${roomId} value.room_id mismatch`);
    if (room.creator !== admin) fail(`room/${roomId} was not created by admin ${admin}`);
    if (room.creator_role !== 'admin') fail(`room/${roomId}.creator_role must be admin`);
    if (!room.enclave_id) fail(`room/${roomId} is missing enclave_id`);
    const enclave = activeEnclaves.get(room.enclave_id);
    if (!enclave) {
      fail(`room/${roomId} references inactive or missing enclave ${room.enclave_id}`);
    } else if (room.model_id !== enclave.model_id) {
      fail(`room/${roomId} model_id does not match enclave ${room.enclave_id}`);
    }
    if (room.sidechannel !== `mx/room/${roomId}`) {
      fail(`room/${roomId} sidechannel does not match mx/room/${roomId}`);
    }
    const serveIndex = roomServeIndexEntries(room);
    if (!serveIndex) {
      fail(`room/${roomId} is missing active serve index room.serves`);
    } else {
      for (const invalidIndex of serveIndex.invalid) {
        fail(`room/${roomId}.serves[${invalidIndex}] is not a valid {provider,enclave_id} entry`);
      }
    }
  }

  const activeServeByKey = new Map();
  for (const entry of activeServes) {
    const parsed = parseServeKey(entry.key);
    if (!parsed) {
      fail(`${entry.key} is not a valid serve key`);
      continue;
    }
    const { provider, enclave_id: enclaveId } = parsed;
    activeServeByKey.set(entry.key, entry.value);
    if (entry.value.provider !== provider) fail(`${entry.key} value.provider mismatch`);
    if (entry.value.enclave_id !== enclaveId) fail(`${entry.key} value.enclave_id mismatch`);
    if (!activeProviders.has(provider)) fail(`${entry.key} references inactive or missing provider ${provider}`);
    const enclave = activeEnclaves.get(enclaveId);
    if (!enclave) {
      fail(`${entry.key} references inactive or missing enclave ${enclaveId}`);
      continue;
    }
    if (entry.value.model_id !== enclave.model_id) fail(`${entry.key} model_id does not match enclave ${enclaveId}`);

    const price = currentPriceFor(prices, enclaveId);
    if (!price) {
      fail(`${entry.key} has no current admin price for enclave ${enclaveId}`);
      if (prices.has(enclaveId)) verifyServedPriceSchedule(enclaveId, enclave);
    } else {
      verifyServedPriceSchedule(enclaveId, enclave);
    }
  }

  for (const entry of activeRoomServes) {
    const parsed = parseRoomServeKey(entry.key);
    if (!parsed) {
      fail(`${entry.key} is not a valid roomserve key`);
      continue;
    }
    const { room_id: roomId, provider, enclave_id: enclaveId } = parsed;
    if (entry.value.room_id !== roomId) fail(`${entry.key} value.room_id mismatch`);
    if (entry.value.provider !== provider) fail(`${entry.key} value.provider mismatch`);
    if (entry.value.enclave_id !== enclaveId) fail(`${entry.key} value.enclave_id mismatch`);
    if (!activeProviders.has(provider)) fail(`${entry.key} references inactive or missing provider ${provider}`);
    const room = openRooms.get(roomId);
    const enclave = activeEnclaves.get(enclaveId);
    const serve = activeServeByKey.get(`serve/${provider}/${enclaveId}`);
    if (!room) fail(`${entry.key} references inactive or missing room ${roomId}`);
    if (!enclave) fail(`${entry.key} references inactive or missing enclave ${enclaveId}`);
    if (!serve) fail(`${entry.key} has no matching active serve/${provider}/${enclaveId}`);
    if (room && room.enclave_id !== enclaveId) fail(`${entry.key} room.enclave_id mismatch`);
    if (room && enclave && room.model_id !== enclave.model_id) fail(`${entry.key} room model does not match enclave model`);
    if (entry.value.model_id && enclave && entry.value.model_id !== enclave.model_id) {
      fail(`${entry.key} value.model_id does not match enclave model`);
    }
    if (room && entry.value.sidechannel !== room.sidechannel) fail(`${entry.key} sidechannel does not match room sidechannel`);
    const roomServeIndex = roomServeIndexEntries(room);
    if (roomServeIndex && !roomServeIndex.entries.some((item) => (
      item.provider === provider && item.enclave_id === enclaveId
    ))) {
      fail(`${entry.key} missing from room/${roomId}.serves`);
    }
    if (serve && Array.isArray(serve.rooms) && !serve.rooms.includes(roomId)) {
      fail(`${entry.key} missing from serve/${provider}/${enclaveId}.rooms`);
    }
    const providerRecord = activeProviders.get(provider);
    if (providerRecord && Array.isArray(providerRecord.enclaves) && !providerRecord.enclaves.includes(enclaveId)) {
      fail(`${entry.key} missing from prov/${provider}.enclaves`);
    }
    if (enclave && Array.isArray(enclave.providers) && !enclave.providers.includes(provider)) {
      fail(`${entry.key} missing from enclave/${enclaveId}.providers`);
    }
  }

  for (const [roomId, room] of openRooms.entries()) {
    const roomServeIndex = roomServeIndexEntries(room);
    if (!roomServeIndex) continue;
    for (const indexed of roomServeIndex.entries) {
      const key = roomServeIndexKey(roomId, indexed);
      const record = records.get(key);
      if (!record || record.status !== 'active') {
        fail(`room/${roomId}.serves references missing or inactive ${key}`);
      }
    }
  }

  const ok = errors.length === 0;
  const counts = {
    records: records.size,
    active_providers: activeProviders.size,
    active_enclaves: activeEnclaves.size,
    open_rooms: openRooms.size,
    active_serves: activeServes.length,
    active_room_joins: activeRoomServes.length,
    provider_payout_bindings: verifiedPayoutBindingKeys.size,
    providers_with_verified_payout_bindings: providersWithVerifiedPayoutBindings,
    banned_providers: bannedProviders.size,
    catalog_models: catalogProof?.modelIds?.size ?? 0,
    catalog_artifacts: catalogProof?.artifactCount ?? 0,
  };
  const summaryDigest = crypto
    .createHash('sha256')
    .update(stableJson({ admin, catalog: catalogProof?.blake3, counts, ok }))
    .digest('hex');

  return {
    schema_version: 1,
    ok,
    generated_at: new Date().toISOString(),
    source: {
      evidence: sourceEvidence,
      admin,
      records: records.size,
      catalog: catalogProof ? {
        path: catalogProof.catalog_path,
        signature_path: catalogProof.signature_path,
        blake3: catalogProof.blake3,
        models: catalogProof.modelIds.size,
        artifacts: catalogProof.artifactCount,
      } : null,
    },
    canonical_service: {
      admin_created_enclaves_verified: ok,
      admin_catalog_records_verified: ok && catalogProof?.ok === true,
      admin_created_rooms_verified: ok,
      provider_join_records_verified: ok,
      admin_price_records_verified: ok,
      provider_payout_bindings_verified: ok && providerPayoutBindingsVerified,
      admin_rules_records_verified: ok && adminRulesRecordsVerified,
      admin_params_records_verified: ok && adminParamsRecordsVerified,
      admin_provider_ban_records_verified: ok && providerBanRecordsVerified,
      counts,
      evidence: [
        sourceEvidence,
        ...(catalogProof?.evidence || []),
        `audit:canonical-service:v1#sha256:${summaryDigest}`,
      ],
    },
    controls: {
      admin_controls_economy: ok,
      admin_sets_prices: ok,
      admin_sets_rules: ok && adminRulesRecordsVerified,
      admin_sets_params: ok && adminParamsRecordsVerified,
      admin_can_ban_providers: ok,
      providers_set_prices: false,
      providers_set_rules: false,
      providers_set_params: false,
      providers_set_payout_terms: false,
      providers_submit_models: false,
      providers_create_canonical_rooms: false,
      providers_only_join_admin_rooms: ok,
      provider_payout_bindings_permissionless: ok && providerPayoutBindingsVerified,
      provider_payout_bindings_ownership_verified: ok && providerPayoutBindingsVerified,
      admin_rules_params_verified: ok && adminRulesRecordsVerified && adminParamsRecordsVerified,
      evidence: [
        sourceEvidence,
        `audit:admin-control-plane:v1#sha256:${summaryDigest}`,
      ],
    },
    errors,
    warnings,
  };
}

function printHuman(report, outPath) {
  console.log(`Mayhem canonical service audit: ${report.ok ? 'ok' : 'not ready'}`);
  console.log(`Copy/paste audit path: ${outPath}`);
  console.log(`Active providers: ${report.canonical_service.counts.active_providers}`);
  console.log(`Active enclaves: ${report.canonical_service.counts.active_enclaves}`);
  console.log(`Open rooms: ${report.canonical_service.counts.open_rooms}`);
  console.log(`Active room joins: ${report.canonical_service.counts.active_room_joins}`);
  for (const warning of report.warnings) console.log(`warning: ${warning}`);
  for (const error of report.errors) console.error(`error: ${error}`);
  if (report.ok) {
    console.log('');
    console.log('Copy/paste beta metrics argument:');
    console.log(`--canonical-service ${outPath}`);
  }
}

async function main() {
  try {
    const args = parseArgs(process.argv.slice(2));
    const source = readJsonEvidence(args.snapshot);
    const records = buildRecordMap(source.value);
    const report = await auditCanonicalService({
      records,
      sourceEvidence: source.evidence,
      adminOverride: args.adminPubkey,
      catalogProof: await readCatalogProof(args),
    });
    const outPath = writeJson(args.out, report);
    if (args.json) console.log(JSON.stringify({ ...report, audit_path: outPath }, null, 2));
    else printHuman(report, outPath);
    if (!report.ok) process.exitCode = 1;
  } catch (error) {
    console.error(`error: ${error.message}`);
    process.exitCode = 1;
  }
}

await main();

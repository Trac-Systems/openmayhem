#!/usr/bin/env node
import fs from 'node:fs';
import crypto from 'node:crypto';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';
import { blake3 } from '../intercom/node_modules/@tracsystems/blake3/dist/wasm/blake3.mjs';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const defaultManifest = 'config/beta/testnet.template.json';
const hex64 = /^[0-9a-fA-F]{64}$/;
const hex128 = /^[0-9a-fA-F]{128}$/;
const pubkey64 = /^[0-9a-fA-F]{64}$/;
const testtracAddress = /^testtrac1[0-9a-z]+$/;
const safeCommandText = /^[a-zA-Z0-9._:@/+~,\-\s<>$:"{}[\]]+$/;
const sha256Evidence = /#sha256:[0-9a-fA-F]{64}(?:$|[#?&])/;
const httpUrl = /^https?:\/\//;
const httpsUrl = /^https:\/\//;
const ed25519SpkiPrefix = Buffer.from('302a300506032b6570032100', 'hex');

function usage() {
  console.log(`Usage: node scripts/beta-launch.mjs [--manifest PATH] [--allow-placeholders] [--json] [--no-commands]

Validates a Mayhem testnet beta launch manifest and prints copy/paste launch
commands. Strict mode fails on template placeholders; use --allow-placeholders
only to validate the committed template shape.`);
}

function parseArgs(argv) {
  const args = {
    manifest: defaultManifest,
    allowPlaceholders: false,
    json: false,
    commands: true,
  };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--manifest') {
      i += 1;
      if (!argv[i]) throw new Error('--manifest requires a path');
      args.manifest = argv[i];
    } else if (arg === '--allow-placeholders') {
      args.allowPlaceholders = true;
    } else if (arg === '--json') {
      args.json = true;
    } else if (arg === '--no-commands') {
      args.commands = false;
    } else if (arg === '-h' || arg === '--help') {
      usage();
      process.exit(0);
    } else {
      throw new Error(`unknown argument: ${arg}`);
    }
  }
  return args;
}

function readJson(filePath) {
  return JSON.parse(fs.readFileSync(filePath, 'utf8'));
}

function stableJson(value) {
  if (Array.isArray(value)) return `[${value.map(stableJson).join(',')}]`;
  if (value && typeof value === 'object') {
    return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${stableJson(value[key])}`).join(',')}}`;
  }
  return JSON.stringify(value);
}

function catalogArtifactForEnclave(model, backend, artifactRoot) {
  if (!model || typeof model !== 'object' || Array.isArray(model)) return null;
  if (!model.artifacts || typeof model.artifacts !== 'object' || Array.isArray(model.artifacts)) return null;
  for (const [artifactName, artifact] of Object.entries(model.artifacts)) {
    if (
      artifact
      && typeof artifact === 'object'
      && !Array.isArray(artifact)
      && artifact.engine === backend
      && typeof artifact.artifact_root === 'string'
      && artifact.artifact_root.toLowerCase() === String(artifactRoot || '').toLowerCase()
    ) {
      return { artifactName, artifact };
    }
  }
  return null;
}

async function deriveCatalogEnclaveId(adminPubkey, enclave) {
  const digest = await blake3(Buffer.from(
    `${adminPubkey}${enclave.model_id}${enclave.artifact_root}${enclave.manifest_hash}${enclave.binary_hash}`
  ));
  return Buffer.from(digest).toString('hex');
}

function isPlaceholder(value) {
  return typeof value === 'string' && (
    value.includes('<') ||
    value.includes('>') ||
    /\b(TBD|TODO|REPLACE|PLACEHOLDER|CHANGE_ME)\b/i.test(value)
  );
}

function issueFactory({ allowPlaceholders }) {
  const errors = [];
  const warnings = [];
  const add = (kind, message) => {
    if (kind === 'placeholder' && allowPlaceholders) warnings.push(message);
    else errors.push(message);
  };
  return { errors, warnings, add };
}

function requireObject(add, value, name) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    add('error', `${name} must be an object`);
    return false;
  }
  return true;
}

function requireOnlyKeys(add, value, name, allowedKeys) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return;
  const allowed = new Set(allowedKeys);
  const unknown = Object.keys(value).filter((key) => !allowed.has(key)).sort();
  if (unknown.length > 0) {
    add('error', `${name} contains unsupported field(s): ${unknown.join(', ')}`);
  }
}

function requireArray(add, value, name, min = 0) {
  if (!Array.isArray(value)) {
    add('error', `${name} must be an array`);
    return false;
  }
  if (value.length < min) add('error', `${name} must contain at least ${min} item(s)`);
  return true;
}

function requireStringArray(add, value, name, min = 0) {
  if (!requireArray(add, value, name, min)) return;
  const seen = new Set();
  for (const [index, item] of value.entries()) {
    const itemName = `${name}[${index}]`;
    requireString(add, item, itemName);
    if (typeof item !== 'string' || isPlaceholder(item)) continue;
    if (seen.has(item)) add('error', `${itemName} duplicates another ${name} entry`);
    seen.add(item);
  }
}

function requireLiteral(add, value, expected, name) {
  if (value !== expected) add('error', `${name} must be ${JSON.stringify(expected)}`);
}

function requireBoolean(add, value, name) {
  if (typeof value !== 'boolean') {
    add('error', `${name} must be a boolean`);
  }
}

function requireString(add, value, name, regex = null) {
  if (typeof value !== 'string' || value.length === 0) {
    add('error', `${name} must be a non-empty string`);
    return;
  }
  if (isPlaceholder(value)) add('placeholder', `${name} still contains a template placeholder`);
  if (regex && !isPlaceholder(value) && !regex.test(value)) {
    add('error', `${name} has invalid format`);
  }
}

function requirePositiveInteger(add, value, name) {
  if (!Number.isInteger(value) || value <= 0) {
    add('error', `${name} must be a positive integer`);
  }
}

function requirePositiveIntegerOrPlaceholder(add, value, name) {
  if (isPlaceholder(value)) {
    add('placeholder', `${name} still contains a template placeholder`);
    return;
  }
  requirePositiveInteger(add, value, name);
}

function requireNumberRange(add, value, name, { min = null, max = null, minExclusive = false } = {}) {
  if (typeof value !== 'number' || !Number.isFinite(value)) {
    add('error', `${name} must be a finite number`);
    return;
  }
  if (min !== null && (minExclusive ? value <= min : value < min)) {
    add('error', `${name} must be ${minExclusive ? '>' : '>='} ${min}`);
  }
  if (max !== null && value > max) {
    add('error', `${name} must be <= ${max}`);
  }
}

function requireDecimalString(add, value, name) {
  if (typeof value !== 'string' || !/^[0-9]+$/.test(value) || value === '0') {
    add('error', `${name} must be a positive decimal string`);
  }
}

function parseCheckedUrl(add, value, name) {
  if (typeof value !== 'string' || isPlaceholder(value)) return;
  try {
    return new URL(value);
  } catch {
    add('error', `${name} must be a valid URL`);
    return null;
  }
}

function isPrivateOrReservedHostname(hostname) {
  const host = String(hostname || '').toLowerCase().replace(/\.$/, '');
  return (
    host === 'localhost' ||
    host === '[::1]' ||
    host === '::1' ||
    host === '0.0.0.0' ||
    host === 'example.com' ||
    host.endsWith('.example') ||
    host.endsWith('.invalid') ||
    host.endsWith('.localhost') ||
    host.endsWith('.local') ||
    host.endsWith('.internal') ||
    host.endsWith('.test') ||
    host.startsWith('127.') ||
    host.startsWith('10.') ||
    host.startsWith('192.168.') ||
    host.startsWith('169.254.') ||
    /^172\.(1[6-9]|2[0-9]|3[0-1])\./.test(host)
  );
}

function validatePublicLaunchHostname(add, parsed, name) {
  if (!parsed) return;
  if (isPrivateOrReservedHostname(parsed.hostname)) {
    add('placeholder', `${name} must use a public launch hostname, not ${parsed.hostname}`);
  }
}

function validatePaygatePublicBase(add, paygate) {
  requireString(add, paygate.public_base_url, 'paygate.public_base_url', httpsUrl);
  const parsed = parseCheckedUrl(add, paygate.public_base_url, 'paygate.public_base_url');
  if (!parsed) return null;
  if (parsed.protocol !== 'https:') {
    add('error', 'paygate.public_base_url must use https');
  }
  if (parsed.username || parsed.password || parsed.search || parsed.hash) {
    add('error', 'paygate.public_base_url must not include credentials, query, or fragment');
  }
  validatePublicLaunchHostname(add, parsed, 'paygate.public_base_url');
  return parsed.origin;
}

function requireRailCheckoutUrl(add, value, name, rail, expectedPathSegment, expectedOrigin = null) {
  requireString(add, value, name, httpsUrl);
  const parsed = parseCheckedUrl(add, value, name);
  if (!parsed) return;
  if (parsed.protocol !== 'https:') {
    add('error', `${name} must use https`);
  }
  if (expectedOrigin && parsed.origin !== expectedOrigin) {
    add('error', `${name} must use the paygate public_base_url origin ${expectedOrigin}`);
  }
  validatePublicLaunchHostname(add, parsed, name);
  const segments = parsed.pathname.split('/').filter(Boolean);
  const last = segments.at(-1);
  const previous = segments.at(-2);
  if (previous !== rail || last !== expectedPathSegment) {
    add('error', `${name} must end with /${rail}/${expectedPathSegment}`);
  }
}

function validateHttpsDownloadUrl(add, value, name) {
  requireString(add, value, name, httpsUrl);
  const parsed = parseCheckedUrl(add, value, name);
  if (!parsed) return;
  if (parsed.protocol !== 'https:') add('error', `${name} must use https`);
  if (parsed.username || parsed.password || parsed.hash) {
    add('error', `${name} must not include credentials or fragment`);
  }
  validatePublicLaunchHostname(add, parsed, name);
}

function validateEnclaveDistribution(add, distribution, name) {
  if (!requireObject(add, distribution, name)) return;
  requireOnlyKeys(add, distribution, name, [
    'bundle_url',
    'manifest_url',
    'bundle_sha256',
    'bundle_bytes',
    'admin_signature',
    'mirrors',
  ]);
  validateHttpsDownloadUrl(add, distribution.bundle_url, `${name}.bundle_url`);
  validateHttpsDownloadUrl(add, distribution.manifest_url, `${name}.manifest_url`);
  requireString(add, distribution.bundle_sha256, `${name}.bundle_sha256`, hex64);
  requirePositiveIntegerOrPlaceholder(add, distribution.bundle_bytes, `${name}.bundle_bytes`);
  requireString(add, distribution.admin_signature, `${name}.admin_signature`, hex128);
  if (distribution.mirrors !== undefined) {
    if (requireArray(add, distribution.mirrors, `${name}.mirrors`)) {
      for (const [index, mirror] of distribution.mirrors.entries()) {
        validateHttpsDownloadUrl(add, mirror, `${name}.mirrors[${index}]`);
      }
    }
  }
}

function ed25519PublicKeyFromRawHex(publicKeyHex) {
  return crypto.createPublicKey({
    key: Buffer.concat([ed25519SpkiPrefix, Buffer.from(publicKeyHex, 'hex')]),
    format: 'der',
    type: 'spki',
  });
}

function enclaveDistributionSigningPayload(adminPubkey, enclave) {
  return Buffer.from(stableJson({
    schema_version: 1,
    kind: 'mayhem-enclave-distribution-v1',
    admin_pubkey: adminPubkey,
    enclave_id: enclave.enclave_id,
    model_id: enclave.model_id,
    backend: enclave.backend,
    artifact_root: enclave.artifact_root,
    manifest_hash: enclave.manifest_hash,
    binary_hash: enclave.binary_hash,
    distribution: {
      bundle_url: enclave.distribution?.bundle_url,
      manifest_url: enclave.distribution?.manifest_url,
      bundle_sha256: enclave.distribution?.bundle_sha256,
      bundle_bytes: enclave.distribution?.bundle_bytes,
      mirrors: enclave.distribution?.mirrors || [],
    },
  }));
}

function verifyEnclaveDistributionSignature(add, adminPubkey, enclave, name) {
  const fields = [
    adminPubkey,
    enclave.enclave_id,
    enclave.model_id,
    enclave.backend,
    enclave.artifact_root,
    enclave.manifest_hash,
    enclave.binary_hash,
    enclave.distribution?.bundle_url,
    enclave.distribution?.manifest_url,
    enclave.distribution?.bundle_sha256,
    enclave.distribution?.admin_signature,
  ];
  if (
    fields.some((value) => typeof value !== 'string' || isPlaceholder(value)) ||
    !Number.isInteger(enclave.distribution?.bundle_bytes) ||
    !pubkey64.test(adminPubkey) ||
    !hex64.test(enclave.enclave_id || '') ||
    !hex64.test(enclave.artifact_root || '') ||
    !hex64.test(enclave.manifest_hash || '') ||
    !hex64.test(enclave.binary_hash || '') ||
    !hex64.test(enclave.distribution?.bundle_sha256 || '') ||
    !hex128.test(enclave.distribution?.admin_signature || '')
  ) {
    return;
  }
  try {
    const ok = crypto.verify(
      null,
      enclaveDistributionSigningPayload(adminPubkey, enclave),
      ed25519PublicKeyFromRawHex(adminPubkey),
      Buffer.from(enclave.distribution.admin_signature, 'hex'),
    );
    if (!ok) {
      add('error', `${name}.admin_signature must verify against admin.peer_pubkey over the enclave distribution payload`);
    }
  } catch (error) {
    add('error', `${name}.admin_signature verification failed: ${error.message}`);
  }
}

function validateCheckoutUrls(add, paygate, expectedOrigin = null) {
  if (!requireObject(add, paygate.checkout_urls, 'paygate.checkout_urls')) return;
  const activeRails = ['stripe'];
  if (paygate.coinbase_enabled === true) activeRails.push('coinbase');
  requireOnlyKeys(add, paygate.checkout_urls, 'paygate.checkout_urls', activeRails);
  for (const rail of activeRails) {
    const railConfig = paygate.checkout_urls[rail];
    const prefix = `paygate.checkout_urls.${rail}`;
    if (!requireObject(add, railConfig, prefix)) continue;
    requireOnlyKeys(add, railConfig, prefix, ['success_url', 'cancel_url']);
    requireRailCheckoutUrl(add, railConfig.success_url, `${prefix}.success_url`, rail, 'return', expectedOrigin);
    requireRailCheckoutUrl(add, railConfig.cancel_url, `${prefix}.cancel_url`, rail, 'cancel', expectedOrigin);
  }
}

function validateRoomPolicy(add, value, name) {
  if (!requireObject(add, value, name)) return;
  requireOnlyKeys(add, value, name, ['region_hint', 'canary_set', 'min_reputation', 'max_price_mult']);
  if (value.region_hint !== undefined) requireString(add, value.region_hint, `${name}.region_hint`);
  if (value.canary_set !== undefined) requireString(add, value.canary_set, `${name}.canary_set`);
  if (value.min_reputation !== undefined) {
    requireNumberRange(add, value.min_reputation, `${name}.min_reputation`, { min: 0, max: 1 });
  }
  if (value.max_price_mult !== undefined) {
    requireNumberRange(add, value.max_price_mult, `${name}.max_price_mult`, { min: 0, minExclusive: true });
  }
}

function validateEvidenceArray(add, value, name) {
  if (!requireArray(add, value, name, 1)) return;
  const seen = new Set();
  for (const [index, item] of value.entries()) {
    const itemName = `${name}[${index}]`;
    requireString(add, item, itemName);
    if (typeof item !== 'string' || isPlaceholder(item)) continue;
    if (!sha256Evidence.test(item)) {
      add('error', `${itemName} must include #sha256:<64-hex> durable evidence`);
    }
    validateFileEvidenceHash(add, item, itemName);
    if (seen.has(item)) add('error', `${itemName} duplicates another evidence string`);
    seen.add(item);
  }
}

function fileEvidenceTarget(value) {
  if (typeof value !== 'string' || !value.startsWith('file:')) return null;
  const hashMatch = value.match(/#sha256:([0-9a-fA-F]{64})(?:$|[#?&])/);
  if (!hashMatch) return null;
  const rawPath = value.slice('file:'.length, hashMatch.index);
  if (!rawPath || path.isAbsolute(rawPath)) return null;
  const resolved = path.resolve(repoRoot, rawPath);
  const rel = path.relative(repoRoot, resolved);
  if (rel.startsWith('..') || path.isAbsolute(rel) || !fs.existsSync(resolved)) return null;
  return { rawPath, resolved };
}

function fileEvidenceJsonRecords(add, value, name) {
  if (!Array.isArray(value) || value.some(isPlaceholder)) return [];
  const records = [];
  for (const [index, item] of value.entries()) {
    const itemName = `${name}[${index}]`;
    const target = fileEvidenceTarget(item);
    if (!target) continue;
    try {
      records.push({
        path: target.rawPath,
        value: JSON.parse(fs.readFileSync(target.resolved, 'utf8')),
      });
    } catch (error) {
      add('error', `${itemName} must point to JSON evidence: ${error.message}`);
    }
  }
  if (records.length === 0) {
    add('error', `${name} must include at least one file-bound JSON evidence record`);
  }
  return records;
}

function validateFileEvidenceHash(add, value, name) {
  if (typeof value !== 'string' || !value.startsWith('file:')) return;
  const hashMatch = value.match(/#sha256:([0-9a-fA-F]{64})(?:$|[#?&])/);
  if (!hashMatch) return;
  const rawPath = value.slice('file:'.length, hashMatch.index);
  if (!rawPath) {
    add('error', `${name} file evidence path is empty`);
    return;
  }
  if (path.isAbsolute(rawPath)) {
    add('error', `${name} file evidence path must be repo-relative`);
    return;
  }
  const resolved = path.resolve(repoRoot, rawPath);
  const rel = path.relative(repoRoot, resolved);
  if (rel.startsWith('..') || path.isAbsolute(rel)) {
    add('error', `${name} file evidence path must stay inside the repo`);
    return;
  }
  if (!fs.existsSync(resolved)) {
    add('error', `${name} file evidence target does not exist: ${rawPath}`);
    return;
  }
  const actual = sha256File(resolved);
  const expected = hashMatch[1].toLowerCase();
  if (actual !== expected) {
    add('error', `${name} file evidence hash mismatch for ${rawPath}: expected ${expected}, got ${actual}`);
  }
}

function decimalTnkToE18(value) {
  if (typeof value !== 'string' && typeof value !== 'number') return null;
  const raw = String(value).trim();
  if (!/^[0-9]+(?:\.[0-9]+)?$/.test(raw)) return null;
  const [whole, frac = ''] = raw.split('.');
  if (frac.length > 18) return null;
  return BigInt(whole) * 10n ** 18n + BigInt(frac.padEnd(18, '0'));
}

function evidenceBalanceE18(record) {
  const value = record?.balance_tnk_e18 ?? record?.balance_e18 ?? record?.tnk_e18;
  if (typeof value === 'string' && /^[0-9]+$/.test(value)) return BigInt(value);
  return decimalTnkToE18(record?.balance_tnk ?? record?.balance);
}

function arrayContainsAll(actual, expected) {
  if (!Array.isArray(actual)) return false;
  const set = new Set(actual.map((item) => String(item)));
  return expected.every((item) => set.has(String(item)));
}

function evidenceMatchesNetwork(record, manifest) {
  const msb = record?.msb || record?.network?.msb || record || {};
  return (
    (record.network === undefined || record.network === manifest.network?.name) &&
    (record.network_name === undefined || record.network_name === manifest.network?.name) &&
    (msb.address_prefix === undefined || msb.address_prefix === manifest.network?.msb?.address_prefix) &&
    (msb.network_id === undefined || msb.network_id === manifest.network?.msb?.network_id) &&
    (msb.bootstrap === undefined || msb.bootstrap === manifest.network?.msb?.bootstrap) &&
    (msb.msb_bootstrap === undefined || msb.msb_bootstrap === manifest.network?.msb?.bootstrap) &&
    (msb.channel === undefined || msb.channel === manifest.network?.msb?.channel) &&
    (msb.msb_channel === undefined || msb.msb_channel === manifest.network?.msb?.channel)
  );
}

function validateBootstrapEvidence(add, manifest) {
  const records = fileEvidenceJsonRecords(add, manifest.evidence?.bootstrap_nodes, 'evidence.bootstrap_nodes');
  if (records.length === 0) return;
  const expectedPeerDht = manifest.network?.dht?.peer_bootstrap || [];
  const expectedMsbDht = manifest.network?.dht?.msb_bootstrap || [];
  const ok = records.some(({ value }) => {
    const peerDht = value.peer_dht_bootstrap || value.peer_bootstrap_nodes || value.peer_bootstrap;
    const msbDht = value.msb_dht_bootstrap || value.msb_bootstrap_nodes || value.msb_bootstrap;
    return (
      value.ok === true &&
      evidenceMatchesNetwork(value, manifest) &&
      arrayContainsAll(peerDht, expectedPeerDht) &&
      arrayContainsAll(msbDht, expectedMsbDht)
    );
  });
  if (!ok) {
    add('error', 'evidence.bootstrap_nodes must include JSON proof for the manifest network, MSB bootstrap/channel, and peer/MSB DHT bootstrap nodes');
  }
}

function validateEpochWalletEvidence(add, manifest) {
  const records = fileEvidenceJsonRecords(add, manifest.evidence?.epoch_wallet_funding, 'evidence.epoch_wallet_funding');
  if (records.length === 0) return;
  const min = typeof manifest.epoch_wallet?.min_balance_tnk_e18 === 'string' && /^[0-9]+$/.test(manifest.epoch_wallet.min_balance_tnk_e18)
    ? BigInt(manifest.epoch_wallet.min_balance_tnk_e18)
    : null;
  const ok = records.some(({ value }) => {
    const balance = evidenceBalanceE18(value);
    return (
      value.funded === true &&
      value.address === manifest.epoch_wallet?.address &&
      evidenceMatchesNetwork(value, manifest) &&
      balance !== null &&
      min !== null &&
      balance >= min
    );
  });
  if (!ok) {
    add('error', 'evidence.epoch_wallet_funding must include JSON proof matching epoch_wallet.address with balance >= epoch_wallet.min_balance_tnk_e18 on the manifest MSB network');
  }
}

async function expectedSeedOptIns(manifest, roomLabels) {
  const expected = [];
  for (const provider of manifest.seed_providers || []) {
    for (const join of provider.joins || []) {
      for (const roomRef of join.rooms || []) {
        const canonicalRoom = roomLabels.get(roomRef);
        const roomId = canonicalRoom
          ? await roomIdForLaunchRoom({
            enclaveId: join.enclave_id,
            adminPubkey: manifest.admin?.peer_pubkey,
            room: canonicalRoom.room,
          })
          : null;
        expected.push({
          provider_pubkey: provider.provider_pubkey,
          enclave_id: join.enclave_id,
          room_label: roomRef,
          room_id: roomId,
        });
      }
    }
  }
  return expected;
}

function evidenceOptInRows(record) {
  if (Array.isArray(record?.opt_ins)) return record.opt_ins;
  if (Array.isArray(record?.seed_provider_opt_ins)) return record.seed_provider_opt_ins;
  if (Array.isArray(record?.providers)) {
    return record.providers.map((provider) => ({
      provider_pubkey: typeof provider === 'string' ? provider : provider.provider_pubkey,
      enclave_id: record.enclave_id,
      rooms: record.rooms,
      room_ids: record.room_ids || (record.room_id ? [record.room_id] : undefined),
    }));
  }
  return [];
}

async function validateSeedProviderEvidence(add, manifest, roomLabels) {
  const records = fileEvidenceJsonRecords(add, manifest.evidence?.seed_provider_opt_ins, 'evidence.seed_provider_opt_ins');
  if (records.length === 0) return;
  const rows = records.flatMap(({ value }) => evidenceOptInRows(value));
  const expected = await expectedSeedOptIns(manifest, roomLabels);
  const ok = expected.every((required) => rows.some((row) => {
    const rooms = Array.isArray(row.rooms) ? row.rooms.map(String) : [];
    const roomIds = Array.isArray(row.room_ids) ? row.room_ids.map(String) : [];
    return (
      row.provider_pubkey === required.provider_pubkey &&
      row.enclave_id === required.enclave_id &&
      (
        rooms.includes(required.room_label) ||
        (required.room_id && roomIds.includes(required.room_id))
      )
    );
  }));
  const hasFreeFeatureProof = records.some(({ value }) => value.free_feature_lifecycle_records === true);
  if (!ok || !hasFreeFeatureProof) {
    add('error', 'evidence.seed_provider_opt_ins must include JSON proof for every manifest seed provider/enclave/room opt-in as free lifecycle feature records');
  }
}

function evidenceDownloadRows(record) {
  if (Array.isArray(record?.distributions)) return record.distributions;
  if (Array.isArray(record?.enclave_downloads)) return record.enclave_downloads;
  return record?.enclave_id ? [record] : [];
}

function validateEnclaveDownloadEvidence(add, manifest) {
  const records = fileEvidenceJsonRecords(add, manifest.evidence?.enclave_downloads, 'evidence.enclave_downloads');
  if (records.length === 0) return;
  const rows = records.flatMap(({ value }) => evidenceDownloadRows(value));
  const ok = (manifest.canonical_enclaves || []).every((enclave) => rows.some((row) => (
    row.enclave_id === enclave.enclave_id &&
    row.admin_signed === true &&
    row.bundle_url === enclave.distribution?.bundle_url &&
    row.manifest_url === enclave.distribution?.manifest_url &&
    row.bundle_sha256 === enclave.distribution?.bundle_sha256 &&
    row.bundle_bytes === enclave.distribution?.bundle_bytes &&
    row.admin_signature === enclave.distribution?.admin_signature
  )));
  if (!ok) {
    add('error', 'evidence.enclave_downloads must include JSON proof matching every canonical enclave distribution URL/hash/size/admin signature');
  }
}

async function validateSemanticLaunchEvidence(add, manifest, roomLabels) {
  if (JSON.stringify(manifest.evidence || {}).includes('<')) return;
  validateBootstrapEvidence(add, manifest);
  validateEpochWalletEvidence(add, manifest);
  await validateSeedProviderEvidence(add, manifest, roomLabels);
  validateEnclaveDownloadEvidence(add, manifest);
}

function sha256File(filePath) {
  return crypto.createHash('sha256').update(fs.readFileSync(filePath)).digest('hex');
}

function relativeFile(filePath) {
  const rel = path.relative(repoRoot, filePath);
  return rel.startsWith('..') ? filePath : rel;
}

function requiredFileEvidence(filePath) {
  return `file:${relativeFile(filePath)}#sha256:${sha256File(filePath)}`;
}

function validateCanaryEvidence(add, evidence, canaryPath) {
  validateEvidenceArray(add, evidence, 'evidence.canary_set');
  if (!Array.isArray(evidence) || evidence.some(isPlaceholder)) return;
  const expected = requiredFileEvidence(canaryPath);
  if (!evidence.includes(expected)) {
    add('error', `evidence.canary_set must include ${expected}`);
  }
}

function validateEpochWalletPaysFor(add, value) {
  if (!requireArray(add, value, 'epoch_wallet.pays_for', 1)) return;
  const allowed = new Set(['epoch_commit', 'payment_proof_rollup', 'payout_fee_sweep']);
  const seen = new Set();
  for (const [index, item] of value.entries()) {
    const name = `epoch_wallet.pays_for[${index}]`;
    requireString(add, item, name);
    if (typeof item !== 'string' || isPlaceholder(item)) continue;
    if (!allowed.has(item)) {
      add('error', `${name} must be one of ${[...allowed].join(', ')}`);
    }
    if (seen.has(item)) add('error', `${name} duplicates another epoch wallet purpose`);
    seen.add(item);
  }
}

function providerHomePlaceholder(providerPubkey) {
  if (typeof providerPubkey !== 'string' || isPlaceholder(providerPubkey)) {
    return '<provider-home-for-seed-provider>';
  }
  return `<provider-home-for-${providerPubkey.slice(0, 12)}>`;
}

async function validateLaunchManifest(manifest, { manifestPath, allowPlaceholders }) {
  const { errors, warnings, add } = issueFactory({ allowPlaceholders });
  const catalogPath = path.join(repoRoot, 'catalog/models.json');
  const catalog = readJson(catalogPath);
  const catalogModels = new Map((catalog.models || []).map((model) => [model.model_id, model]));

  requireOnlyKeys(add, manifest, 'manifest', [
    'schema_version',
    'launch_id',
    'network',
    'controls',
    'admin',
    'paygate',
    'epoch_wallet',
    'canary',
    'evidence',
    'canonical_enclaves',
    'seed_providers',
  ]);
  requireLiteral(add, manifest.schema_version, 1, 'schema_version');
  requireString(add, manifest.launch_id, 'launch_id');

  if (requireObject(add, manifest.network, 'network')) {
    requireOnlyKeys(add, manifest.network, 'network', ['name', 'denom', 'msb', 'subnet', 'dht']);
    requireLiteral(add, manifest.network.name, 'testnet1', 'network.name');
    requireLiteral(add, manifest.network.denom, 'mu_usd', 'network.denom');
    if (requireObject(add, manifest.network.msb, 'network.msb')) {
      requireOnlyKeys(add, manifest.network.msb, 'network.msb', ['address_prefix', 'network_id', 'bootstrap', 'channel']);
      requireLiteral(add, manifest.network.msb.address_prefix, 'testtrac', 'network.msb.address_prefix');
      requireLiteral(add, manifest.network.msb.network_id, 919, 'network.msb.network_id');
      requireString(add, manifest.network.msb.bootstrap, 'network.msb.bootstrap', hex64);
      requireString(add, manifest.network.msb.channel, 'network.msb.channel');
    }
    if (requireObject(add, manifest.network.subnet, 'network.subnet')) {
      requireOnlyKeys(add, manifest.network.subnet, 'network.subnet', ['channel', 'bootstrap']);
      requireString(add, manifest.network.subnet.channel, 'network.subnet.channel');
      if (manifest.network.subnet.bootstrap !== null && manifest.network.subnet.bootstrap !== undefined) {
        requireString(add, manifest.network.subnet.bootstrap, 'network.subnet.bootstrap', hex64);
        if (
          !isPlaceholder(manifest.network.subnet.bootstrap) &&
          manifest.network.subnet.bootstrap === manifest.network.msb?.bootstrap
        ) {
          add('error', 'network.subnet.bootstrap must not equal network.msb.bootstrap');
        }
      }
    }
    if (requireObject(add, manifest.network.dht, 'network.dht')) {
      requireOnlyKeys(add, manifest.network.dht, 'network.dht', ['peer_bootstrap', 'msb_bootstrap']);
      requireStringArray(add, manifest.network.dht.peer_bootstrap, 'network.dht.peer_bootstrap', 1);
      requireStringArray(add, manifest.network.dht.msb_bootstrap, 'network.dht.msb_bootstrap', 1);
    }
  }

  if (requireObject(add, manifest.controls, 'controls')) {
    requireOnlyKeys(add, manifest.controls, 'controls', [
      'admin_controls_economy',
      'admin_sets_prices',
      'admin_sets_rules',
      'admin_sets_params',
      'admin_sets_provider_payout_targets',
      'admin_can_ban_providers',
      'providers_set_prices',
      'providers_set_rules',
      'providers_set_params',
      'providers_set_payout_terms',
      'providers_submit_models',
      'providers_create_canonical_rooms',
      'providers_only_join_admin_rooms',
      'provider_payout_targets_admin_verified',
      'browser_handoffs_print_copy_paste_url',
    ]);
    requireLiteral(add, manifest.controls.admin_controls_economy, true, 'controls.admin_controls_economy');
    requireLiteral(add, manifest.controls.admin_sets_prices, true, 'controls.admin_sets_prices');
    requireLiteral(add, manifest.controls.admin_sets_rules, true, 'controls.admin_sets_rules');
    requireLiteral(add, manifest.controls.admin_sets_params, true, 'controls.admin_sets_params');
    requireLiteral(add, manifest.controls.admin_sets_provider_payout_targets, true, 'controls.admin_sets_provider_payout_targets');
    requireLiteral(add, manifest.controls.admin_can_ban_providers, true, 'controls.admin_can_ban_providers');
    requireLiteral(add, manifest.controls.providers_set_prices, false, 'controls.providers_set_prices');
    requireLiteral(add, manifest.controls.providers_set_rules, false, 'controls.providers_set_rules');
    requireLiteral(add, manifest.controls.providers_set_params, false, 'controls.providers_set_params');
    requireLiteral(add, manifest.controls.providers_set_payout_terms, false, 'controls.providers_set_payout_terms');
    requireLiteral(add, manifest.controls.providers_submit_models, false, 'controls.providers_submit_models');
    requireLiteral(add, manifest.controls.providers_create_canonical_rooms, false, 'controls.providers_create_canonical_rooms');
    requireLiteral(add, manifest.controls.providers_only_join_admin_rooms, true, 'controls.providers_only_join_admin_rooms');
    requireLiteral(add, manifest.controls.provider_payout_targets_admin_verified, true, 'controls.provider_payout_targets_admin_verified');
    requireLiteral(add, manifest.controls.browser_handoffs_print_copy_paste_url, true, 'controls.browser_handoffs_print_copy_paste_url');
  }

  if (requireObject(add, manifest.admin, 'admin')) {
    requireOnlyKeys(add, manifest.admin, 'admin', ['peer_pubkey', 'store_name', 'rpc_url', 'sc_bridge_url', 'sc_bridge_token_env']);
    requireString(add, manifest.admin.peer_pubkey, 'admin.peer_pubkey', pubkey64);
    requireString(add, manifest.admin.store_name, 'admin.store_name');
    requireString(add, manifest.admin.rpc_url, 'admin.rpc_url');
    requireString(add, manifest.admin.sc_bridge_url, 'admin.sc_bridge_url');
  }

  if (requireObject(add, manifest.paygate, 'paygate')) {
    requireOnlyKeys(add, manifest.paygate, 'paygate', [
      'public_base_url',
      'health_path',
      'tnk_treasury_address',
      'stripe_enabled',
      'coinbase_enabled',
      'checkout_urls',
    ]);
    const paygateOrigin = validatePaygatePublicBase(add, manifest.paygate);
    requireString(add, manifest.paygate.health_path, 'paygate.health_path');
    requireString(add, manifest.paygate.tnk_treasury_address, 'paygate.tnk_treasury_address', testtracAddress);
    requireLiteral(add, manifest.paygate.stripe_enabled, true, 'paygate.stripe_enabled');
    requireBoolean(add, manifest.paygate.coinbase_enabled, 'paygate.coinbase_enabled');
    validateCheckoutUrls(add, manifest.paygate, paygateOrigin);
  }

  if (requireObject(add, manifest.epoch_wallet, 'epoch_wallet')) {
    requireOnlyKeys(add, manifest.epoch_wallet, 'epoch_wallet', ['address', 'min_balance_tnk_e18', 'funded', 'pays_for']);
    requireString(add, manifest.epoch_wallet.address, 'epoch_wallet.address', testtracAddress);
    if (
      typeof manifest.epoch_wallet.address === 'string' &&
      !isPlaceholder(manifest.epoch_wallet.address) &&
      manifest.epoch_wallet.address.length !== 67
    ) {
      add('error', 'epoch_wallet.address must be a 67-character testtrac1 address');
    }
    requireDecimalString(add, manifest.epoch_wallet.min_balance_tnk_e18, 'epoch_wallet.min_balance_tnk_e18');
    if (manifest.epoch_wallet.funded !== true) {
      add(allowPlaceholders ? 'placeholder' : 'error', 'epoch_wallet.funded must be true for a real beta launch');
    }
    validateEpochWalletPaysFor(add, manifest.epoch_wallet.pays_for);
  }

  if (requireObject(add, manifest.canary, 'canary')) {
    requireOnlyKeys(add, manifest.canary, 'canary', ['set_id', 'path']);
    requireLiteral(add, manifest.canary.set_id, 'canary-launch-v1', 'canary.set_id');
    const canaryPath = path.join(repoRoot, manifest.canary.path || '');
    if (!fs.existsSync(canaryPath)) add('error', `canary.path does not exist: ${manifest.canary.path}`);
  }

  const canaryPath = path.join(repoRoot, manifest.canary?.path || '');
  if (requireObject(add, manifest.evidence, 'evidence')) {
    requireOnlyKeys(add, manifest.evidence, 'evidence', [
      'bootstrap_nodes',
      'epoch_wallet_funding',
      'seed_provider_opt_ins',
      'canary_set',
      'enclave_downloads',
    ]);
    validateEvidenceArray(add, manifest.evidence.bootstrap_nodes, 'evidence.bootstrap_nodes');
    validateEvidenceArray(add, manifest.evidence.epoch_wallet_funding, 'evidence.epoch_wallet_funding');
    validateEvidenceArray(add, manifest.evidence.seed_provider_opt_ins, 'evidence.seed_provider_opt_ins');
    validateEvidenceArray(add, manifest.evidence.enclave_downloads, 'evidence.enclave_downloads');
    if (fs.existsSync(canaryPath)) {
      validateCanaryEvidence(add, manifest.evidence.canary_set, canaryPath);
    } else {
      validateEvidenceArray(add, manifest.evidence.canary_set, 'evidence.canary_set');
    }
  }

  const enclaveIds = new Set();
  const roomLabels = new Map();
  if (requireArray(add, manifest.canonical_enclaves, 'canonical_enclaves', 1)) {
    for (const [index, enclave] of manifest.canonical_enclaves.entries()) {
      const prefix = `canonical_enclaves[${index}]`;
      if (!requireObject(add, enclave, prefix)) continue;
      requireOnlyKeys(add, enclave, prefix, [
        'enclave_id',
        'model_id',
        'backend',
        'artifact_root',
        'manifest_hash',
        'binary_hash',
        'att_tier',
        'caps',
        'distribution',
        'model_ref_mu',
        'price_mu',
        'rooms',
      ]);
      requireString(add, enclave.enclave_id, `${prefix}.enclave_id`, hex64);
      if (typeof enclave.enclave_id === 'string') enclaveIds.add(enclave.enclave_id);
      requireString(add, enclave.model_id, `${prefix}.model_id`);
      if (enclave.model_id && !isPlaceholder(enclave.model_id) && !catalogModels.has(enclave.model_id)) {
        add('error', `${prefix}.model_id is not present in catalog/models.json`);
      }
      requireString(add, enclave.backend, `${prefix}.backend`);
      requireString(add, enclave.artifact_root, `${prefix}.artifact_root`, hex64);
      if (
        typeof enclave.model_id === 'string'
        && typeof enclave.backend === 'string'
        && typeof enclave.artifact_root === 'string'
        && !isPlaceholder(enclave.model_id)
        && !isPlaceholder(enclave.backend)
        && !isPlaceholder(enclave.artifact_root)
        && hex64.test(enclave.artifact_root)
        && !catalogArtifactForEnclave(catalogModels.get(enclave.model_id), enclave.backend, enclave.artifact_root)
      ) {
        add('error', `${prefix}.backend/artifact_root is not present in catalog/models.json for ${enclave.model_id}`);
      }
      requireString(add, enclave.manifest_hash, `${prefix}.manifest_hash`, hex64);
      requireString(add, enclave.binary_hash, `${prefix}.binary_hash`, hex64);
      if (
        typeof manifest.admin?.peer_pubkey === 'string'
        && typeof enclave.enclave_id === 'string'
        && typeof enclave.model_id === 'string'
        && typeof enclave.artifact_root === 'string'
        && typeof enclave.manifest_hash === 'string'
        && typeof enclave.binary_hash === 'string'
        && ![
          manifest.admin.peer_pubkey,
          enclave.enclave_id,
          enclave.model_id,
          enclave.artifact_root,
          enclave.manifest_hash,
          enclave.binary_hash,
        ].some(isPlaceholder)
        && pubkey64.test(manifest.admin.peer_pubkey)
        && hex64.test(enclave.enclave_id)
        && hex64.test(enclave.artifact_root)
        && hex64.test(enclave.manifest_hash)
        && hex64.test(enclave.binary_hash)
      ) {
        const expectedEnclaveId = await deriveCatalogEnclaveId(manifest.admin.peer_pubkey, enclave);
        if (enclave.enclave_id !== expectedEnclaveId) {
          add('error', `${prefix}.enclave_id must equal derived catalog enclave id ${expectedEnclaveId}`);
        }
      }
      if (enclave.att_tier !== 1 && enclave.att_tier !== 2) {
        add('error', `${prefix}.att_tier must be 1 or 2`);
      }
      if (requireObject(add, enclave.caps, `${prefix}.caps`)) {
        requireOnlyKeys(add, enclave.caps, `${prefix}.caps`, ['chat', 'tools', 'json', 'ctx']);
        requireBoolean(add, enclave.caps.chat, `${prefix}.caps.chat`);
        requireBoolean(add, enclave.caps.tools, `${prefix}.caps.tools`);
        requireBoolean(add, enclave.caps.json, `${prefix}.caps.json`);
        requirePositiveInteger(add, enclave.caps.ctx, `${prefix}.caps.ctx`);
      }
      validateEnclaveDistribution(add, enclave.distribution, `${prefix}.distribution`);
      if (manifest.admin?.peer_pubkey) {
        verifyEnclaveDistributionSignature(add, manifest.admin.peer_pubkey, enclave, `${prefix}.distribution`);
      }
      if (requireObject(add, enclave.model_ref_mu, `${prefix}.model_ref_mu`)) {
        requireOnlyKeys(add, enclave.model_ref_mu, `${prefix}.model_ref_mu`, ['in_per_1k', 'out_per_1k']);
        requirePositiveInteger(add, enclave.model_ref_mu.in_per_1k, `${prefix}.model_ref_mu.in_per_1k`);
        requirePositiveInteger(add, enclave.model_ref_mu.out_per_1k, `${prefix}.model_ref_mu.out_per_1k`);
      }
      if (requireObject(add, enclave.price_mu, `${prefix}.price_mu`)) {
        requireOnlyKeys(add, enclave.price_mu, `${prefix}.price_mu`, [
          'denom',
          'in_per_1k',
          'out_per_1k',
          'per_req',
          'min_session',
          'effective_at',
        ]);
        if (enclave.price_mu.denom !== undefined) {
          requireLiteral(add, enclave.price_mu.denom, 'mu_usd', `${prefix}.price_mu.denom`);
        }
        requirePositiveInteger(add, enclave.price_mu.in_per_1k, `${prefix}.price_mu.in_per_1k`);
        requirePositiveInteger(add, enclave.price_mu.out_per_1k, `${prefix}.price_mu.out_per_1k`);
        if (!Number.isInteger(enclave.price_mu.per_req) || enclave.price_mu.per_req < 0) {
          add('error', `${prefix}.price_mu.per_req must be a non-negative integer`);
        }
        if (!Number.isInteger(enclave.price_mu.min_session) || enclave.price_mu.min_session < 0) {
          add('error', `${prefix}.price_mu.min_session must be a non-negative integer`);
        }
        if (!Number.isInteger(enclave.price_mu.effective_at) || enclave.price_mu.effective_at < 0) {
          add('error', `${prefix}.price_mu.effective_at must be a non-negative integer`);
        }
      }
      if (requireArray(add, enclave.rooms, `${prefix}.rooms`, 1)) {
        for (const [roomIndex, room] of enclave.rooms.entries()) {
          const roomPrefix = `${prefix}.rooms[${roomIndex}]`;
          if (!requireObject(add, room, roomPrefix)) continue;
          requireOnlyKeys(add, room, roomPrefix, ['label', 'nonce', 'admin_created', 'policy']);
          requireString(add, room.label, `${roomPrefix}.label`);
          requireString(add, room.nonce, `${roomPrefix}.nonce`);
          requireLiteral(add, room.admin_created, true, `${roomPrefix}.admin_created`);
          validateRoomPolicy(add, room.policy, `${roomPrefix}.policy`);
          if (typeof room.label === 'string') {
            if (roomLabels.has(room.label)) {
              add('error', `${roomPrefix}.label duplicates another canonical room label`);
            }
            roomLabels.set(room.label, { enclave, room });
          }
        }
      }
    }
  }

  if (manifest.provider_created_enclaves !== undefined || manifest.provider_rooms !== undefined) {
    add('error', 'provider-created canonical enclaves/rooms are forbidden in the launch manifest');
  }

  if (requireArray(add, manifest.seed_providers, 'seed_providers', 1)) {
    for (const [index, provider] of manifest.seed_providers.entries()) {
      const prefix = `seed_providers[${index}]`;
      if (!requireObject(add, provider, prefix)) continue;
      requireOnlyKeys(add, provider, prefix, ['provider_pubkey', 'payout', 'joins']);
      requireString(add, provider.provider_pubkey, `${prefix}.provider_pubkey`, pubkey64);
      if (provider.submitted_models !== undefined || provider.created_rooms !== undefined || provider.created_enclaves !== undefined) {
        add('error', `${prefix} must not contain provider-created models, rooms, or enclaves`);
      }
      if (requireObject(add, provider.payout, `${prefix}.payout`)) {
        requireOnlyKeys(add, provider.payout, `${prefix}.payout`, ['admin_approved', 'method', 'addr']);
        requireLiteral(add, provider.payout.admin_approved, true, `${prefix}.payout.admin_approved`);
        if (!['tnk', 'stripe', 'coinbase'].includes(provider.payout.method)) {
          add('error', `${prefix}.payout.method must be tnk, stripe, or coinbase`);
        }
        requireString(add, provider.payout.addr, `${prefix}.payout.addr`);
      }
      if (requireArray(add, provider.joins, `${prefix}.joins`, 1)) {
        for (const [joinIndex, join] of provider.joins.entries()) {
          const joinPrefix = `${prefix}.joins[${joinIndex}]`;
          if (!requireObject(add, join, joinPrefix)) continue;
          requireOnlyKeys(add, join, joinPrefix, ['enclave_id', 'rooms']);
          requireString(add, join.enclave_id, `${joinPrefix}.enclave_id`);
          if (!isPlaceholder(join.enclave_id) && !enclaveIds.has(join.enclave_id)) {
            add('error', `${joinPrefix}.enclave_id is not a canonical enclave`);
          }
          if (requireArray(add, join.rooms, `${joinPrefix}.rooms`, 1)) {
            for (const [roomIndex, roomRef] of join.rooms.entries()) {
              requireString(add, roomRef, `${joinPrefix}.rooms[${roomIndex}]`);
              const canonicalRoom = roomLabels.get(roomRef);
              if (!isPlaceholder(roomRef) && !canonicalRoom) {
                add('error', `${joinPrefix}.rooms[${roomIndex}] is not an admin-created canonical room label`);
              } else if (
                canonicalRoom &&
                !isPlaceholder(join.enclave_id) &&
                canonicalRoom.enclave.enclave_id !== join.enclave_id
              ) {
                add('error', `${joinPrefix}.rooms[${roomIndex}] belongs to enclave ${canonicalRoom.enclave.enclave_id}, not ${join.enclave_id}`);
              }
            }
          }
        }
      }
    }
  }

  await validateSemanticLaunchEvidence(add, manifest, roomLabels);

  const allText = JSON.stringify(manifest);
  if (!allowPlaceholders && isPlaceholder(allText)) {
    add('error', 'manifest still contains placeholders; use --allow-placeholders only for template checks');
  }

  return {
    ok: errors.length === 0,
    manifest_path: manifestPath,
    errors,
    warnings,
    counts: {
      canonical_enclaves: manifest.canonical_enclaves?.length ?? 0,
      canonical_rooms: roomLabels.size,
      seed_providers: manifest.seed_providers?.length ?? 0,
    },
  };
}

function sh(value) {
  const text = String(value);
  if (safeCommandText.test(text) && !text.includes("'")) return `'${text}'`;
  return `'${text.replaceAll("'", "'\\''")}'`;
}

function txCommand(value, sim = 0) {
  return `/tx --command ${sh(JSON.stringify(value))} --sim ${sim}`;
}

function providerJoinCommand({ home, rpcUrl, enclaveId, rooms }) {
  const roomArg = Array.isArray(rooms) && rooms.length > 0 ? commaList(rooms) : 'auto';
  return `mayhem provider join --home ${sh(home)} --rpc-url ${sh(rpcUrl)} --enclave ${sh(enclaveId)} --rooms ${sh(roomArg)}`;
}

function commaList(values) {
  return Array.isArray(values) && values.length > 0 ? values.join(',') : '';
}

function joinUrl(base, suffix) {
  if (!base) return '';
  const rawSuffix = suffix.startsWith('/') ? suffix : `/${suffix}`;
  if (base.startsWith('<') && base.endsWith('>')) {
    const inner = base.slice(1, -1).replace(/\/+$/, '');
    return `<${inner}${rawSuffix}>`;
  }
  return `${base.replace(/\/+$/, '')}${rawSuffix}`;
}

function railCheckoutUrl(paygate, rail, field) {
  return paygate.checkout_urls?.[rail]?.[field]
    || paygate[field]
    || `<${rail}-${field.replace('_', '-')}>`;
}

async function deriveRoomId(enclaveId, creator, nonce) {
  const digest = await blake3(Buffer.from(`${enclaveId}${creator}${nonce}`));
  return Buffer.from(digest).toString('hex').slice(0, 32);
}

async function roomIdForLaunchRoom({ enclaveId, adminPubkey, room }) {
  if (
    typeof enclaveId !== 'string' ||
    typeof adminPubkey !== 'string' ||
    typeof room?.nonce !== 'string' ||
    isPlaceholder(enclaveId) ||
    isPlaceholder(adminPubkey) ||
    isPlaceholder(room.nonce)
  ) {
    return room?.room_id || `<room_id returned by open_room for ${room?.label || 'room'}>`;
  }
  return deriveRoomId(enclaveId, adminPubkey, room.nonce);
}

async function buildCommands(manifest) {
  const appDir = path.join(repoRoot, 'intercom');
  const network = manifest.network || {};
  const msb = network.msb || {};
  const subnet = network.subnet || {};
  const dht = network.dht || {};
  const admin = manifest.admin || {};
  const paygate = manifest.paygate || {};
  const scTokenEnv = admin.sc_bridge_token_env || 'MAYHEM_BETA_SC_TOKEN';
  const peerDht = commaList(dht.peer_bootstrap);
  const msbDht = commaList(dht.msb_bootstrap);
  const adminStore = admin.store_name || 'mayhem-beta-admin';
  const paygateBase = String(paygate.public_base_url || '').replace(/\/+$/, '');
  const healthPath = String(paygate.health_path || '/v1/health');
  const roomByLabel = new Map();
  for (const enclave of manifest.canonical_enclaves || []) {
    for (const room of enclave.rooms || []) {
      roomByLabel.set(room.label, {
        enclave,
        room,
        room_id: await roomIdForLaunchRoom({
          enclaveId: enclave.enclave_id,
          adminPubkey: admin.peer_pubkey,
          room,
        }),
      });
    }
  }

  const boot = [
    `cd ${sh(appDir)}`,
    `"$MAYHEM_PEAR_RUNTIME" run . --network testnet1 --peer-store-name ${sh(adminStore)} --msb-store-name ${sh(`${adminStore}-msb`)} --msb-bootstrap ${sh(msb.bootstrap)} --msb-channel ${sh(msb.channel)} --subnet-channel ${sh(subnet.channel)} --sc-bridge 1 --sc-bridge-host 127.0.0.1 --sc-bridge-port 49222 --sc-bridge-token "$${scTokenEnv}" --rpc 1 --rpc-host 127.0.0.1 --rpc-port 49223${peerDht ? ` --peer-dht-bootstrap ${sh(peerDht)}` : ''}${msbDht ? ` --msb-dht-bootstrap ${sh(msbDht)}` : ''}`,
  ];

  const adminSetupTxs = [];
  const adminPayoutTxs = [];
  const providerLifecycleCommands = [];
  for (const enclave of manifest.canonical_enclaves || []) {
    adminSetupTxs.push(txCommand({
      op: 'set_model_ref',
      model_id: enclave.model_id,
      price_ref_mu: enclave.model_ref_mu,
    }));
    adminSetupTxs.push(txCommand({
      op: 'register_enclave',
      enclave_id: enclave.enclave_id,
      model_id: enclave.model_id,
      backend: enclave.backend,
      artifact_root: enclave.artifact_root,
      manifest_hash: enclave.manifest_hash,
      att_tier: enclave.att_tier,
      binary_hash: enclave.binary_hash,
      caps: enclave.caps,
    }));
    adminSetupTxs.push(txCommand({
      op: 'set_price',
      enclave_id: enclave.enclave_id,
      in_per_1k_mu: enclave.price_mu?.in_per_1k,
      out_per_1k_mu: enclave.price_mu?.out_per_1k,
      per_req_mu: enclave.price_mu?.per_req ?? 0,
      min_session_mu: enclave.price_mu?.min_session ?? 0,
      effective_at: enclave.price_mu?.effective_at ?? 0,
    }));
    for (const room of enclave.rooms || []) {
      const derivedRoom = roomByLabel.get(room.label);
      if (derivedRoom?.room_id) adminSetupTxs.push(`# room ${room.label} => ${derivedRoom.room_id}`);
      adminSetupTxs.push(txCommand({
        op: 'open_room',
        enclave_id: enclave.enclave_id,
        model_id: enclave.model_id,
        nonce: room.nonce,
        label: room.label,
        policy: room.policy || {},
      }));
    }
  }

  for (const provider of manifest.seed_providers || []) {
    adminPayoutTxs.push(txCommand({
      op: 'set_provider_payout',
      provider: provider.provider_pubkey,
      payout_addr: provider.payout?.addr,
      payout_method: provider.payout?.method,
    }));
    for (const join of provider.joins || []) {
      providerLifecycleCommands.push(`# provider ${provider.provider_pubkey}: run from this provider wallet; submits free signed Mayhem Feature records`);
      const roomIds = [];
      for (const roomRef of join.rooms || []) {
        const room = roomByLabel.get(roomRef);
        roomIds.push(room?.room_id || `<room_id returned by open_room for ${roomRef}>`);
      }
      providerLifecycleCommands.push(providerJoinCommand({
        home: providerHomePlaceholder(provider.provider_pubkey),
        rpcUrl: manifest.admin?.rpc_url || '<peer-rpc-url>',
        enclaveId: join.enclave_id,
        rooms: roomIds,
      }));
    }
  }

  const paygateHealthUrl = paygateBase ? joinUrl(paygateBase, healthPath) : '';
  const checkoutCommands = [
    `mayhem pay tnk --rpc-url ${sh(manifest.admin?.rpc_url || '<peer-rpc-url>')} --treasury-address ${sh(paygate.tnk_treasury_address || '<tnk-treasury-address>')} --amount 10`,
    `mayhem pay stripe --paygate-url ${sh(paygateBase || '<paygate-url>')} --amount 10 --success-url ${sh(railCheckoutUrl(paygate, 'stripe', 'success_url'))} --cancel-url ${sh(railCheckoutUrl(paygate, 'stripe', 'cancel_url'))}`,
  ];
  if (paygate.coinbase_enabled === true) {
    checkoutCommands.push(
      `mayhem pay coinbase --paygate-url ${sh(paygateBase || '<paygate-url>')} --amount 10 --success-url ${sh(railCheckoutUrl(paygate, 'coinbase', 'success_url'))} --cancel-url ${sh(railCheckoutUrl(paygate, 'coinbase', 'cancel_url'))}`,
    );
  }

  return {
    boot,
    adminTxs: adminSetupTxs,
    providerCommands: providerLifecycleCommands,
    adminSetupTxs,
    adminPayoutTxs,
    providerLifecycleCommands,
    allTxs: [
      ...adminSetupTxs,
      ...adminPayoutTxs,
    ],
    allCommands: [
      ...adminSetupTxs,
      ...providerLifecycleCommands,
      ...adminPayoutTxs,
    ],
    orderedCommands: [
      { label: 'admin canonical setup commands', commands: adminSetupTxs },
      { label: 'provider lifecycle feature commands', commands: providerLifecycleCommands },
      { label: 'admin provider payout commands', commands: adminPayoutTxs },
    ],
    paygateHealthUrl,
    checkoutCommands,
    emergencyBan: txCommand({
      op: 'ban_provider',
      provider: '<provider-pubkey>',
      reason_hash: '<reason-hash-64-hex>',
    }),
  };
}

function printHuman(report, commands, { args }) {
  console.log(`Mayhem beta launch manifest: ${report.ok ? 'ok' : 'not ready'}`);
  console.log(`Copy/paste manifest path: ${path.resolve(repoRoot, args.manifest)}`);
  console.log(`Canonical enclaves: ${report.counts.canonical_enclaves}`);
  console.log(`Canonical rooms: ${report.counts.canonical_rooms}`);
  console.log(`Seed providers: ${report.counts.seed_providers}`);

  for (const warning of report.warnings) console.log(`warning: ${warning}`);
  for (const error of report.errors) console.error(`error: ${error}`);
  if (!args.commands || !commands) return;

  console.log('');
  console.log('Copy/paste admin bootstrap command:');
  for (const line of commands.boot) console.log(line);

  for (const step of commands.orderedCommands || []) {
    console.log('');
    console.log(`Copy/paste ${step.label}:`);
    for (const command of step.commands) console.log(command);
  }

  if (commands.paygateHealthUrl) {
    console.log('');
    console.log(`Copy/paste paygate health URL: ${commands.paygateHealthUrl}`);
  }

  console.log('');
  console.log('Copy/paste payment commands; hosted rails print the checkout URL before any browser open:');
  for (const command of commands.checkoutCommands) console.log(command);

  console.log('');
  console.log('Copy/paste emergency provider ban command:');
  console.log(commands.emergencyBan);
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const manifestPath = path.resolve(repoRoot, args.manifest);
  const manifest = readJson(manifestPath);
  const report = await validateLaunchManifest(manifest, {
    manifestPath,
    allowPlaceholders: args.allowPlaceholders,
  });
  const commands = args.commands ? await buildCommands(manifest) : null;

  if (args.json) {
    console.log(JSON.stringify({ ...report, commands }, null, 2));
  } else {
    printHuman(report, commands, { args });
  }

  if (!report.ok) process.exit(1);
}

main().catch((error) => {
  console.error(error?.message || String(error));
  process.exit(1);
});

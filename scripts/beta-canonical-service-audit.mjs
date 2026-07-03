#!/usr/bin/env node
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const defaultOut = 'config/beta/canonical-service-audit.json';
const pubkey64 = /^[0-9a-fA-F]{64}$/;

function usage() {
  console.log(`Usage: node scripts/beta-canonical-service-audit.mjs --snapshot PATH [--admin-pubkey HEX] [--out PATH] [--json]

Audits a contract-state snapshot for P8.5 canonical service evidence. The output
is suitable for beta-metrics-collect --canonical-service PATH.

Accepted snapshot shapes:
- sorted [key, value] arrays from tests or exported state
- single RPC state responses: { key, value }
- RPC prefix responses: { prefix, values: [{ key, value }] }
- grouped exports: { admin, enclaves, rooms, serves, roomserve, providers, prices }
- direct maps: { "enclave/<id>": {...}, "room/<id>": {...}, ... }`);
}

function parseArgs(argv) {
  const args = {
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
  return schedule?.current ?? schedule;
}

function auditCanonicalService({ records, sourceEvidence, adminOverride }) {
  const errors = [];
  const warnings = [];
  const fail = (message) => errors.push(message);
  const warn = (message) => warnings.push(message);

  const admin = adminFromRecords(records, adminOverride);
  if (!admin || !pubkey64.test(admin)) fail('admin pubkey is missing or invalid; include state key admin or pass --admin-pubkey');

  const providers = indexByTail(records, 'prov/');
  const enclaves = indexByTail(records, 'enclave/');
  const rooms = indexByTail(records, 'room/');
  const prices = indexByTail(records, 'price/');
  const activeProviders = new Map(Array.from(providers.entries()).filter(([, value]) => value?.status === 'active'));
  const activeEnclaves = new Map(Array.from(enclaves.entries()).filter(([, value]) => value?.status === 'active'));
  const openRooms = new Map(Array.from(rooms.entries()).filter(([, value]) => value?.status === 'open'));
  const activeServes = entriesByPrefix(records, 'serve/').filter((entry) => entry.value?.status === 'active');
  const activeRoomServes = entriesByPrefix(records, 'roomserve/').filter((entry) => entry.value?.status === 'active');

  if (activeEnclaves.size === 0) fail('no active admin-created enclave records found');
  if (openRooms.size === 0) fail('no open admin-created room records found');
  if (activeServes.length === 0) fail('no active provider serve records found');
  if (activeRoomServes.length === 0) fail('no active provider room join records found');

  for (const [enclaveId, enclave] of activeEnclaves.entries()) {
    if (enclave.enclave_id !== enclaveId) fail(`enclave/${enclaveId} value.enclave_id mismatch`);
    if (enclave.created_by !== admin) fail(`enclave/${enclaveId} was not created by admin ${admin}`);
    if (typeof enclave.model_id !== 'string' || enclave.model_id.length === 0) {
      fail(`enclave/${enclaveId} is missing model_id`);
    }
  }

  for (const [roomId, room] of openRooms.entries()) {
    if (room.room_id !== roomId) fail(`room/${roomId} value.room_id mismatch`);
    if (room.creator !== admin) fail(`room/${roomId} was not created by admin ${admin}`);
    if (!room.enclave_id) fail(`room/${roomId} is missing enclave_id`);
    const enclave = activeEnclaves.get(room.enclave_id);
    if (!enclave) {
      fail(`room/${roomId} references inactive or missing enclave ${room.enclave_id}`);
    } else if (room.model_id !== enclave.model_id) {
      fail(`room/${roomId} model_id does not match enclave ${room.enclave_id}`);
    }
    if (room.sidechannel && room.sidechannel !== `mx/room/${roomId}`) {
      fail(`room/${roomId} sidechannel does not match mx/room/${roomId}`);
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
    } else {
      if (price.denom !== 'mu_usd') fail(`price/${enclaveId} is not denominated in mu_usd`);
      if (price.enclave_id !== enclaveId) fail(`price/${enclaveId} value.enclave_id mismatch`);
      if (price.model_id !== enclave.model_id) fail(`price/${enclaveId} model_id does not match enclave ${enclaveId}`);
      if (price.set_by !== admin || price.set_by_role !== 'admin') {
        fail(`price/${enclaveId} was not set by admin ${admin}`);
      }
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

  for (const entry of entriesByPrefix(records, 'roomserve/').filter((item) => item.value?.status === 'active')) {
    if (!entry.value.sidechannel) warn(`${entry.key} has no sidechannel field`);
  }

  const ok = errors.length === 0;
  const counts = {
    records: records.size,
    active_providers: activeProviders.size,
    active_enclaves: activeEnclaves.size,
    open_rooms: openRooms.size,
    active_serves: activeServes.length,
    active_room_joins: activeRoomServes.length,
  };
  const summaryDigest = crypto
    .createHash('sha256')
    .update(stableJson({ admin, counts, ok }))
    .digest('hex');

  return {
    schema_version: 1,
    ok,
    generated_at: new Date().toISOString(),
    source: {
      evidence: sourceEvidence,
      admin,
      records: records.size,
    },
    canonical_service: {
      admin_created_enclaves_verified: ok,
      admin_created_rooms_verified: ok,
      provider_join_records_verified: ok,
      admin_price_records_verified: ok,
      counts,
      evidence: [
        sourceEvidence,
        `audit:canonical-service:v1#sha256:${summaryDigest}`,
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

function main() {
  try {
    const args = parseArgs(process.argv.slice(2));
    const source = readJsonEvidence(args.snapshot);
    const records = buildRecordMap(source.value);
    const report = auditCanonicalService({
      records,
      sourceEvidence: source.evidence,
      adminOverride: args.adminPubkey,
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

main();

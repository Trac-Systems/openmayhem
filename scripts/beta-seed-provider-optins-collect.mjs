#!/usr/bin/env node
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';
import { blake3 } from '../intercom/node_modules/@tracsystems/blake3/dist/wasm/blake3.mjs';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const defaultManifest = 'config/beta/testnet.json';
const defaultOut = '.mayhem-local/p8.4-launch-evidence/seed-provider-opt-ins.json';
const hex64 = /^[0-9a-fA-F]{64}$/;
const hex128 = /^[0-9a-fA-F]{128}$/;
const ed25519SpkiPrefix = Buffer.from('302a300506032b6570032100', 'hex');

function usage() {
  console.log(`Usage: node scripts/beta-seed-provider-optins-collect.mjs --manifest PATH --provider-report PATH [--provider-report PATH...] [options]

Normalizes real provider lifecycle CLI reports into the P8.4
seed-provider-opt-ins JSON proof consumed by beta-launch-evidence-collect.mjs.
The collector verifies each provider-signed free Mayhem Feature intent, derives
canonical room IDs from the admin-created launch manifest, and fails if any
manifest seed provider opt-in is missing.

Options:
  --manifest PATH              Filled launch manifest (default: ${defaultManifest})
  --provider-report PATH       JSON output from mayhem provider start/join --json
  --out PATH                   Output proof path (default: ${defaultOut})
  --json                       Print JSON report`);
}

function parseArgs(argv) {
  const args = {
    manifest: defaultManifest,
    providerReports: [],
    out: defaultOut,
    json: false,
  };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    const next = () => {
      i += 1;
      if (!argv[i]) throw new Error(`${arg} requires a value`);
      return argv[i];
    };
    if (arg === '--manifest') args.manifest = next();
    else if (arg === '--provider-report') args.providerReports.push(next());
    else if (arg === '--out') args.out = next();
    else if (arg === '--json') args.json = true;
    else if (arg === '-h' || arg === '--help') {
      usage();
      process.exit(0);
    } else {
      throw new Error(`unknown argument: ${arg}`);
    }
  }
  return args;
}

function resolveRepo(filePath) {
  return path.isAbsolute(filePath) ? filePath : path.resolve(repoRoot, filePath);
}

function relativeFile(filePath) {
  const rel = path.relative(repoRoot, filePath);
  return rel.startsWith('..') ? filePath : rel;
}

function readJson(filePath) {
  return JSON.parse(fs.readFileSync(resolveRepo(filePath), 'utf8'));
}

function writeJson(filePath, value) {
  const resolved = resolveRepo(filePath);
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

async function blake3Hex(text) {
  const digest = await blake3(Buffer.from(text));
  return Buffer.from(digest).toString('hex');
}

async function deriveRoomId(enclaveId, adminPubkey, nonce) {
  return (await blake3Hex(`${enclaveId}${adminPubkey}${nonce}`)).slice(0, 32);
}

function providerLifecycleIntentMessage(intent) {
  return `mayhem-provider-lifecycle-v1${stableJson(intent)}`;
}

async function providerLifecycleFeatureKey(intent) {
  return `intent/provider/${intent.provider}/${intent.op}/${await blake3Hex(providerLifecycleIntentMessage(intent))}`;
}

function ed25519PublicKeyFromRawHex(publicKeyHex) {
  return crypto.createPublicKey({
    key: Buffer.concat([ed25519SpkiPrefix, Buffer.from(publicKeyHex, 'hex')]),
    format: 'der',
    type: 'spki',
  });
}

function verifyProviderLifecycleSignature(intent, sig) {
  if (!hex64.test(intent.provider || '') || !hex128.test(sig || '')) return false;
  return crypto.verify(
    null,
    Buffer.from(providerLifecycleIntentMessage(intent)),
    ed25519PublicKeyFromRawHex(intent.provider),
    Buffer.from(sig, 'hex'),
  );
}

function hasPlaceholder(value) {
  return typeof value === 'string' && (
    value.includes('<') ||
    value.includes('>') ||
    /\b(TBD|TODO|REPLACE|PLACEHOLDER|CHANGE_ME)\b/i.test(value)
  );
}

function assertNoPlaceholders(value, name) {
  if (hasPlaceholder(JSON.stringify(value))) {
    throw new Error(`${name} still contains placeholders`);
  }
}

function collectFeatureEnvelopes(value, out = []) {
  if (!value || typeof value !== 'object') return out;
  if (
    value.feature === 'mayhem' &&
    typeof value.key === 'string' &&
    value.value &&
    typeof value.value === 'object' &&
    value.value.op === 'provider_lifecycle'
  ) {
    out.push(value);
  }
  if (Array.isArray(value)) {
    for (const item of value) collectFeatureEnvelopes(item, out);
  } else {
    for (const item of Object.values(value)) collectFeatureEnvelopes(item, out);
  }
  return out;
}

async function expectedOptIns(manifest) {
  const roomLabels = new Map();
  for (const enclave of manifest.canonical_enclaves || []) {
    for (const room of enclave.rooms || []) {
      roomLabels.set(`${enclave.enclave_id}:${room.label}`, {
        room,
        room_id: await deriveRoomId(enclave.enclave_id, manifest.admin.peer_pubkey, room.nonce),
      });
    }
  }
  const expected = [];
  for (const provider of manifest.seed_providers || []) {
    for (const join of provider.joins || []) {
      expected.push({
        op: 'join_enclave',
        provider_pubkey: provider.provider_pubkey,
        enclave_id: join.enclave_id,
      });
      for (const roomLabel of join.rooms || []) {
        const room = roomLabels.get(`${join.enclave_id}:${roomLabel}`);
        if (!room) {
          throw new Error(`seed provider ${provider.provider_pubkey} references unknown room label ${roomLabel} for enclave ${join.enclave_id}`);
        }
        expected.push({
          op: 'join_room',
          provider_pubkey: provider.provider_pubkey,
          enclave_id: join.enclave_id,
          room_label: roomLabel,
          room_id: room.room_id,
        });
      }
    }
  }
  return expected;
}

function featureRow(envelope, sourceFile) {
  const value = envelope.value;
  const intent = value.intent || {};
  return {
    provider_pubkey: intent.provider,
    enclave_id: intent.enclave_id,
    op: intent.op,
    room_id: intent.room_id,
    feature: envelope.feature,
    feature_key: envelope.key,
    sig: value.sig,
    intent,
    source_file: sourceFile,
    result: envelope.result,
  };
}

async function verifiedRows(providerReports) {
  const rows = [];
  const errors = [];
  for (const reportPath of providerReports) {
    const resolved = resolveRepo(reportPath);
    const report = readJson(resolved);
    assertNoPlaceholders(report, reportPath);
    for (const envelope of collectFeatureEnvelopes(report)) {
      const intent = envelope.value.intent || {};
      const row = featureRow(envelope, relativeFile(resolved));
      if (!['register_provider', 'join_enclave', 'join_room'].includes(intent.op)) continue;
      if (!hex64.test(intent.provider || '')) errors.push(`${reportPath}: provider lifecycle intent has invalid provider`);
      if (!hex128.test(envelope.value.sig || '')) errors.push(`${reportPath}: provider lifecycle feature has invalid signature`);
      const expectedKey = await providerLifecycleFeatureKey(intent);
      if (envelope.key !== expectedKey) errors.push(`${reportPath}: feature key mismatch for ${intent.op}`);
      if (!verifyProviderLifecycleSignature(intent, envelope.value.sig)) {
        errors.push(`${reportPath}: provider signature does not verify for ${intent.op}`);
      }
      rows.push(row);
    }
  }
  if (errors.length > 0) throw new Error(errors.join('\n'));
  return rows;
}

function optInRows(rows, expected) {
  return expected
    .filter((required) => required.op === 'join_room')
    .map((required) => {
      const joinEnclave = rows.find((row) => (
        row.op === 'join_enclave' &&
        row.provider_pubkey === required.provider_pubkey &&
        row.enclave_id === required.enclave_id
      ));
      const joinRoom = rows.find((row) => (
        row.op === 'join_room' &&
        row.provider_pubkey === required.provider_pubkey &&
        row.enclave_id === required.enclave_id &&
        row.room_id === required.room_id
      ));
      return {
        provider_pubkey: required.provider_pubkey,
        enclave_id: required.enclave_id,
        rooms: [required.room_label],
        room_ids: [required.room_id],
        feature_keys: {
          join_enclave: joinEnclave?.feature_key,
          join_room: joinRoom?.feature_key,
        },
        signatures: {
          join_enclave: joinEnclave?.sig,
          join_room: joinRoom?.sig,
        },
      };
    });
}

function verifyCoverage(rows, expected) {
  const missing = [];
  for (const required of expected) {
    const ok = rows.some((row) => (
      row.op === required.op &&
      row.provider_pubkey === required.provider_pubkey &&
      row.enclave_id === required.enclave_id &&
      (required.op !== 'join_room' || row.room_id === required.room_id)
    ));
    if (!ok) {
      missing.push(`${required.op}:${required.provider_pubkey}:${required.enclave_id}${required.room_id ? `:${required.room_id}` : ''}`);
    }
  }
  if (missing.length > 0) {
    throw new Error(`missing seed provider lifecycle feature(s): ${missing.join(', ')}`);
  }
}

async function collect(args) {
  if (args.providerReports.length === 0) throw new Error('pass at least one --provider-report');
  const manifest = readJson(args.manifest);
  assertNoPlaceholders(manifest, '--manifest');
  const expected = await expectedOptIns(manifest);
  const rows = await verifiedRows(args.providerReports);
  verifyCoverage(rows, expected);
  const proof = {
    free_feature_lifecycle_records: true,
    proof_kind: 'provider_lifecycle_feature_reports_v1',
    launch_id: manifest.launch_id,
    admin_pubkey: manifest.admin.peer_pubkey,
    provider_reports: args.providerReports.map((file) => relativeFile(resolveRepo(file))),
    opt_ins: optInRows(rows, expected),
    verified_features: rows,
  };
  const out = writeJson(args.out, proof);
  return {
    ok: true,
    out: relativeFile(out),
    expected_features: expected.length,
    verified_features: rows.length,
    opt_ins: proof.opt_ins.length,
    copy_paste_collect_launch_evidence: `node scripts/beta-launch-evidence-collect.mjs --manifest ${relativeFile(resolveRepo(args.manifest))} --seed-provider-opt-ins ${relativeFile(out)}`,
  };
}

async function main() {
  try {
    const args = parseArgs(process.argv.slice(2));
    const report = await collect(args);
    if (args.json) {
      console.log(JSON.stringify(report, null, 2));
    } else {
      console.log('Mayhem seed provider opt-in evidence: ok');
      console.log(`Copy/paste proof path: ${report.out}`);
      console.log(`Copy/paste launch evidence command: ${report.copy_paste_collect_launch_evidence}`);
    }
    if (!report.ok) process.exitCode = 1;
  } catch (error) {
    console.error(`error: ${error.message}`);
    process.exitCode = 1;
  }
}

await main();

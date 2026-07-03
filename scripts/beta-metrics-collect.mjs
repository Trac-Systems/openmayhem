#!/usr/bin/env node
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const defaultOut = 'config/beta/metrics.json';
const hex64 = /^[0-9a-fA-F]{64}$/;

function usage() {
  console.log(`Usage: node scripts/beta-metrics-collect.mjs --window-start ISO --window-end ISO \\
  --providers PATH --users PATH --epoch PATH --guardian PATH --canary PATH \\
  --browser-handoffs PATH --canonical-service PATH --payment-rails PATH \\
  --commit-tx HEX --apply-tx HEX --auditor HEX [--out PATH]

Normalizes beta evidence into config/beta/metrics.json, then runs the strict
P8.5 validator unless --no-validate is passed. Use --tracker-recorded only after
the validated metrics have also been recorded in docs/TRACKER.md.

Accepted evidence shapes are intentionally plain:
- providers/users: array, { data: [] }, { providers: [] }, { users: [] }, or a count record
- epoch: mayhem receipts export --json output, recompute-epoch-roots output, or roots record
- canonical-service: contract-state audit proving admin enclaves/rooms and provider joins
- payment-rails: paygate/rail report proving TNK, Stripe, and Coinbase credit mu_usd
- guardian: small summary JSON; the source file hash is recorded as evidence
- canary: small summary JSON; the source file hash is recorded as evidence
- browser: small summary JSON; browser handoffs may also be a text log`);
}

function parseArgs(argv) {
  const args = {
    out: defaultOut,
    launchId: 'mayhem-testnet-beta-v1',
    auditors: [],
    validate: true,
    trackerRecorded: false,
    json: false,
  };
  const needsValue = new Set([
    '--out',
    '--launch-id',
    '--window-start',
    '--window-end',
    '--providers',
    '--users',
    '--epoch',
    '--guardian',
    '--canary',
    '--browser-handoffs',
    '--canonical-service',
    '--payment-rails',
    '--commit-tx',
    '--apply-tx',
    '--auditor',
  ]);
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '-h' || arg === '--help') {
      usage();
      process.exit(0);
    }
    if (arg === '--no-validate') {
      args.validate = false;
      continue;
    }
    if (arg === '--tracker-recorded') {
      args.trackerRecorded = true;
      continue;
    }
    if (arg === '--json') {
      args.json = true;
      continue;
    }
    if (!needsValue.has(arg)) throw new Error(`unknown argument: ${arg}`);
    i += 1;
    if (!argv[i]) throw new Error(`${arg} requires a value`);
    const key = arg.slice(2).replace(/-([a-z])/g, (_, ch) => ch.toUpperCase());
    if (key === 'auditor') args.auditors.push(argv[i]);
    else args[key] = argv[i];
  }
  return args;
}

function requireArg(args, name) {
  if (!args[name]) throw new Error(`--${name.replace(/[A-Z]/g, (ch) => `-${ch.toLowerCase()}`)} is required`);
  return args[name];
}

function resolveInput(filePath) {
  return path.resolve(repoRoot, filePath);
}

function relativeFile(filePath) {
  const rel = path.relative(repoRoot, filePath);
  return rel.startsWith('..') ? filePath : rel;
}

function readFileEvidence(filePath) {
  const resolved = resolveInput(filePath);
  const bytes = fs.readFileSync(resolved);
  const sha256 = crypto.createHash('sha256').update(bytes).digest('hex');
  return {
    path: resolved,
    text: bytes.toString('utf8'),
    evidence: `file:${relativeFile(resolved)}#sha256:${sha256}`,
  };
}

function readJsonEvidence(filePath) {
  const evidence = readFileEvidence(filePath);
  return {
    ...evidence,
    value: JSON.parse(evidence.text),
  };
}

function parseMaybeJsonEvidence(filePath) {
  const evidence = readFileEvidence(filePath);
  const trimmed = evidence.text.trim();
  if (trimmed.startsWith('{') || trimmed.startsWith('[')) {
    return { ...evidence, value: JSON.parse(trimmed) };
  }
  return { ...evidence, value: null };
}

function getPath(value, dotted) {
  return dotted.split('.').reduce((acc, key) => {
    if (acc === undefined || acc === null) return undefined;
    return acc[key];
  }, value);
}

function firstDefined(value, paths) {
  for (const dotted of paths) {
    const found = getPath(value, dotted);
    if (found !== undefined && found !== null) return found;
  }
  return undefined;
}

function asArrayFrom(value, paths) {
  for (const dotted of paths) {
    const found = getPath(value, dotted);
    if (Array.isArray(found)) return found;
  }
  if (Array.isArray(value)) return value;
  return null;
}

function asEvidenceArray(value, paths = ['evidence']) {
  const found = asArrayFrom(value, paths);
  return found ? found.filter((item) => typeof item === 'string' && item.length > 0) : [];
}

function checkoutUrlFromPayReport(value) {
  const url = firstDefined(value, ['copy_paste.checkout_url', 'checkout.url']);
  return typeof url === 'string' && /^https?:\/\//i.test(url) ? url : null;
}

function checkoutUrlsFromText(text) {
  const urls = [];
  const pattern = /Copy\/paste checkout URL:\s*(https?:\/\/\S+)/gi;
  let match = pattern.exec(text);
  while (match) {
    urls.push(match[1].replace(/[),.;]+$/g, ''));
    match = pattern.exec(text);
  }
  return urls;
}

function checkoutUrlEvidence(urls, sourceEvidence) {
  return urls.map((url) => `${sourceEvidence}#copy_paste.checkout_url:${url}`);
}

function checkoutUrlEvidenceFromJson(value, sourceEvidence) {
  const records = Array.isArray(value)
    ? value
    : (asArrayFrom(value, ['browser_handoffs.reports', 'reports', 'data']) ?? [value]);
  const samples = [];
  for (const record of records) {
    if (!record || typeof record !== 'object' || Array.isArray(record)) continue;
    const url = checkoutUrlFromPayReport(record);
    if (url) samples.push(...checkoutUrlEvidence([url], sourceEvidence));
  }
  return samples;
}

function stableJson(value) {
  if (Array.isArray(value)) return `[${value.map(stableJson).join(',')}]`;
  if (value && typeof value === 'object') {
    return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${stableJson(value[key])}`).join(',')}}`;
  }
  return JSON.stringify(value);
}

function identityOf(record, fields, label) {
  if (typeof record === 'string' && record.length > 0) return record;
  if (!record || typeof record !== 'object' || Array.isArray(record)) {
    throw new Error(`${label} record must be a string or object`);
  }
  for (const field of fields) {
    const value = getPath(record, field);
    if (typeof value === 'string' && value.length > 0) return value;
  }
  throw new Error(`${label} record is missing an identity field (${fields.join(', ')})`);
}

function countIdentities(value, { label, countPaths, arrayPaths, idFields, filter }) {
  const explicit = firstDefined(value, countPaths);
  const evidence = asEvidenceArray(value, [
    'evidence',
    `${label}.evidence`,
    `participants.${label}.evidence`,
  ]);
  if (Number.isInteger(explicit)) return { count: explicit, evidence };

  const records = asArrayFrom(value, arrayPaths);
  if (!records) throw new Error(`${label} evidence must include a count or records array`);
  const ids = new Set();
  for (const record of records) {
    if (filter && !filter(record)) continue;
    ids.add(identityOf(record, idFields, label));
  }
  return { count: ids.size, evidence };
}

function providerIsExternal(record) {
  if (!record || typeof record !== 'object' || Array.isArray(record)) return true;
  if (record.external === false || record.internal === true) return false;
  if (record.role && !['provider', 'both'].includes(String(record.role))) return false;
  return true;
}

function collectParticipants(args) {
  const providers = readJsonEvidence(requireArg(args, 'providers'));
  const users = readJsonEvidence(requireArg(args, 'users'));
  const providerSummary = countIdentities(providers.value, {
    label: 'external_providers',
    countPaths: [
      'participants.external_providers.count',
      'external_providers.count',
      'provider_count',
      'count',
    ],
    arrayPaths: [
      'participants.external_providers.records',
      'external_providers.records',
      'external_providers',
      'providers',
      'data',
    ],
    idFields: ['provider', 'provider_pubkey', 'public_key', 'pubkey', 'id', 'address', 'who'],
    filter: providerIsExternal,
  });
  const userSummary = countIdentities(users.value, {
    label: 'users',
    countPaths: ['participants.users.count', 'users.count', 'user_count', 'count'],
    arrayPaths: [
      'participants.users.records',
      'users.records',
      'users',
      'activity',
      'sessions',
      'receipts',
      'data',
    ],
    idFields: ['user', 'user_pubkey', 'public_key', 'pubkey', 'id', 'address', 'who', 'receipt.body.user', 'body.user'],
  });
  return {
    external_providers: {
      count: providerSummary.count,
      evidence: [providers.evidence, ...providerSummary.evidence],
    },
    users: {
      count: userSummary.count,
      evidence: [users.evidence, ...userSummary.evidence],
    },
  };
}

function collectEpoch(args) {
  const source = readJsonEvidence(requireArg(args, 'epoch'));
  const value = source.value;
  const roots = firstDefined(value, ['audited_epoch.roots', 'recomputed.roots', 'roots']);
  if (!roots || typeof roots !== 'object') throw new Error('epoch evidence is missing roots');
  const epoch = firstDefined(value, ['audited_epoch.epoch', 'recomputed.epoch', 'bundle.epoch', 'epoch']);
  const checks = Array.isArray(value.checks) ? value.checks : [];
  const checksOk = checks.length > 0 && checks.every((check) => check && check.ok === true);
  const verified = value.verified === true || checksOk || value.receipt_batches_verified === true;
  const commitTx = args.commitTx
    ?? firstDefined(value, ['audited_epoch.commit_tx', 'commit_tx', 'epoch_commit.tx_hash', 'commit.tx_hash']);
  const applyTx = args.applyTx
    ?? firstDefined(value, ['audited_epoch.apply_tx', 'apply_tx', 'epoch_apply.tx_hash', 'apply.tx_hash']);
  const auditors = [
    ...args.auditors,
    ...(asArrayFrom(value, ['audited_epoch.auditors', 'auditors']) ?? []),
  ];
  return {
    epoch,
    committed: true,
    applied: true,
    commit_tx: commitTx,
    apply_tx: applyTx,
    roots: {
      dep: roots.dep,
      use: roots.use,
      earn: roots.earn,
      fee: roots.fee,
      pay: roots.pay,
    },
    receipt_batches_verified: verified,
    payout_evidence_verified: value.payout_evidence_verified === true || verified,
    auditors: Array.from(new Set(auditors.filter((item) => typeof item === 'string' && item.length > 0))),
    evidence: [
      source.evidence,
      ...asEvidenceArray(value, ['audited_epoch.evidence', 'evidence']),
    ],
  };
}

function collectGuardian(args) {
  const source = readJsonEvidence(requireArg(args, 'guardian'));
  const value = source.value;
  const trips = firstDefined(value, ['guardian.trips', 'trips']);
  const tripArray = asArrayFrom(value, ['guardian.trip_records', 'trip_records', 'trips']);
  return {
    trips: Number.isInteger(trips) ? trips : (tripArray ? tripArray.length : 0),
    conservation_ok: firstDefined(value, ['guardian.conservation_ok', 'conservation_ok']) === true,
    monotonic_epochs: firstDefined(value, ['guardian.monotonic_epochs', 'monotonic_epochs']) === true,
    evidence: [source.evidence, ...asEvidenceArray(value, ['guardian.evidence', 'evidence', 'guardian.notes', 'notes'])],
  };
}

function collectCanary(args) {
  const source = readJsonEvidence(requireArg(args, 'canary'));
  const value = source.value;
  const probeRecords = asArrayFrom(value, ['canary.probe_records', 'probe_records', 'probes']);
  const explicitProbes = firstDefined(value, ['canary.probes', 'summary.probes', 'probes']);
  const probes = Number.isInteger(explicitProbes) ? explicitProbes : (probeRecords ? probeRecords.length : 0);
  const explicitFailures = firstDefined(value, ['canary.failures', 'summary.failures', 'failures']);
  const failures = Number.isInteger(explicitFailures)
    ? explicitFailures
    : (probeRecords ? probeRecords.filter((probe) => probe && probe.pass === false).length : 0);
  return {
    set_id: firstDefined(value, ['canary.set_id', 'set_id']) ?? 'canary-launch-v1',
    probes,
    failures,
    evidence: [source.evidence, ...asEvidenceArray(value, ['canary.evidence', 'evidence'])],
  };
}

function collectBrowserHandoffs(args) {
  const source = parseMaybeJsonEvidence(requireArg(args, 'browserHandoffs'));
  if (source.value && typeof source.value === 'object') {
    const checkoutSamples = checkoutUrlEvidenceFromJson(source.value, source.evidence);
    const explicitCopyPaste = firstDefined(source.value, [
      'browser_handoffs.copy_paste_urls_printed',
      'copy_paste_urls_printed',
    ]);
    return {
      copy_paste_urls_printed: explicitCopyPaste === true || checkoutSamples.length > 0,
      samples: [
        source.evidence,
        ...checkoutSamples,
        ...asEvidenceArray(source.value, ['browser_handoffs.samples', 'samples', 'evidence']),
      ],
    };
  }
  const checkoutSamples = checkoutUrlEvidence(checkoutUrlsFromText(source.text), source.evidence);
  return {
    copy_paste_urls_printed: checkoutSamples.length > 0,
    samples: [source.evidence, ...checkoutSamples],
  };
}

function collectCanonicalService(args) {
  const source = readJsonEvidence(requireArg(args, 'canonicalService'));
  const value = source.value;
  const record = firstDefined(value, ['canonical_service']) ?? value;
  return {
    admin_created_enclaves_verified: firstDefined(record, ['admin_created_enclaves_verified']) === true,
    admin_created_rooms_verified: firstDefined(record, ['admin_created_rooms_verified']) === true,
    provider_join_records_verified: firstDefined(record, ['provider_join_records_verified']) === true,
    admin_price_records_verified: firstDefined(record, ['admin_price_records_verified']) === true,
    evidence: [
      source.evidence,
      ...asEvidenceArray(value, ['canonical_service.evidence', 'evidence']),
    ],
  };
}

function collectPaymentRails(args) {
  const source = readJsonEvidence(requireArg(args, 'paymentRails'));
  const value = source.value;
  const record = firstDefined(value, ['payment_rails']) ?? value;
  return {
    ledger_denom: firstDefined(record, ['ledger_denom', 'denom', 'network.denom']) ?? firstDefined(value, ['network.denom']),
    tnk_enabled: firstDefined(record, ['tnk_enabled', 'tnk.enabled', 'rails.tnk.enabled']) === true,
    stripe_enabled:
      firstDefined(record, ['stripe_enabled', 'stripe.enabled', 'paygate.stripe_enabled', 'rails.stripe.enabled']) === true
      || firstDefined(value, ['paygate.stripe_enabled', 'rails.stripe.enabled']) === true,
    coinbase_enabled:
      firstDefined(record, ['coinbase_enabled', 'coinbase.enabled', 'paygate.coinbase_enabled', 'rails.coinbase.enabled']) === true
      || firstDefined(value, ['paygate.coinbase_enabled', 'rails.coinbase.enabled']) === true,
    rails_credit_mu_usd: firstDefined(record, ['rails_credit_mu_usd', 'credit_mu_usd', 'credits_mu_usd']) === true,
    evidence: [
      source.evidence,
      ...asEvidenceArray(value, ['payment_rails.evidence', 'evidence']),
    ],
  };
}

function requireHex64(value, label) {
  if (typeof value !== 'string' || !hex64.test(value)) {
    throw new Error(`${label} must be a 64-character hex string`);
  }
  return value.toLowerCase();
}

function buildMetrics(args) {
  const participants = collectParticipants(args);
  const auditedEpoch = collectEpoch(args);
  const guardian = collectGuardian(args);
  const canary = collectCanary(args);
  const browserHandoffs = collectBrowserHandoffs(args);
  const canonicalService = collectCanonicalService(args);
  const paymentRails = collectPaymentRails(args);

  for (const key of ['dep', 'use', 'earn', 'fee', 'pay']) {
    auditedEpoch.roots[key] = requireHex64(auditedEpoch.roots[key], `audited_epoch.roots.${key}`);
  }
  auditedEpoch.commit_tx = requireHex64(auditedEpoch.commit_tx, 'audited_epoch.commit_tx');
  auditedEpoch.apply_tx = requireHex64(auditedEpoch.apply_tx, 'audited_epoch.apply_tx');
  auditedEpoch.auditors = auditedEpoch.auditors.map((auditor, index) => requireHex64(auditor, `audited_epoch.auditors[${index}]`));

  return {
    schema_version: 1,
    launch_id: args.launchId,
    network: {
      name: 'testnet1',
      denom: 'mu_usd',
      msb: {
        address_prefix: 'testtrac',
        network_id: 919,
      },
    },
    payment_rails: paymentRails,
    window: {
      started_at: requireArg(args, 'windowStart'),
      ended_at: requireArg(args, 'windowEnd'),
    },
    controls: {
      admin_controls_economy: true,
      providers_set_prices: false,
      providers_submit_models: false,
      providers_create_canonical_rooms: false,
      providers_only_join_admin_rooms: true,
    },
    canonical_service: canonicalService,
    participants,
    audited_epoch: auditedEpoch,
    guardian,
    canary,
    browser_handoffs: browserHandoffs,
    tracker: {
      metrics_recorded: args.trackerRecorded,
      notes: args.trackerRecorded
        ? 'Validated metrics were recorded in docs/TRACKER.md.'
        : 'Set --tracker-recorded only after docs/TRACKER.md has the validated beta metrics row/note.',
    },
  };
}

function writeJson(filePath, value) {
  const resolved = path.resolve(repoRoot, filePath);
  fs.mkdirSync(path.dirname(resolved), { recursive: true });
  fs.writeFileSync(resolved, `${JSON.stringify(value, null, 2)}\n`);
  return resolved;
}

function validateMetrics(outPath) {
  const child = spawnSync(process.execPath, ['scripts/beta-metrics.mjs', '--metrics', outPath], {
    cwd: repoRoot,
    encoding: 'utf8',
  });
  return {
    ok: child.status === 0,
    status: child.status,
    stdout: child.stdout,
    stderr: child.stderr,
  };
}

function printHuman(report) {
  console.log(`Mayhem beta metrics collected: ${report.validator?.ok === false ? 'not ready' : 'ok'}`);
  console.log(`Copy/paste metrics path: ${report.metrics_path}`);
  console.log(`Copy/paste validate command: node scripts/beta-metrics.mjs --metrics ${report.metrics_arg}`);
  console.log(`External providers: ${report.external_providers}`);
  console.log(`Users: ${report.users}`);
  console.log(`Audited epoch: ${report.audited_epoch}`);
  console.log(`Guardian trips: ${report.guardian_trips}`);
  if (report.validator && !report.validator.ok) {
    process.stdout.write(report.validator.stdout);
    process.stderr.write(report.validator.stderr);
  }
}

function main() {
  try {
    const args = parseArgs(process.argv.slice(2));
    const metrics = buildMetrics(args);
    const outPath = writeJson(args.out, metrics);
    const metricsArg = path.isAbsolute(args.out) ? args.out : path.relative(repoRoot, outPath);
    const validator = args.validate ? validateMetrics(metricsArg) : null;
    const report = {
      ok: !validator || validator.ok,
      metrics_path: outPath,
      metrics_arg: metricsArg,
      external_providers: metrics.participants.external_providers.count,
      users: metrics.participants.users.count,
      audited_epoch: metrics.audited_epoch.epoch,
      guardian_trips: metrics.guardian.trips,
      evidence_digest: crypto.createHash('sha256').update(stableJson(metrics)).digest('hex'),
      validator,
    };
    if (args.json) console.log(JSON.stringify(report, null, 2));
    else printHuman(report);
    if (validator && !validator.ok) process.exitCode = validator.status || 1;
  } catch (error) {
    console.error(`error: ${error.message}`);
    process.exitCode = 1;
  }
}

main();

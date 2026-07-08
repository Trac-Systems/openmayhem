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
const canonicalLedgerRails = ['fiat', 'tap', 'tnk'];

function usage() {
  console.log(`Usage: node scripts/beta-metrics-collect.mjs --window-start ISO --window-end ISO \\
  --launch-manifest PATH --providers PATH --users PATH --epoch PATH --guardian PATH --canary PATH \\
  --browser-handoffs PATH --canonical-service PATH --payment-rails PATH \\
  --commit-tx HEX --apply-tx HEX --auditor HEX [--out PATH]

Normalizes beta rehearsal evidence into config/beta/metrics.json, then runs the
strict P8.5 validator unless --no-validate is passed. Use --tracker-recorded
only after the validated metrics have also been recorded in docs/TRACKER.md.

Accepted evidence shapes are intentionally plain:
- providers/users: array, { data: [] }, { providers: [] }, or { users: [] }; count-only summaries are rejected; synthetic records are accepted
- launch-manifest: a strict P8.4 beta launch manifest accepted by scripts/beta-launch.mjs
- epoch: mayhem receipts export --json output, recompute-epoch-roots output, or roots record
- canonical-service: contract-state audit proving admin enclaves/rooms and provider joins
- payment-rails: rail report proving fiat, TAP, and TNK credit au_usd; Stripe is only the fiat processor
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
    '--launch-manifest',
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

function normalizeRail(value) {
  const normalized = typeof value === 'string' ? value.toLowerCase() : value;
  if (normalized === 'stripe') return 'fiat';
  return canonicalLedgerRails.includes(normalized) ? normalized : null;
}

function railFromUrl(url) {
  let parsed;
  try {
    parsed = new URL(url);
  } catch {
    return null;
  }
  const hostname = parsed.hostname.toLowerCase();
  if (hostname === 'checkout.stripe.com') return 'fiat';
  return null;
}

function processorFromUrl(url) {
  try {
    const parsed = new URL(url);
    if (parsed.hostname.toLowerCase() === 'checkout.stripe.com') return 'stripe';
  } catch {
    return null;
  }
  return null;
}

function checkoutRecordsFromText(text) {
  const records = [];
  const pattern = /Copy\/paste checkout URL:\s*(https?:\/\/\S+)/gi;
  let match = pattern.exec(text);
  while (match) {
    const prefix = text.slice(Math.max(0, match.index - 160), match.index);
    const railMatches = Array.from(prefix.matchAll(/Mayhem\s+(fiat|stripe)\s+checkout/gi));
    const railMatch = railMatches.at(-1);
    const url = match[1].replace(/[),.;]+$/g, '');
    records.push({ url, rail: normalizeRail(railMatch?.[1]) ?? railFromUrl(url) });
    match = pattern.exec(text);
  }
  return records;
}

function checkoutUrlEvidence(records, sourceEvidence) {
  return records.map(({ url, rail }) => {
    const railSuffix = rail ? `#rail:${rail}` : '';
    const processor = processorFromUrl(url);
    const processorSuffix = processor ? `#processor:${processor}` : '';
    return `${sourceEvidence}#copy_paste.checkout_url:${url}${railSuffix}${processorSuffix}`;
  });
}

function checkoutRecordsFromJson(value) {
  const records = Array.isArray(value)
    ? value
    : (asArrayFrom(value, ['browser_handoffs.reports', 'reports', 'data']) ?? [value]);
  const checkoutRecords = [];
  for (const record of records) {
    if (!record || typeof record !== 'object' || Array.isArray(record)) continue;
    const url = checkoutUrlFromPayReport(record);
    if (url) {
      const rail = normalizeRail(firstDefined(record, ['rail', 'checkout.rail'])) ?? railFromUrl(url);
      checkoutRecords.push({ url, rail });
    }
  }
  return checkoutRecords;
}

function railsFromCheckoutRecords(records) {
  const rails = new Set(records.map((record) => record.rail).map(normalizeRail).filter(Boolean));
  return canonicalLedgerRails.filter((rail) => rails.has(rail));
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

function countIdentities(value, { label, arrayPaths, idFields, filter }) {
  const evidence = asEvidenceArray(value, [
    'evidence',
    `${label}.evidence`,
    `participants.${label}.evidence`,
  ]);

  const records = asArrayFrom(value, arrayPaths);
  if (!records) throw new Error(`${label} evidence must include identity records; count-only summaries are not accepted`);
  const ids = new Set();
  for (const record of records) {
    if (filter && !filter(record)) continue;
    ids.add(identityOf(record, idFields, label));
  }
  return { count: ids.size, identityRecordsVerified: true, evidence };
}

function providerRecordCounts(record) {
  if (!record || typeof record !== 'object' || Array.isArray(record)) return true;
  if (record.role && !['provider', 'both'].includes(String(record.role))) return false;
  return true;
}

function collectParticipants(args) {
  const providers = readJsonEvidence(requireArg(args, 'providers'));
  const users = readJsonEvidence(requireArg(args, 'users'));
  const providerSummary = countIdentities(providers.value, {
    label: 'external_providers',
    arrayPaths: [
      'participants.external_providers.records',
      'external_providers.records',
      'external_providers',
      'providers',
      'data',
    ],
    idFields: ['provider', 'provider_pubkey', 'public_key', 'pubkey', 'id', 'address', 'who'],
    filter: providerRecordCounts,
  });
  const userSummary = countIdentities(users.value, {
    label: 'users',
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
      identity_records_verified: providerSummary.identityRecordsVerified,
      evidence: [providers.evidence, ...providerSummary.evidence],
    },
    users: {
      count: userSummary.count,
      identity_records_verified: userSummary.identityRecordsVerified,
      evidence: [users.evidence, ...userSummary.evidence],
    },
  };
}

function collectLaunch(args) {
  const source = readJsonEvidence(requireArg(args, 'launchManifest'));
  const manifestArg = path.isAbsolute(args.launchManifest)
    ? args.launchManifest
    : relativeFile(source.path);
  const child = spawnSync(process.execPath, [
    'scripts/beta-launch.mjs',
    '--manifest',
    manifestArg,
    '--json',
    '--no-commands',
  ], {
    cwd: repoRoot,
    encoding: 'utf8',
  });
  if (child.status !== 0) {
    const details = `${child.stdout || ''}${child.stderr || ''}`.trim();
    throw new Error(`launch manifest did not pass strict beta-launch validation${details ? `: ${details}` : ''}`);
  }
  const report = JSON.parse(child.stdout);
  return {
    manifest_path: relativeFile(source.path),
    manifest_validated: true,
    canonical_enclaves: report.counts?.canonical_enclaves ?? source.value.canonical_enclaves?.length ?? 0,
    canonical_rooms: report.counts?.canonical_rooms ?? source.value.canonical_enclaves?.flatMap((enclave) => enclave.rooms || []).length ?? 0,
    seed_providers: report.counts?.seed_providers ?? source.value.seed_providers?.length ?? 0,
    evidence: [
      source.evidence,
      ...asEvidenceArray(source.value, ['launch.evidence', 'evidence.launch']),
    ],
  };
}

function collectEpoch(args) {
  const source = readJsonEvidence(requireArg(args, 'epoch'));
  const value = source.value;
  const roots = firstDefined(value, ['audited_epoch.roots', 'recomputed.roots', 'roots']);
  if (!roots || typeof roots !== 'object') throw new Error('epoch evidence is missing roots');
  const epoch = firstDefined(value, ['audited_epoch.epoch', 'recomputed.epoch', 'bundle.epoch', 'epoch']);
  const feeBps = firstDefined(value, [
    'audited_epoch.params.fee_bps',
    'recomputed.params.fee_bps',
    'bundle.params.fee_bps',
    'params.fee_bps',
  ]);
  if (!Number.isInteger(feeBps) || feeBps < 0 || feeBps > 5_000) {
    throw new Error('epoch evidence params.fee_bps must be the admin-set fee_bps integer between 0 and 5000');
  }
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
    params: {
      fee_bps: feeBps,
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
    evidence: [source.evidence, ...asEvidenceArray(value, ['guardian.evidence', 'evidence'])],
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
    const checkoutRecords = checkoutRecordsFromJson(source.value);
    const checkoutSamples = checkoutUrlEvidence(checkoutRecords, source.evidence);
    const explicitCopyPaste = firstDefined(source.value, [
      'browser_handoffs.copy_paste_urls_printed',
      'copy_paste_urls_printed',
    ]);
    const explicitRails = asArrayFrom(source.value, ['browser_handoffs.rails_verified', 'rails_verified'])
      ?.map(normalizeRail)
      .filter(Boolean) ?? [];
    return {
      copy_paste_urls_printed: explicitCopyPaste === true || checkoutSamples.length > 0,
      rails_verified: canonicalLedgerRails.filter((rail) => (
        new Set([...railsFromCheckoutRecords(checkoutRecords), ...explicitRails]).has(rail)
      )),
      samples: [
        source.evidence,
        ...checkoutSamples,
        ...asEvidenceArray(source.value, ['browser_handoffs.samples', 'samples', 'evidence']),
      ],
    };
  }
  const checkoutRecords = checkoutRecordsFromText(source.text);
  const checkoutSamples = checkoutUrlEvidence(checkoutRecords, source.evidence);
  return {
    copy_paste_urls_printed: checkoutSamples.length > 0,
    rails_verified: railsFromCheckoutRecords(checkoutRecords),
    samples: [source.evidence, ...checkoutSamples],
  };
}

function collectCanonicalService(args) {
  const source = readJsonEvidence(requireArg(args, 'canonicalService'));
  const value = source.value;
  const record = firstDefined(value, ['canonical_service']) ?? value;
  const controls = firstDefined(value, ['controls']) ?? {};
  return {
    canonical_service: {
      admin_created_enclaves_verified: firstDefined(record, ['admin_created_enclaves_verified']) === true,
      admin_catalog_records_verified: firstDefined(record, ['admin_catalog_records_verified']) === true,
      admin_created_rooms_verified: firstDefined(record, ['admin_created_rooms_verified']) === true,
      provider_join_records_verified: firstDefined(record, ['provider_join_records_verified']) === true,
      admin_price_records_verified: firstDefined(record, ['admin_price_records_verified']) === true,
      admin_payout_records_verified: firstDefined(record, ['admin_payout_records_verified']) === true,
      admin_rules_records_verified: firstDefined(record, ['admin_rules_records_verified']) === true,
      admin_params_records_verified: firstDefined(record, ['admin_params_records_verified']) === true,
      admin_provider_ban_records_verified: firstDefined(record, ['admin_provider_ban_records_verified']) === true,
      evidence: [
        source.evidence,
        ...asEvidenceArray(value, ['canonical_service.evidence', 'evidence']),
      ],
    },
    controls: {
      admin_controls_economy: firstDefined(controls, ['admin_controls_economy']),
      admin_sets_prices: firstDefined(controls, ['admin_sets_prices']),
      admin_sets_rules: firstDefined(controls, ['admin_sets_rules']),
      admin_sets_params: firstDefined(controls, ['admin_sets_params']),
      admin_sets_provider_payout_targets: firstDefined(controls, ['admin_sets_provider_payout_targets']),
      admin_can_ban_providers: firstDefined(controls, ['admin_can_ban_providers']),
      providers_set_prices: firstDefined(controls, ['providers_set_prices']),
      providers_set_rules: firstDefined(controls, ['providers_set_rules']),
      providers_set_params: firstDefined(controls, ['providers_set_params']),
      providers_set_payout_terms: firstDefined(controls, ['providers_set_payout_terms']),
      providers_submit_models: firstDefined(controls, ['providers_submit_models']),
      providers_create_canonical_rooms: firstDefined(controls, ['providers_create_canonical_rooms']),
      providers_only_join_admin_rooms: firstDefined(controls, ['providers_only_join_admin_rooms']),
      provider_payout_targets_admin_verified: firstDefined(controls, ['provider_payout_targets_admin_verified']),
      admin_rules_params_verified: firstDefined(controls, ['admin_rules_params_verified']),
      evidence: [
        source.evidence,
        ...asEvidenceArray(value, ['controls.evidence', 'evidence']),
      ],
    },
  };
}

function paygateAdminControlsVerified(value) {
  const controls = firstDefined(value, [
    'payment_rails.controls',
    'paygate.controls',
    'controls',
  ]);
  if (!controls || typeof controls !== 'object' || Array.isArray(controls)) return false;
  const expectedTrue = [
    'admin_controls_economy',
    'admin_sets_terms',
    'admin_sets_prices',
    'admin_sets_rules',
    'admin_sets_params',
    'admin_sets_provider_payout_targets',
    'admin_can_ban_providers',
    'providers_only_join_admin_rooms',
    'provider_payout_targets_admin_verified',
  ];
  const expectedFalse = [
    'providers_set_prices',
    'providers_set_rules',
    'providers_set_params',
    'providers_set_payout_terms',
    'providers_submit_models',
    'providers_create_canonical_rooms',
  ];
  return expectedTrue.every((key) => controls[key] === true)
    && expectedFalse.every((key) => controls[key] === false);
}

function collectPaymentRails(args) {
  const source = readJsonEvidence(requireArg(args, 'paymentRails'));
  const value = source.value;
  const record = firstDefined(value, ['payment_rails']) ?? value;
  const stripeProcessorEnabled =
    firstDefined(record, ['stripe_processor_enabled', 'stripe.enabled', 'paygate.stripe_enabled', 'rails.stripe.enabled']) === true
    || firstDefined(value, ['paygate.stripe_enabled', 'rails.stripe.enabled']) === true
    || firstDefined(record, ['stripe_enabled']) === true;
  return {
    ledger_denom: firstDefined(record, ['ledger_denom', 'denom', 'network.denom']) ?? firstDefined(value, ['network.denom']),
    fiat_enabled:
      firstDefined(record, ['fiat_enabled', 'fiat.enabled', 'rails.fiat.enabled']) === true
      || stripeProcessorEnabled,
    tap_enabled: firstDefined(record, ['tap_enabled', 'tap.enabled', 'rails.tap.enabled']) === true,
    tnk_enabled: firstDefined(record, ['tnk_enabled', 'tnk.enabled', 'rails.tnk.enabled']) === true,
    stripe_processor_enabled: stripeProcessorEnabled,
    rails_credit_au_usd: firstDefined(record, ['rails_credit_au_usd', 'credit_au_usd', 'credits_au_usd']) === true,
    paygate_admin_controls_verified:
      firstDefined(record, ['paygate_admin_controls_verified', 'admin_economy_controls_verified']) === true
      || paygateAdminControlsVerified(value),
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
  const launch = collectLaunch(args);
  const participants = collectParticipants(args);
  const auditedEpoch = collectEpoch(args);
  const guardian = collectGuardian(args);
  const canary = collectCanary(args);
  const browserHandoffs = collectBrowserHandoffs(args);
  const canonicalAudit = collectCanonicalService(args);
  const paymentRails = collectPaymentRails(args);

  for (const key of ['dep', 'use', 'earn', 'fee', 'pay']) {
    auditedEpoch.roots[key] = requireHex64(auditedEpoch.roots[key], `audited_epoch.roots.${key}`);
  }
  if (
    !auditedEpoch.params ||
    !Number.isInteger(auditedEpoch.params.fee_bps) ||
    auditedEpoch.params.fee_bps < 0 ||
    auditedEpoch.params.fee_bps > 5_000
  ) {
    throw new Error('audited_epoch.params.fee_bps must be the admin-set fee_bps integer between 0 and 5000');
  }
  auditedEpoch.commit_tx = requireHex64(auditedEpoch.commit_tx, 'audited_epoch.commit_tx');
  auditedEpoch.apply_tx = requireHex64(auditedEpoch.apply_tx, 'audited_epoch.apply_tx');
  auditedEpoch.auditors = auditedEpoch.auditors.map((auditor, index) => requireHex64(auditor, `audited_epoch.auditors[${index}]`));

  return {
    schema_version: 1,
    launch_id: args.launchId,
    launch,
    network: {
      name: 'testnet1',
      denom: 'au_usd',
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
    controls: canonicalAudit.controls,
    canonical_service: canonicalAudit.canonical_service,
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
  console.log(`Provider records: ${report.external_providers}`);
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

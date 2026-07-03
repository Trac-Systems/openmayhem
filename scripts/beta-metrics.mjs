#!/usr/bin/env node
import fs from 'node:fs';
import crypto from 'node:crypto';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const defaultMetrics = 'config/beta/metrics.template.json';
const hex64 = /^[0-9a-fA-F]{64}$/;
const isoUtc = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z$/;
const sha256Evidence = /#sha256:[0-9a-fA-F]{64}(?:$|[#?&])/;
const thresholds = {
  externalProviders: 20,
  users: 100,
};

function usage() {
  console.log(`Usage: node scripts/beta-metrics.mjs [--metrics PATH] [--allow-placeholders] [--json]

Validates Mayhem P8.5 beta exit metrics. Strict mode fails on template
placeholders and on any unmet threshold. Use --allow-placeholders only to
validate the committed template shape.`);
}

function parseArgs(argv) {
  const args = {
    metrics: defaultMetrics,
    allowPlaceholders: false,
    json: false,
  };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--metrics') {
      i += 1;
      if (!argv[i]) throw new Error('--metrics requires a path');
      args.metrics = argv[i];
    } else if (arg === '--allow-placeholders') {
      args.allowPlaceholders = true;
    } else if (arg === '--json') {
      args.json = true;
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

function requireArray(add, value, name, min = 0) {
  if (!Array.isArray(value)) {
    add('error', `${name} must be an array`);
    return false;
  }
  if (value.length < min) add('error', `${name} must contain at least ${min} item(s)`);
  return true;
}

function requireLiteral(add, value, expected, name) {
  if (value !== expected) add('error', `${name} must be ${JSON.stringify(expected)}`);
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

function requireIntegerAtLeast(add, value, min, name) {
  if (isPlaceholder(value)) {
    add('placeholder', `${name} still contains a template placeholder`);
    return;
  }
  if (!Number.isInteger(value) || value < min) {
    add('error', `${name} must be an integer >= ${min}`);
  }
}

function requireBoolean(add, value, expected, name) {
  if (value !== expected) add('error', `${name} must be ${expected}`);
}

function relativeFile(filePath) {
  const rel = path.relative(repoRoot, filePath);
  return rel.startsWith('..') ? filePath : rel;
}

function sha256File(filePath) {
  return crypto.createHash('sha256').update(fs.readFileSync(filePath)).digest('hex');
}

function validateBoundFileEvidence(add, evidence, filePath, name) {
  if (!Array.isArray(evidence) || evidence.some((item) => isPlaceholder(item))) return;
  const resolved = path.resolve(repoRoot, filePath);
  if (!fs.existsSync(resolved)) {
    add('error', `${name} does not exist: ${filePath}`);
    return;
  }
  const expected = `file:${relativeFile(resolved)}#sha256:${sha256File(resolved)}`;
  if (!evidence.includes(expected)) {
    add('error', `${name} evidence must include ${expected}`);
  }
}

function validateEvidenceArray(add, value, name) {
  if (!requireArray(add, value, name, 1)) return;
  for (const [index, item] of value.entries()) {
    const itemName = `${name}[${index}]`;
    requireString(add, item, itemName);
    if (typeof item !== 'string' || isPlaceholder(item)) continue;
    if (!sha256Evidence.test(item)) {
      add('error', `${itemName} must include #sha256:<64-hex> durable evidence`);
    }
  }
}

function hasEvidenceTag(value, tag) {
  return typeof value === 'string' && value.split('#').includes(tag);
}

function checkoutUrlFromEvidence(value) {
  if (typeof value !== 'string') return null;
  const marker = '#copy_paste.checkout_url:';
  const offset = value.indexOf(marker);
  if (offset === -1) return null;
  const rest = value.slice(offset + marker.length);
  const end = rest.indexOf('#');
  return end === -1 ? rest : rest.slice(0, end);
}

function checkoutUrlMatchesRail(value, rail) {
  const url = checkoutUrlFromEvidence(value);
  if (!url) return false;
  let parsed;
  try {
    parsed = new URL(url);
  } catch {
    return false;
  }
  if (parsed.protocol !== 'https:') return false;
  const hostname = parsed.hostname.toLowerCase();
  if (rail === 'stripe') return hostname === 'checkout.stripe.com';
  if (rail === 'coinbase') return hostname === 'commerce.coinbase.com';
  return false;
}

function validatePaymentRailEvidence(add, value) {
  validateEvidenceArray(add, value, 'payment_rails.evidence');
  if (!Array.isArray(value)) return;
  if (value.some((item) => isPlaceholder(item))) return;
  for (const rail of ['tnk', 'stripe', 'coinbase']) {
    if (!value.some((item) => hasEvidenceTag(item, `rail:${rail}`) && hasEvidenceTag(item, 'credits_mu_usd'))) {
      add('error', `payment_rails.evidence must include mu_usd credit evidence for ${rail}`);
    }
  }
}

function validateCheckoutHandoffSamples(add, value, railsVerified) {
  validateEvidenceArray(add, value, 'browser_handoffs.samples');
  if (!Array.isArray(value)) return;
  if (value.some((sample) => isPlaceholder(sample))) return;
  if (!value.some((sample) => /#copy_paste\.checkout_url:https?:\/\//i.test(sample))) {
    add('error', 'browser_handoffs.samples must include copy_paste.checkout_url evidence');
  }
  if (!Array.isArray(railsVerified)) return;
  for (const rail of ['stripe', 'coinbase']) {
    if (!railsVerified.includes(rail)) continue;
    if (!value.some((sample) => hasEvidenceTag(sample, `rail:${rail}`) && checkoutUrlMatchesRail(sample, rail))) {
      add('error', `browser_handoffs.samples must include hosted ${rail} checkout URL evidence`);
    }
  }
}

function validateRequiredRails(add, value) {
  if (!requireArray(add, value, 'browser_handoffs.rails_verified', 2)) return;
  for (const [index, rail] of value.entries()) {
    requireString(add, rail, `browser_handoffs.rails_verified[${index}]`);
  }
  if (value.some((rail) => isPlaceholder(rail))) return;
  for (const rail of ['stripe', 'coinbase']) {
    if (!value.includes(rail)) add('error', `browser_handoffs.rails_verified must include ${rail}`);
  }
}

function validateLaunchBinding(add, metrics) {
  if (!requireObject(add, metrics.launch, 'launch')) return;
  requireString(add, metrics.launch.manifest_path, 'launch.manifest_path');
  requireBoolean(add, metrics.launch.manifest_validated, true, 'launch.manifest_validated');
  requireIntegerAtLeast(add, metrics.launch.canonical_enclaves, 1, 'launch.canonical_enclaves');
  requireIntegerAtLeast(add, metrics.launch.canonical_rooms, 1, 'launch.canonical_rooms');
  requireIntegerAtLeast(add, metrics.launch.seed_providers, 1, 'launch.seed_providers');
  validateEvidenceArray(add, metrics.launch.evidence, 'launch.evidence');

  if (
    typeof metrics.launch.manifest_path !== 'string' ||
    isPlaceholder(metrics.launch.manifest_path)
  ) {
    return;
  }

  validateBoundFileEvidence(add, metrics.launch.evidence, metrics.launch.manifest_path, 'launch.manifest_path');
  const resolved = path.resolve(repoRoot, metrics.launch.manifest_path);
  if (!fs.existsSync(resolved)) return;
  const manifest = readJson(resolved);
  if (!isPlaceholder(manifest.launch_id) && manifest.launch_id !== metrics.launch_id) {
    add('error', 'launch.launch_id must match the bound launch manifest');
  }
  if (!isPlaceholder(manifest.network?.name) && manifest.network?.name !== metrics.network?.name) {
    add('error', 'network.name must match the bound launch manifest');
  }
  if (!isPlaceholder(manifest.network?.denom) && manifest.network?.denom !== metrics.network?.denom) {
    add('error', 'network.denom must match the bound launch manifest');
  }
}

function validateMetrics(metrics, { metricsPath, allowPlaceholders }) {
  const { errors, warnings, add } = issueFactory({ allowPlaceholders });

  requireLiteral(add, metrics.schema_version, 1, 'schema_version');
  requireString(add, metrics.launch_id, 'launch_id');
  validateLaunchBinding(add, metrics);

  if (requireObject(add, metrics.network, 'network')) {
    requireLiteral(add, metrics.network.name, 'testnet1', 'network.name');
    requireLiteral(add, metrics.network.denom, 'mu_usd', 'network.denom');
    if (requireObject(add, metrics.network.msb, 'network.msb')) {
      requireLiteral(add, metrics.network.msb.address_prefix, 'testtrac', 'network.msb.address_prefix');
      requireLiteral(add, metrics.network.msb.network_id, 919, 'network.msb.network_id');
    }
  }

  if (requireObject(add, metrics.payment_rails, 'payment_rails')) {
    requireLiteral(add, metrics.payment_rails.ledger_denom, 'mu_usd', 'payment_rails.ledger_denom');
    requireBoolean(add, metrics.payment_rails.tnk_enabled, true, 'payment_rails.tnk_enabled');
    requireBoolean(add, metrics.payment_rails.stripe_enabled, true, 'payment_rails.stripe_enabled');
    requireBoolean(add, metrics.payment_rails.coinbase_enabled, true, 'payment_rails.coinbase_enabled');
    requireBoolean(add, metrics.payment_rails.rails_credit_mu_usd, true, 'payment_rails.rails_credit_mu_usd');
    validatePaymentRailEvidence(add, metrics.payment_rails.evidence);
  }

  if (requireObject(add, metrics.window, 'window')) {
    requireString(add, metrics.window.started_at, 'window.started_at', isoUtc);
    requireString(add, metrics.window.ended_at, 'window.ended_at', isoUtc);
    if (
      !isPlaceholder(metrics.window.started_at) &&
      !isPlaceholder(metrics.window.ended_at) &&
      typeof metrics.window.started_at === 'string' &&
      typeof metrics.window.ended_at === 'string' &&
      Date.parse(metrics.window.ended_at) <= Date.parse(metrics.window.started_at)
    ) {
      add('error', 'window.ended_at must be after window.started_at');
    }
  }

  if (requireObject(add, metrics.controls, 'controls')) {
    requireBoolean(add, metrics.controls.admin_controls_economy, true, 'controls.admin_controls_economy');
    requireBoolean(add, metrics.controls.providers_set_prices, false, 'controls.providers_set_prices');
    requireBoolean(add, metrics.controls.providers_set_payout_terms, false, 'controls.providers_set_payout_terms');
    requireBoolean(add, metrics.controls.providers_submit_models, false, 'controls.providers_submit_models');
    requireBoolean(add, metrics.controls.providers_create_canonical_rooms, false, 'controls.providers_create_canonical_rooms');
    requireBoolean(add, metrics.controls.providers_only_join_admin_rooms, true, 'controls.providers_only_join_admin_rooms');
    requireBoolean(
      add,
      metrics.controls.provider_payout_targets_admin_verified,
      true,
      'controls.provider_payout_targets_admin_verified',
    );
    validateEvidenceArray(add, metrics.controls.evidence, 'controls.evidence');
  }

  if (requireObject(add, metrics.canonical_service, 'canonical_service')) {
    requireBoolean(
      add,
      metrics.canonical_service.admin_created_enclaves_verified,
      true,
      'canonical_service.admin_created_enclaves_verified',
    );
    requireBoolean(
      add,
      metrics.canonical_service.admin_created_rooms_verified,
      true,
      'canonical_service.admin_created_rooms_verified',
    );
    requireBoolean(
      add,
      metrics.canonical_service.provider_join_records_verified,
      true,
      'canonical_service.provider_join_records_verified',
    );
    requireBoolean(
      add,
      metrics.canonical_service.admin_price_records_verified,
      true,
      'canonical_service.admin_price_records_verified',
    );
    requireBoolean(
      add,
      metrics.canonical_service.admin_payout_records_verified,
      true,
      'canonical_service.admin_payout_records_verified',
    );
    validateEvidenceArray(add, metrics.canonical_service.evidence, 'canonical_service.evidence');
  }

  const counts = {
    external_providers: 0,
    users: 0,
  };
  if (requireObject(add, metrics.participants, 'participants')) {
    if (requireObject(add, metrics.participants.external_providers, 'participants.external_providers')) {
      counts.external_providers = metrics.participants.external_providers.count;
      requireIntegerAtLeast(
        add,
        metrics.participants.external_providers.count,
        thresholds.externalProviders,
        'participants.external_providers.count',
      );
      requireBoolean(
        add,
        metrics.participants.external_providers.identity_records_verified,
        true,
        'participants.external_providers.identity_records_verified',
      );
      validateEvidenceArray(add, metrics.participants.external_providers.evidence, 'participants.external_providers.evidence');
    }
    if (requireObject(add, metrics.participants.users, 'participants.users')) {
      counts.users = metrics.participants.users.count;
      requireIntegerAtLeast(add, metrics.participants.users.count, thresholds.users, 'participants.users.count');
      requireBoolean(
        add,
        metrics.participants.users.identity_records_verified,
        true,
        'participants.users.identity_records_verified',
      );
      validateEvidenceArray(add, metrics.participants.users.evidence, 'participants.users.evidence');
    }
  }

  const auditedEpoch = metrics.audited_epoch || {};
  if (requireObject(add, auditedEpoch, 'audited_epoch')) {
    requireIntegerAtLeast(add, auditedEpoch.epoch, 0, 'audited_epoch.epoch');
    requireBoolean(add, auditedEpoch.committed, true, 'audited_epoch.committed');
    requireBoolean(add, auditedEpoch.applied, true, 'audited_epoch.applied');
    requireString(add, auditedEpoch.commit_tx, 'audited_epoch.commit_tx', hex64);
    requireString(add, auditedEpoch.apply_tx, 'audited_epoch.apply_tx', hex64);
    if (requireObject(add, auditedEpoch.roots, 'audited_epoch.roots')) {
      for (const key of ['dep', 'use', 'earn', 'fee', 'pay']) {
        requireString(add, auditedEpoch.roots[key], `audited_epoch.roots.${key}`, hex64);
      }
    }
    requireBoolean(add, auditedEpoch.receipt_batches_verified, true, 'audited_epoch.receipt_batches_verified');
    requireBoolean(add, auditedEpoch.payout_evidence_verified, true, 'audited_epoch.payout_evidence_verified');
    if (requireArray(add, auditedEpoch.auditors, 'audited_epoch.auditors', 1)) {
      for (const [index, auditor] of auditedEpoch.auditors.entries()) {
        requireString(add, auditor, `audited_epoch.auditors[${index}]`, hex64);
      }
    }
    validateEvidenceArray(add, auditedEpoch.evidence, 'audited_epoch.evidence');
  }

  if (requireObject(add, metrics.guardian, 'guardian')) {
    requireLiteral(add, metrics.guardian.trips, 0, 'guardian.trips');
    requireBoolean(add, metrics.guardian.conservation_ok, true, 'guardian.conservation_ok');
    requireBoolean(add, metrics.guardian.monotonic_epochs, true, 'guardian.monotonic_epochs');
    validateEvidenceArray(add, metrics.guardian.evidence, 'guardian.evidence');
  }

  if (requireObject(add, metrics.canary, 'canary')) {
    requireLiteral(add, metrics.canary.set_id, 'canary-launch-v1', 'canary.set_id');
    requireIntegerAtLeast(add, metrics.canary.probes, 1, 'canary.probes');
    requireLiteral(add, metrics.canary.failures, 0, 'canary.failures');
    validateEvidenceArray(add, metrics.canary.evidence, 'canary.evidence');
  }

  if (requireObject(add, metrics.browser_handoffs, 'browser_handoffs')) {
    requireBoolean(add, metrics.browser_handoffs.copy_paste_urls_printed, true, 'browser_handoffs.copy_paste_urls_printed');
    validateRequiredRails(add, metrics.browser_handoffs.rails_verified);
    validateCheckoutHandoffSamples(add, metrics.browser_handoffs.samples, metrics.browser_handoffs.rails_verified);
  }

  if (metrics.tracker?.metrics_recorded !== true) {
    add('placeholder', 'tracker.metrics_recorded must be true after docs/TRACKER.md has been updated with the real beta metrics');
  }

  const allText = JSON.stringify(metrics);
  if (!allowPlaceholders && isPlaceholder(allText)) {
    add('error', 'metrics file still contains placeholders; use --allow-placeholders only for template checks');
  }

  return {
    ok: errors.length === 0,
    metrics_path: metricsPath,
    errors,
    warnings,
    thresholds,
    counts,
    launch_manifest: metrics.launch?.manifest_path ?? null,
    audited_epoch: auditedEpoch.epoch ?? null,
    guardian_trips: metrics.guardian?.trips ?? null,
    tracker_snippet: buildTrackerSnippet(metrics, metricsPath),
  };
}

function buildTrackerSnippet(metrics, metricsPath) {
  const providers = metrics.participants?.external_providers?.count ?? '<providers>';
  const users = metrics.participants?.users?.count ?? '<users>';
  const epoch = metrics.audited_epoch?.epoch ?? '<epoch>';
  const guardianTrips = metrics.guardian?.trips ?? '<guardian-trips>';
  const evidenceFile = metricsPath ? path.basename(metricsPath) : path.basename(defaultMetrics).replace('.template', '');
  return `P8.5 beta exit metrics: ${providers} external providers, ${users} users, audited epoch ${epoch}, guardian trips ${guardianTrips}. Evidence file: ${evidenceFile}.`;
}

function printHuman(report, args) {
  console.log(`Mayhem beta metrics: ${report.ok ? 'ok' : 'not ready'}`);
  console.log(`Copy/paste metrics path: ${path.resolve(repoRoot, args.metrics)}`);
  console.log(`External providers: ${report.counts.external_providers}`);
  console.log(`Users: ${report.counts.users}`);
  console.log(`Audited epoch: ${report.audited_epoch}`);
  console.log(`Guardian trips: ${report.guardian_trips}`);

  for (const warning of report.warnings) console.log(`warning: ${warning}`);
  for (const error of report.errors) console.error(`error: ${error}`);

  if (report.ok) {
    console.log('');
    console.log('Copy/paste tracker metrics note:');
    console.log(report.tracker_snippet);
  }
}

function main() {
  try {
    const args = parseArgs(process.argv.slice(2));
    const metricsPath = path.resolve(repoRoot, args.metrics);
    const metrics = readJson(metricsPath);
    const report = validateMetrics(metrics, {
      metricsPath: path.relative(repoRoot, metricsPath),
      allowPlaceholders: args.allowPlaceholders,
    });
    if (args.json) console.log(JSON.stringify(report, null, 2));
    else printHuman(report, args);
    if (!report.ok) process.exitCode = 1;
  } catch (error) {
    console.error(`error: ${error.message}`);
    process.exitCode = 1;
  }
}

main();

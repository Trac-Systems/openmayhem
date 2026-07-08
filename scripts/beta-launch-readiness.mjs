#!/usr/bin/env node
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const defaultManifest = 'config/beta/testnet.json';
const fileEvidencePattern = /^file:([^#]+)#sha256:([0-9a-fA-F]{64})(?:$|[#?&])/;

function usage() {
  console.log(`Usage: node scripts/beta-launch-readiness.mjs --manifest PATH [options]

Checks whether a P8.4 beta manifest is backed by real launch evidence instead
of template or smoke-only proofs. This is read-only: it validates files, calls
the strict launch validator, and reports the configured paygate health URL as
advisory evidence. Payment acceptance is the CLI/browser handoff proof; final
checkout follow-through can be verified manually by the operator.

Options:
  --manifest PATH                 Launch manifest (default: ${defaultManifest})
  --skip-paygate-health           Do not fetch the advisory paygate health URL
  --require-bundle-hash-fetch     Require collector evidence from --verify-bundle-hash
  --http-timeout SECONDS          Paygate health timeout (default: 20)
  --json                          Print JSON report`);
}

function parseArgs(argv) {
  const args = {
    manifest: defaultManifest,
    skipPaygateHealth: false,
    requireBundleHashFetch: false,
    httpTimeout: 20,
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
    else if (arg === '--skip-paygate-health') args.skipPaygateHealth = true;
    else if (arg === '--require-bundle-hash-fetch') args.requireBundleHashFetch = true;
    else if (arg === '--http-timeout') args.httpTimeout = Number.parseInt(next(), 10);
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

function sha256File(filePath) {
  return crypto.createHash('sha256').update(fs.readFileSync(filePath)).digest('hex');
}

function hasPlaceholder(value) {
  return typeof value === 'string' && (
    value.includes('<') ||
    value.includes('>') ||
    /\b(TBD|TODO|REPLACE|PLACEHOLDER|CHANGE_ME)\b/i.test(value)
  );
}

function containsPlaceholder(value) {
  return hasPlaceholder(JSON.stringify(value));
}

function check(id, ok, message, extra = {}) {
  return {
    id,
    status: ok ? 'ok' : 'fail',
    message,
    ...extra,
  };
}

function missing(id, message, extra = {}) {
  return {
    id,
    status: 'missing',
    message,
    ...extra,
  };
}

function advisory(id, message, extra = {}) {
  return {
    id,
    status: 'advisory',
    message,
    blocking: false,
    ...extra,
  };
}

function evidenceRefs(manifest, key) {
  const value = manifest?.evidence?.[key];
  return Array.isArray(value) ? value : [];
}

function readFileEvidenceRecords(manifest, key) {
  const refs = evidenceRefs(manifest, key);
  const records = [];
  const errors = [];
  for (const ref of refs) {
    const match = typeof ref === 'string' ? fileEvidencePattern.exec(ref) : null;
    if (!match) {
      errors.push(`${key} evidence is not file-bound: ${String(ref)}`);
      continue;
    }
    const evidencePath = match[1];
    if (path.isAbsolute(evidencePath) || evidencePath.split(/[\\/]/).includes('..')) {
      errors.push(`${key} evidence path must be repo-relative: ${evidencePath}`);
      continue;
    }
    const resolved = resolveRepo(evidencePath);
    if (!fs.existsSync(resolved)) {
      errors.push(`${key} evidence file does not exist: ${evidencePath}`);
      continue;
    }
    const actual = sha256File(resolved);
    const expected = match[2].toLowerCase();
    if (actual !== expected) {
      errors.push(`${key} evidence hash mismatch for ${evidencePath}: ${actual} !== ${expected}`);
      continue;
    }
    try {
      records.push({
        ref,
        path: relativeFile(resolved),
        value: JSON.parse(fs.readFileSync(resolved, 'utf8')),
      });
    } catch (error) {
      errors.push(`${key} evidence is not JSON: ${evidencePath}: ${error.message}`);
    }
  }
  return { records, errors };
}

function runStrictLaunchValidator(manifestPath) {
  const child = spawnSync(process.execPath, [
    'scripts/beta-launch.mjs',
    '--manifest',
    relativeFile(manifestPath),
    '--json',
    '--no-commands',
  ], {
    cwd: repoRoot,
    encoding: 'utf8',
  });
  if (child.status !== 0) {
    const output = `${child.stdout || ''}${child.stderr || ''}`.trim();
    return check('manifest.strict_validation', false, 'strict beta-launch validation failed', {
      output,
    });
  }
  try {
    const parsed = JSON.parse(child.stdout);
    return check('manifest.strict_validation', parsed.ok === true, 'strict beta-launch validation passed', {
      warnings: parsed.warnings || [],
      errors: parsed.errors || [],
    });
  } catch (error) {
    return check('manifest.strict_validation', false, 'strict beta-launch validation did not return JSON', {
      output: child.stdout,
      error: error.message,
    });
  }
}

function checkBootstrap(manifest) {
  const { records, errors } = readFileEvidenceRecords(manifest, 'bootstrap_nodes');
  if (errors.length > 0) {
    return check('evidence.bootstrap.real_dht_roundtrip', false, errors.join('; '));
  }
  if (records.length === 0) {
    return missing('evidence.bootstrap.real_dht_roundtrip', 'no bootstrap-node evidence file is bound');
  }
  const okRecord = records.find(({ value }) => (
    value?.ok === true &&
    value?.dht_probe?.skipped !== true &&
    value?.dht_probe?.peer?.ok === true &&
    value?.dht_probe?.msb?.ok === true
  ));
  return check(
    'evidence.bootstrap.real_dht_roundtrip',
    !!okRecord,
    okRecord
      ? `DHT bootstrap round-trip evidence is real (${okRecord.path})`
      : 'bootstrap evidence must include non-skipped peer and MSB DHT round-trip proofs',
    okRecord ? { evidence: okRecord.path } : {}
  );
}

function checkEpochWallet(manifest) {
  const { records, errors } = readFileEvidenceRecords(manifest, 'epoch_wallet_funding');
  if (errors.length > 0) {
    return check('evidence.epoch_wallet.live_msb_probe', false, errors.join('; '));
  }
  if (records.length === 0) {
    return missing('evidence.epoch_wallet.live_msb_probe', 'no epoch-wallet funding evidence file is bound');
  }
  const okRecord = records.find(({ value }) => (
    value?.funded === true &&
    value?.address === manifest.epoch_wallet?.address &&
    value?.probe?.skipped !== true &&
    value?.probe?.ok === true
  ));
  return check(
    'evidence.epoch_wallet.live_msb_probe',
    !!okRecord,
    okRecord
      ? `live MSB funding proof is bound (${okRecord.path})`
      : 'epoch-wallet evidence must come from a non-skipped public MSB balance probe',
    okRecord ? { evidence: okRecord.path } : {}
  );
}

function checkSeedProviders(manifest) {
  const { records, errors } = readFileEvidenceRecords(manifest, 'seed_provider_opt_ins');
  if (errors.length > 0) {
    return check('evidence.seed_providers.real_lifecycle_reports', false, errors.join('; '));
  }
  if (records.length === 0) {
    return missing('evidence.seed_providers.real_lifecycle_reports', 'no seed-provider opt-in proof is bound');
  }
  const okRecord = records.find(({ value }) => (
    value?.free_feature_lifecycle_records === true &&
    value?.derived_from_manifest_for_smoke_only !== true &&
    Array.isArray(value?.provider_reports) &&
    value.provider_reports.length > 0 &&
    Array.isArray(value?.verified_features) &&
    value.verified_features.length > 0
  ));
  return check(
    'evidence.seed_providers.real_lifecycle_reports',
    !!okRecord,
    okRecord
      ? `provider lifecycle feature reports are bound (${okRecord.path})`
      : 'seed-provider evidence must be built from real provider JSON reports, not manifest-derived smoke data',
    okRecord ? { evidence: okRecord.path } : {}
  );
}

function checkDownloads(manifest, { requireBundleHashFetch }) {
  const { records, errors } = readFileEvidenceRecords(manifest, 'enclave_downloads');
  if (errors.length > 0) {
    return check('evidence.enclave_downloads.public_http', false, errors.join('; '));
  }
  if (records.length === 0) {
    return missing('evidence.enclave_downloads.public_http', 'no enclave-download evidence file is bound');
  }
  const expected = new Set((manifest.canonical_enclaves || []).map((enclave) => enclave.enclave_id));
  const okRecord = records.find(({ value }) => {
    const distributions = Array.isArray(value?.distributions) ? value.distributions : [];
    if (distributions.length < expected.size) return false;
    for (const enclaveId of expected) {
      const distribution = distributions.find((item) => item.enclave_id === enclaveId);
      if (!distribution) return false;
      if (distribution.admin_signed !== true) return false;
      if (distribution.http?.bundle?.ok !== true || distribution.http?.manifest?.ok !== true) return false;
      if (requireBundleHashFetch && distribution.bundle_fetch?.ok !== true) return false;
    }
    return true;
  });
  return check(
    'evidence.enclave_downloads.public_http',
    !!okRecord,
    okRecord
      ? `public enclave download HTTP proof is bound (${okRecord.path})`
      : 'enclave-download evidence must include public HEAD proofs for every canonical bundle and sidecar manifest',
    okRecord ? { evidence: okRecord.path } : {}
  );
}

function checkCanary(manifest) {
  const { records, errors } = readFileEvidenceRecords(manifest, 'canary_set');
  if (errors.length > 0) {
    return check('evidence.canary.hash_bound', false, errors.join('; '));
  }
  const canaryPath = manifest.canary?.path ? resolveRepo(manifest.canary.path) : null;
  const okRecord = canaryPath && records.find((record) => resolveRepo(record.path) === canaryPath);
  return check(
    'evidence.canary.hash_bound',
    !!okRecord,
    okRecord ? 'canary launch set is hash-bound' : 'canary evidence must bind catalog/canaries/canary-launch-v1.json'
  );
}

function expectedPaygateControls(manifest) {
  return {
    ok: true,
    denom: 'au_usd',
    stripe_enabled: manifest.paygate?.stripe_enabled,
    controls: manifest.controls || {},
  };
}

async function fetchJson(url, timeoutSec) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), timeoutSec * 1000);
  try {
    const response = await fetch(url, {
      method: 'GET',
      redirect: 'follow',
      signal: controller.signal,
    });
    const text = await response.text();
    let json = null;
    try {
      json = JSON.parse(text);
    } catch {
      // Keep the raw status. The caller will fail the JSON-specific checks.
    }
    return {
      ok: response.ok,
      status: response.status,
      content_type: response.headers.get('content-type'),
      json,
    };
  } finally {
    clearTimeout(timeout);
  }
}

async function checkPaygate(manifest, args) {
  if (args.skipPaygateHealth) {
    return advisory('paygate.public_health', 'paygate health fetch skipped; Stripe checkout handoff evidence is the beta gate', {
      skipped: true,
    });
  }
  if (!manifest.paygate?.public_base_url || !manifest.paygate?.health_path) {
    return advisory('paygate.public_health', 'paygate.public_base_url and paygate.health_path are not set; use Stripe checkout handoff evidence instead');
  }
  let url = null;
  try {
    url = new URL(manifest.paygate.health_path, manifest.paygate.public_base_url);
  } catch (error) {
    return advisory('paygate.public_health', `invalid paygate health URL: ${error.message}`);
  }
  try {
    const response = await fetchJson(url, args.httpTimeout);
    const expected = expectedPaygateControls(manifest);
    const controls = response.json?.controls || {};
    const rails = response.json?.rails || {};
    const ok = (
      response.ok === true &&
      response.json?.ok === expected.ok &&
      response.json?.denom === expected.denom &&
      rails.stripe?.enabled === expected.stripe_enabled &&
      controls.admin_controls_economy === expected.controls.admin_controls_economy &&
      controls.admin_sets_prices === expected.controls.admin_sets_prices &&
      controls.admin_sets_rules === expected.controls.admin_sets_rules &&
      controls.admin_sets_params === expected.controls.admin_sets_params &&
      controls.admin_sets_provider_payout_targets === expected.controls.admin_sets_provider_payout_targets &&
      controls.admin_can_ban_providers === expected.controls.admin_can_ban_providers &&
      controls.providers_set_prices === expected.controls.providers_set_prices &&
      controls.providers_set_rules === expected.controls.providers_set_rules &&
      controls.providers_set_params === expected.controls.providers_set_params &&
      controls.providers_set_payout_terms === expected.controls.providers_set_payout_terms &&
      controls.providers_submit_models === expected.controls.providers_submit_models &&
      controls.providers_create_canonical_rooms === expected.controls.providers_create_canonical_rooms &&
      controls.providers_only_join_admin_rooms === expected.controls.providers_only_join_admin_rooms &&
      controls.provider_payout_targets_admin_verified === expected.controls.provider_payout_targets_admin_verified
    );
    return ok
      ? check(
        'paygate.public_health',
        true,
        `public paygate health matches active processors and admin-control flags (${url.href})`,
        {
          url: url.href,
          status_code: response.status,
        }
      )
      : advisory(
        'paygate.public_health',
        'public paygate health did not match au_usd processor/admin-control flags; Stripe checkout handoff evidence remains sufficient for beta',
        {
          url: url.href,
          status_code: response.status,
        }
      );
  } catch (error) {
    return advisory('paygate.public_health', `public paygate health fetch failed: ${error.message}; Stripe checkout handoff evidence remains sufficient for beta`, {
      url: url.href,
    });
  }
}

async function readiness(args) {
  const manifestPath = resolveRepo(args.manifest);
  const report = {
    ok: false,
    manifest: relativeFile(manifestPath),
    checks: [],
    copy_paste: {
      create_manifest: `cp config/beta/testnet.template.json ${relativeFile(manifestPath)}`,
      collect_evidence: `node scripts/beta-launch-evidence-collect.mjs --manifest ${relativeFile(manifestPath)} --seed-provider-opt-ins <provider-opt-ins.json> --write-manifest .mayhem-local/p8.4-launch-evidence/testnet.bound.json`,
      validate: `node scripts/beta-launch.mjs --manifest ${relativeFile(manifestPath)}`,
      readiness: `node scripts/beta-launch-readiness.mjs --manifest ${relativeFile(manifestPath)}`,
    },
  };

  if (!fs.existsSync(manifestPath)) {
    report.checks.push(missing('manifest.exists', `manifest not found: ${relativeFile(manifestPath)}`));
    return report;
  }

  let manifest = null;
  try {
    manifest = readJson(manifestPath);
  } catch (error) {
    report.checks.push(check('manifest.json', false, `manifest is not valid JSON: ${error.message}`));
    return report;
  }

  const hasPlaceholders = containsPlaceholder(manifest);
  report.checks.push(check(
    'manifest.no_placeholders',
    !hasPlaceholders,
    hasPlaceholders ? 'manifest still contains placeholders' : 'manifest contains no placeholders'
  ));
  report.checks.push(runStrictLaunchValidator(manifestPath));
  report.checks.push(checkBootstrap(manifest));
  report.checks.push(checkEpochWallet(manifest));
  report.checks.push(checkSeedProviders(manifest));
  report.checks.push(checkDownloads(manifest, args));
  report.checks.push(checkCanary(manifest));
  report.checks.push(await checkPaygate(manifest, args));
  report.ok = report.checks.every((item) => item.status === 'ok' || item.status === 'advisory');
  return report;
}

async function main() {
  try {
    const args = parseArgs(process.argv.slice(2));
    const report = await readiness(args);
    if (args.json) {
      console.log(JSON.stringify(report, null, 2));
    } else {
      console.log(`Mayhem P8.4 real launch readiness: ${report.ok ? 'ready' : 'not ready'}`);
      for (const item of report.checks) {
        console.log(`[${item.status}] ${item.id}: ${item.message}`);
      }
      console.log(`Copy/paste evidence command: ${report.copy_paste.collect_evidence}`);
      console.log(`Copy/paste readiness command: ${report.copy_paste.readiness}`);
    }
    if (!report.ok) process.exitCode = 1;
  } catch (error) {
    console.error(`error: ${error.message}`);
    process.exitCode = 1;
  }
}

await main();

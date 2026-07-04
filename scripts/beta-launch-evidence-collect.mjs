#!/usr/bin/env node
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const defaultManifest = 'config/beta/testnet.json';
const defaultOutDir = '.mayhem-local/p8.4-launch-evidence';

function usage() {
  console.log(`Usage: node scripts/beta-launch-evidence-collect.mjs --manifest PATH [options]

Collects file-bound P8.4 launch evidence for a filled beta manifest, writes
JSON proofs under an ignored local directory, optionally writes a manifest copy
with evidence paths bound, and validates it with scripts/beta-launch.mjs.

Options:
  --out-dir PATH                         Evidence output dir (default: ${defaultOutDir})
  --write-manifest PATH                  Write updated manifest with evidence refs
  --seed-provider-opt-ins PATH           JSON proof from real provider lifecycle feature records
  --derive-seed-opt-ins-for-smoke        Derive seed opt-in proof from manifest for local rehearsal only
  --skip-msb-probe                       Do not run public MSB balance probe
  --epoch-wallet-balance-tnk VALUE       Balance to use with --skip-msb-probe
  --skip-download-head                   Do not HEAD public distribution URLs
  --verify-bundle-hash                   Stream bundle URLs and verify SHA-256
  --msb-timeout SECONDS                  MSB balance probe timeout (default: 90)
  --http-timeout SECONDS                 URL probe timeout (default: 20)
  --no-validate                          Do not run strict beta-launch validation
  --json                                 Print JSON report`);
}

function parseArgs(argv) {
  const args = {
    manifest: defaultManifest,
    outDir: defaultOutDir,
    writeManifest: null,
    seedProviderOptIns: null,
    deriveSeedOptInsForSmoke: false,
    skipMsbProbe: false,
    epochWalletBalanceTnk: null,
    skipDownloadHead: false,
    verifyBundleHash: false,
    validate: true,
    msbTimeout: 90,
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
    else if (arg === '--out-dir') args.outDir = next();
    else if (arg === '--write-manifest') args.writeManifest = next();
    else if (arg === '--seed-provider-opt-ins') args.seedProviderOptIns = next();
    else if (arg === '--derive-seed-opt-ins-for-smoke') args.deriveSeedOptInsForSmoke = true;
    else if (arg === '--skip-msb-probe') args.skipMsbProbe = true;
    else if (arg === '--epoch-wallet-balance-tnk') args.epochWalletBalanceTnk = next();
    else if (arg === '--skip-download-head') args.skipDownloadHead = true;
    else if (arg === '--verify-bundle-hash') args.verifyBundleHash = true;
    else if (arg === '--msb-timeout') args.msbTimeout = Number.parseInt(next(), 10);
    else if (arg === '--http-timeout') args.httpTimeout = Number.parseInt(next(), 10);
    else if (arg === '--no-validate') args.validate = false;
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

function sha256File(filePath) {
  return crypto.createHash('sha256').update(fs.readFileSync(filePath)).digest('hex');
}

function fileEvidence(filePath) {
  return `file:${relativeFile(filePath)}#sha256:${sha256File(filePath)}`;
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
    throw new Error(`${name} still contains placeholders; fill the manifest before collecting real evidence`);
  }
}

function decimalTnkToE18(value) {
  const raw = String(value ?? '').trim();
  if (!/^[0-9]+(?:\.[0-9]+)?$/.test(raw)) {
    throw new Error(`invalid TNK balance: ${value}`);
  }
  const [whole, frac = ''] = raw.split('.');
  if (frac.length > 18) throw new Error(`TNK balance has more than 18 decimal places: ${value}`);
  return (BigInt(whole) * 10n ** 18n + BigInt(frac.padEnd(18, '0'))).toString();
}

function msbRecord(manifest) {
  return {
    address_prefix: manifest.network.msb.address_prefix,
    network_id: manifest.network.msb.network_id,
    bootstrap: manifest.network.msb.bootstrap,
    channel: manifest.network.msb.channel,
  };
}

function runJson(command, args) {
  const child = spawnSync(command, args, {
    cwd: repoRoot,
    encoding: 'utf8',
  });
  if (child.status !== 0) {
    const details = `${child.stdout || ''}${child.stderr || ''}`.trim();
    throw new Error(`${[command, ...args].join(' ')} failed${details ? `:\n${details}` : ''}`);
  }
  return JSON.parse(child.stdout);
}

function collectBootstrapEvidence(manifest) {
  return {
    ok: true,
    network: manifest.network.name,
    msb: msbRecord(manifest),
    peer_dht_bootstrap: manifest.network.dht.peer_bootstrap,
    msb_dht_bootstrap: manifest.network.dht.msb_bootstrap,
  };
}

function collectEpochWalletEvidence(manifest, args) {
  let probe = null;
  let balanceTnk = null;
  if (args.skipMsbProbe) {
    if (!args.epochWalletBalanceTnk) {
      throw new Error('--skip-msb-probe requires --epoch-wallet-balance-tnk');
    }
    balanceTnk = String(args.epochWalletBalanceTnk);
    probe = { ok: true, skipped: true, balance: balanceTnk };
  } else {
    probe = runJson(process.execPath, [
      'intercom/scripts/msb-balance-probe.mjs',
      '--network',
      manifest.network.name,
      '--address',
      manifest.epoch_wallet.address,
      '--msb-bootstrap',
      manifest.network.msb.bootstrap,
      '--msb-channel',
      manifest.network.msb.channel,
      '--timeout',
      String(args.msbTimeout),
      '--json',
    ]);
    if (probe.ok !== true) {
      throw new Error(`epoch wallet ${manifest.epoch_wallet.address} is not visible on public MSB`);
    }
    balanceTnk = probe.balance;
  }
  const balanceE18 = decimalTnkToE18(balanceTnk);
  return {
    network: manifest.network.name,
    msb: msbRecord(manifest),
    address: manifest.epoch_wallet.address,
    funded: BigInt(balanceE18) >= BigInt(manifest.epoch_wallet.min_balance_tnk_e18),
    balance_tnk: balanceTnk,
    balance_tnk_e18: balanceE18,
    min_balance_tnk_e18: manifest.epoch_wallet.min_balance_tnk_e18,
    probe,
  };
}

function derivedSeedOptIns(manifest) {
  return {
    free_feature_lifecycle_records: true,
    derived_from_manifest_for_smoke_only: true,
    opt_ins: manifest.seed_providers.flatMap((provider) => (
      provider.joins || []
    ).map((join) => ({
      provider_pubkey: provider.provider_pubkey,
      enclave_id: join.enclave_id,
      rooms: join.rooms || [],
    }))),
  };
}

function collectSeedProviderEvidence(manifest, args) {
  if (args.seedProviderOptIns) {
    const proof = readJson(args.seedProviderOptIns);
    assertNoPlaceholders(proof, '--seed-provider-opt-ins');
    return proof;
  }
  if (args.deriveSeedOptInsForSmoke) return derivedSeedOptIns(manifest);
  throw new Error('seed provider opt-in evidence is required; pass --seed-provider-opt-ins PATH after providers submit free lifecycle feature records');
}

async function fetchHead(url, timeoutSec) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), timeoutSec * 1000);
  try {
    const response = await fetch(url, {
      method: 'HEAD',
      redirect: 'follow',
      signal: controller.signal,
    });
    return {
      ok: response.ok,
      status: response.status,
      content_length: response.headers.get('content-length'),
      content_type: response.headers.get('content-type'),
      etag: response.headers.get('etag'),
      last_modified: response.headers.get('last-modified'),
    };
  } finally {
    clearTimeout(timeout);
  }
}

async function hashRemote(url, timeoutSec) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), timeoutSec * 1000);
  try {
    const response = await fetch(url, {
      method: 'GET',
      redirect: 'follow',
      signal: controller.signal,
    });
    if (!response.ok || !response.body) {
      throw new Error(`GET ${url} returned HTTP ${response.status}`);
    }
    const hash = crypto.createHash('sha256');
    let bytes = 0;
    for await (const chunk of response.body) {
      const buffer = Buffer.from(chunk);
      hash.update(buffer);
      bytes += buffer.length;
    }
    return {
      ok: true,
      bytes,
      sha256: hash.digest('hex'),
    };
  } finally {
    clearTimeout(timeout);
  }
}

async function collectDownloadEvidence(manifest, args) {
  const distributions = [];
  for (const enclave of manifest.canonical_enclaves || []) {
    const distribution = {
      enclave_id: enclave.enclave_id,
      admin_signed: true,
      ...enclave.distribution,
    };
    if (!args.skipDownloadHead) {
      const bundleHead = await fetchHead(enclave.distribution.bundle_url, args.httpTimeout);
      if (!bundleHead.ok) throw new Error(`${enclave.distribution.bundle_url} HEAD returned HTTP ${bundleHead.status}`);
      if (
        bundleHead.content_length &&
        Number.parseInt(bundleHead.content_length, 10) !== enclave.distribution.bundle_bytes
      ) {
        throw new Error(`${enclave.distribution.bundle_url} content-length ${bundleHead.content_length} does not match bundle_bytes ${enclave.distribution.bundle_bytes}`);
      }
      const manifestHead = await fetchHead(enclave.distribution.manifest_url, args.httpTimeout);
      if (!manifestHead.ok) throw new Error(`${enclave.distribution.manifest_url} HEAD returned HTTP ${manifestHead.status}`);
      distribution.http = {
        bundle: bundleHead,
        manifest: manifestHead,
      };
    }
    if (args.verifyBundleHash) {
      const fetched = await hashRemote(enclave.distribution.bundle_url, args.httpTimeout);
      if (fetched.sha256 !== enclave.distribution.bundle_sha256) {
        throw new Error(`${enclave.distribution.bundle_url} SHA-256 ${fetched.sha256} does not match ${enclave.distribution.bundle_sha256}`);
      }
      if (fetched.bytes !== enclave.distribution.bundle_bytes) {
        throw new Error(`${enclave.distribution.bundle_url} byte count ${fetched.bytes} does not match ${enclave.distribution.bundle_bytes}`);
      }
      distribution.bundle_fetch = fetched;
    }
    distributions.push(distribution);
  }
  return { distributions };
}

function evidencePath(outDir, name) {
  return path.join(outDir, 'launch-evidence', name);
}

function updateManifestEvidence(manifest, evidenceFiles) {
  return {
    ...manifest,
    evidence: {
      ...manifest.evidence,
      bootstrap_nodes: [fileEvidence(evidenceFiles.bootstrap)],
      epoch_wallet_funding: [fileEvidence(evidenceFiles.epochWallet)],
      seed_provider_opt_ins: [fileEvidence(evidenceFiles.seedProviders)],
      enclave_downloads: [fileEvidence(evidenceFiles.downloads)],
    },
  };
}

function validateBoundManifest(manifestPath) {
  const child = spawnSync(process.execPath, [
    'scripts/beta-launch.mjs',
    '--manifest',
    relativeFile(resolveRepo(manifestPath)),
    '--json',
    '--no-commands',
  ], {
    cwd: repoRoot,
    encoding: 'utf8',
  });
  if (child.status !== 0) {
    const details = `${child.stdout || ''}${child.stderr || ''}`.trim();
    throw new Error(`strict launch validation failed${details ? `:\n${details}` : ''}`);
  }
  return JSON.parse(child.stdout);
}

async function collect(args) {
  const manifestPath = resolveRepo(args.manifest);
  const manifest = readJson(manifestPath);
  assertNoPlaceholders(manifest, '--manifest');
  const outDir = resolveRepo(args.outDir);

  const bootstrap = writeJson(evidencePath(outDir, 'bootstrap-health.json'), collectBootstrapEvidence(manifest));
  const epochWallet = writeJson(evidencePath(outDir, 'epoch-wallet-funding.json'), collectEpochWalletEvidence(manifest, args));
  const seedProviders = writeJson(evidencePath(outDir, 'seed-provider-opt-ins.json'), collectSeedProviderEvidence(manifest, args));
  const downloads = writeJson(evidencePath(outDir, 'enclave-downloads.json'), await collectDownloadEvidence(manifest, args));

  const evidenceFiles = { bootstrap, epochWallet, seedProviders, downloads };
  const boundManifest = updateManifestEvidence(manifest, evidenceFiles);
  const writeManifest = args.writeManifest
    ? resolveRepo(args.writeManifest)
    : path.join(outDir, 'testnet.bound.json');
  writeJson(writeManifest, boundManifest);
  const validation = args.validate ? validateBoundManifest(writeManifest) : null;

  return {
    ok: validation ? validation.ok === true : true,
    manifest: relativeFile(manifestPath),
    bound_manifest: relativeFile(writeManifest),
    out_dir: relativeFile(outDir),
    evidence: Object.fromEntries(Object.entries(evidenceFiles).map(([key, value]) => [key, relativeFile(value)])),
    validation,
    copy_paste_validate: `node scripts/beta-launch.mjs --manifest ${relativeFile(writeManifest)}`,
  };
}

async function main() {
  try {
    const args = parseArgs(process.argv.slice(2));
    const report = await collect(args);
    if (args.json) {
      console.log(JSON.stringify(report, null, 2));
    } else {
      console.log('Mayhem P8.4 launch evidence collection: ok');
      console.log(`Copy/paste bound manifest path: ${report.bound_manifest}`);
      console.log(`Copy/paste validate command: ${report.copy_paste_validate}`);
    }
    if (!report.ok) process.exitCode = 1;
  } catch (error) {
    console.error(`error: ${error.message}`);
    process.exitCode = 1;
  }
}

await main();

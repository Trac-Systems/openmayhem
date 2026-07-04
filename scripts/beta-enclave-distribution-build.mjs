#!/usr/bin/env node
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { blake3 } from '../intercom/node_modules/@tracsystems/blake3/dist/wasm/blake3.mjs';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const defaultOutDir = '.mayhem-local/enclave-distributions';
const defaultChunkSize = 8 * 1024 * 1024;
const hex64 = /^[0-9a-fA-F]{64}$/;
const hex40 = /^[0-9a-fA-F]{40}$/;
const ed25519Pkcs8SeedPrefix = Buffer.from('302e020100300506032b657004220420', 'hex');
const ed25519SpkiPrefix = Buffer.from('302a300506032b6570032100', 'hex');

function usage() {
  console.log(`Usage: node scripts/beta-enclave-distribution-build.mjs --artifact PATH --model-id ID --backend NAME --base-url HTTPS_URL --admin-seed-file PATH [options]

Builds the admin-signed downloadable enclave distribution for P8.4. The output
bundle contains the raw admin-approved artifact and a stable enclave manifest;
the public sidecar manifest records the bundle SHA-256/size and the admin
Ed25519 distribution signature. Admin seed input is never written to output.

Required:
  --artifact PATH                         Admin-approved artifact to publish
  --model-id ID                           Canonical catalog model id
  --backend NAME                          Backend bound to the enclave
  --artifact-repo REPO                    Hugging Face artifact repo namespace/name
  --artifact-revision HEX                 Hugging Face artifact git commit revision
  --artifact-path PATH                    Hugging Face artifact path inside the repo
  --base-url HTTPS_URL                    Public HTTPS directory URL for upload
  --admin-seed-file PATH                  File with 32-byte admin Ed25519 seed hex

Options:
  --admin-seed-hex HEX                    Alternative seed source; prefer file/env
  --admin-pubkey HEX                      Require derived admin pubkey to match
  --artifact-root HEX                     Expected artifact Merkle root
  --binary PATH                           Release mayhem-enclave binary to measure
  --binary-hash HEX                       Precomputed binary BLAKE3 hash
  --caps-json JSON                        Canonical caps to copy into patch output
  --out-dir PATH                          Output dir (default: ${defaultOutDir})
  --chunk-size BYTES                      Artifact Merkle chunk size (default: ${defaultChunkSize})
  --format tar.zst|tar.gz                 Bundle archive format (default: tar.zst)
  --bundle-url HTTPS_URL                  Override public bundle URL
  --manifest-url HTTPS_URL                Override public sidecar manifest URL
  --mirror HTTPS_URL                      Add a public bundle mirror (repeatable)
  --allow-artifact-root-mismatch-for-smoke
                                           Local rehearsal only; never launch evidence
  --json                                  Print JSON report`);
}

function parseArgs(argv) {
  const args = {
    artifact: null,
    modelId: null,
    backend: null,
    artifactRepo: null,
    artifactRevision: null,
    artifactPathInRepo: null,
    baseUrl: null,
    adminSeedFile: null,
    adminSeedHex: process.env.MAYHEM_ADMIN_SEED_HEX || null,
    adminPubkey: null,
    artifactRoot: null,
    binary: null,
    binaryHash: null,
    capsJson: null,
    outDir: defaultOutDir,
    chunkSize: defaultChunkSize,
    format: 'tar.zst',
    bundleUrl: null,
    manifestUrl: null,
    mirrors: [],
    allowArtifactRootMismatchForSmoke: false,
    json: false,
  };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    const next = () => {
      i += 1;
      if (!argv[i]) throw new Error(`${arg} requires a value`);
      return argv[i];
    };
    if (arg === '--artifact') args.artifact = next();
    else if (arg === '--model-id') args.modelId = next();
    else if (arg === '--backend') args.backend = next();
    else if (arg === '--artifact-repo') args.artifactRepo = next();
    else if (arg === '--artifact-revision') args.artifactRevision = next();
    else if (arg === '--artifact-path') args.artifactPathInRepo = next();
    else if (arg === '--base-url') args.baseUrl = next();
    else if (arg === '--admin-seed-file') args.adminSeedFile = next();
    else if (arg === '--admin-seed-hex') args.adminSeedHex = next();
    else if (arg === '--admin-pubkey') args.adminPubkey = next();
    else if (arg === '--artifact-root') args.artifactRoot = next();
    else if (arg === '--binary') args.binary = next();
    else if (arg === '--binary-hash') args.binaryHash = next();
    else if (arg === '--caps-json') args.capsJson = next();
    else if (arg === '--out-dir') args.outDir = next();
    else if (arg === '--chunk-size') args.chunkSize = Number.parseInt(next(), 10);
    else if (arg === '--format') args.format = next();
    else if (arg === '--bundle-url') args.bundleUrl = next();
    else if (arg === '--manifest-url') args.manifestUrl = next();
    else if (arg === '--mirror') args.mirrors.push(next());
    else if (arg === '--allow-artifact-root-mismatch-for-smoke') args.allowArtifactRootMismatchForSmoke = true;
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

function assertRequired(value, name) {
  if (!value) throw new Error(`${name} is required`);
}

function assertHex64(value, name) {
  if (typeof value !== 'string' || !hex64.test(value)) {
    throw new Error(`${name} must be 64 hex characters`);
  }
}

function isSafeHuggingFaceComponent(value) {
  return typeof value === 'string' &&
    /^[A-Za-z0-9][A-Za-z0-9._-]{0,95}$/.test(value) &&
    !value.endsWith('.') &&
    !value.endsWith('-') &&
    !value.includes('..') &&
    !value.includes('--');
}

function isSafeHuggingFaceRepo(value) {
  if (typeof value !== 'string') return false;
  const parts = value.split('/');
  return parts.length === 2 && parts.every(isSafeHuggingFaceComponent);
}

function isSafeHuggingFacePath(value) {
  return typeof value === 'string' &&
    value.length > 0 &&
    !value.startsWith('/') &&
    !value.startsWith('\\') &&
    !value.includes('\\') &&
    !value.includes('?') &&
    !value.includes('#') &&
    !value.includes('%') &&
    !/[\x00-\x1f\x7f]/.test(value) &&
    value.split('/').every((part) => (
      part.length > 0 &&
      part !== '.' &&
      part !== '..' &&
      /^[A-Za-z0-9._+-]+$/.test(part)
    ));
}

function validateArtifactSource(args) {
  assertRequired(args.artifactRepo, '--artifact-repo');
  assertRequired(args.artifactRevision, '--artifact-revision');
  assertRequired(args.artifactPathInRepo, '--artifact-path');
  if (!isSafeHuggingFaceRepo(args.artifactRepo)) {
    throw new Error('--artifact-repo must be a safe Hugging Face namespace/name repo id');
  }
  if (!hex40.test(args.artifactRevision)) {
    throw new Error('--artifact-revision must be a 40-hex git commit');
  }
  if (!isSafeHuggingFacePath(args.artifactPathInRepo)) {
    throw new Error('--artifact-path must be a safe relative Hugging Face artifact path');
  }
  return {
    kind: 'huggingface',
    repo: args.artifactRepo,
    revision: args.artifactRevision.toLowerCase(),
    path: args.artifactPathInRepo,
  };
}

function assertFile(filePath, name) {
  const stat = fs.statSync(filePath, { throwIfNoEntry: false });
  if (!stat?.isFile()) throw new Error(`${name} must point to a file: ${filePath}`);
  return stat;
}

function stableJson(value) {
  if (Array.isArray(value)) return `[${value.map(stableJson).join(',')}]`;
  if (value && typeof value === 'object') {
    return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${stableJson(value[key])}`).join(',')}}`;
  }
  return JSON.stringify(value);
}

async function blake3HexBytes(bytes) {
  const digest = await blake3(bytes);
  return Buffer.from(digest).toString('hex');
}

async function blake3HexText(text) {
  return blake3HexBytes(Buffer.from(text));
}

async function blake3File(filePath) {
  return blake3HexBytes(fs.readFileSync(filePath));
}

function sha256File(filePath) {
  return new Promise((resolve, reject) => {
    const hash = crypto.createHash('sha256');
    const stream = fs.createReadStream(filePath);
    stream.on('error', reject);
    stream.on('data', (chunk) => hash.update(chunk));
    stream.on('end', () => resolve(hash.digest('hex')));
  });
}

async function merkleLeafHash(index, len, data) {
  const prefix = Buffer.alloc(8 + 8);
  prefix.writeBigUInt64LE(BigInt(index), 0);
  prefix.writeBigUInt64LE(BigInt(len), 8);
  return Buffer.from(await blake3(Buffer.concat([
    Buffer.from('mayhem-blake3-merkle-v1:leaf'),
    prefix,
    data,
  ])));
}

async function merkleParentHash(left, right) {
  return Buffer.from(await blake3(Buffer.concat([
    Buffer.from('mayhem-blake3-merkle-v1:node'),
    left,
    right,
  ])));
}

async function merkleEmptyHash() {
  return Buffer.from(await blake3(Buffer.from('mayhem-blake3-merkle-v1:empty')));
}

async function merkleRootFromLeaves(leaves) {
  if (leaves.length === 0) return merkleEmptyHash();
  let level = leaves;
  while (level.length > 1) {
    const next = [];
    for (let i = 0; i < level.length; i += 2) {
      next.push(await merkleParentHash(level[i], level[i + 1] || level[i]));
    }
    level = next;
  }
  return level[0];
}

async function buildMerkleManifest(filePath, chunkSize) {
  if (!Number.isInteger(chunkSize) || chunkSize <= 0) throw new Error('--chunk-size must be a positive integer');
  const handle = fs.openSync(filePath, 'r');
  const buffer = Buffer.alloc(chunkSize);
  const chunks = [];
  const leaves = [];
  let totalBytes = 0;
  try {
    for (;;) {
      const read = fs.readSync(handle, buffer, 0, chunkSize, null);
      if (read === 0) break;
      const index = chunks.length;
      const leaf = await merkleLeafHash(index, read, buffer.subarray(0, read));
      leaves.push(leaf);
      chunks.push({
        index,
        offset: totalBytes,
        len: read,
        blake3: leaf.toString('hex'),
      });
      totalBytes += read;
    }
  } finally {
    fs.closeSync(handle);
  }
  return {
    kind: 'blake3_merkle_v1',
    chunk_size: chunkSize,
    total_bytes: totalBytes,
    root: (await merkleRootFromLeaves(leaves)).toString('hex'),
    chunks,
  };
}

function parseSeedFile(filePath) {
  const raw = fs.readFileSync(filePath, 'utf8').trim();
  if (raw.startsWith('{')) {
    const parsed = JSON.parse(raw);
    return parsed.admin_seed_hex || parsed.seed_hex || parsed.seed;
  }
  return raw.replace(/\s+/g, '');
}

function adminKeyFromSeed(seedHex) {
  assertHex64(seedHex, 'admin seed');
  const seed = Buffer.from(seedHex, 'hex');
  const privateKey = crypto.createPrivateKey({
    key: Buffer.concat([ed25519Pkcs8SeedPrefix, seed]),
    format: 'der',
    type: 'pkcs8',
  });
  const publicDer = crypto.createPublicKey(privateKey).export({ format: 'der', type: 'spki' });
  if (!Buffer.from(publicDer.subarray(0, ed25519SpkiPrefix.length)).equals(ed25519SpkiPrefix)) {
    throw new Error('unexpected Ed25519 public key DER prefix');
  }
  return {
    privateKey,
    publicKeyHex: Buffer.from(publicDer.subarray(ed25519SpkiPrefix.length)).toString('hex'),
  };
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

function requirePublicHttpsUrl(value, name) {
  let parsed;
  try {
    parsed = new URL(value);
  } catch {
    throw new Error(`${name} must be a valid URL`);
  }
  if (parsed.protocol !== 'https:') throw new Error(`${name} must use https`);
  if (parsed.username || parsed.password || parsed.hash) {
    throw new Error(`${name} must not include credentials or fragment`);
  }
  if (isPrivateOrReservedHostname(parsed.hostname)) {
    throw new Error(`${name} must use a public launch hostname, not ${parsed.hostname}`);
  }
  return parsed;
}

function urlJoin(baseUrl, fileName) {
  const base = requirePublicHttpsUrl(baseUrl, '--base-url');
  const pathname = base.pathname.endsWith('/') ? base.pathname : `${base.pathname}/`;
  base.pathname = `${pathname}${fileName}`;
  base.search = '';
  base.hash = '';
  return base.toString();
}

function safeArtifactName(filePath) {
  const name = path.basename(filePath).replace(/[^A-Za-z0-9._-]/g, '_');
  if (!name || name === '.' || name === '..') throw new Error('artifact filename is not usable');
  return name;
}

function run(command, args) {
  const child = spawnSync(command, args, {
    cwd: repoRoot,
    encoding: 'utf8',
  });
  if (child.status !== 0) {
    const details = `${child.stdout || ''}${child.stderr || ''}`.trim();
    throw new Error(`${[command, ...args].join(' ')} failed${details ? `:\n${details}` : ''}`);
  }
}

function assertTool(command, hint) {
  const child = spawnSync(command, ['--version'], { encoding: 'utf8' });
  if (child.status !== 0) throw new Error(`${command} is required${hint ? ` (${hint})` : ''}`);
}

function distributionSigningPayload(adminPubkey, enclave) {
  return Buffer.from(stableJson({
    schema_version: 1,
    kind: 'mayhem-enclave-distribution-v1',
    admin_pubkey: adminPubkey,
    enclave_id: enclave.enclave_id,
    model_id: enclave.model_id,
    backend: enclave.backend,
    artifact_root: enclave.artifact_root,
    artifact_root_kind: enclave.artifact_root_kind,
    artifact_source: enclave.artifact_source,
    source_sha256: enclave.source_sha256,
    manifest_hash: enclave.manifest_hash,
    binary_hash: enclave.binary_hash,
    distribution: {
      bundle_url: enclave.distribution.bundle_url,
      manifest_url: enclave.distribution.manifest_url,
      bundle_sha256: enclave.distribution.bundle_sha256,
      bundle_bytes: enclave.distribution.bundle_bytes,
      mirrors: enclave.distribution.mirrors || [],
    },
  }));
}

function signDistribution(adminKey, adminPubkey, enclave) {
  const signature = crypto.sign(null, distributionSigningPayload(adminPubkey, enclave), adminKey.privateKey);
  const publicKey = crypto.createPublicKey(adminKey.privateKey);
  if (!crypto.verify(null, distributionSigningPayload(adminPubkey, enclave), publicKey, signature)) {
    throw new Error('distribution signature self-check failed');
  }
  return signature.toString('hex');
}

function writeJson(filePath, value) {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.writeFileSync(filePath, `${JSON.stringify(value, null, 2)}\n`);
}

async function build(args) {
  assertRequired(args.artifact, '--artifact');
  assertRequired(args.modelId, '--model-id');
  assertRequired(args.backend, '--backend');
  assertRequired(args.baseUrl || (args.bundleUrl && args.manifestUrl), '--base-url or explicit --bundle-url/--manifest-url');
  if (!args.binary && !args.binaryHash) throw new Error('pass --binary PATH or --binary-hash HEX');
  if (!['tar.zst', 'tar.gz'].includes(args.format)) throw new Error('--format must be tar.zst or tar.gz');
  const artifactSource = validateArtifactSource(args);

  const artifactPath = resolveRepo(args.artifact);
  const artifactStat = assertFile(artifactPath, '--artifact');
  const binaryPath = args.binary ? resolveRepo(args.binary) : null;
  if (binaryPath) assertFile(binaryPath, '--binary');

  const seedHex = args.adminSeedFile ? parseSeedFile(resolveRepo(args.adminSeedFile)) : args.adminSeedHex;
  if (!seedHex) throw new Error('admin seed is required via --admin-seed-file, --admin-seed-hex, or MAYHEM_ADMIN_SEED_HEX');
  const adminKey = adminKeyFromSeed(seedHex);
  if (args.adminPubkey) {
    assertHex64(args.adminPubkey, '--admin-pubkey');
    if (args.adminPubkey.toLowerCase() !== adminKey.publicKeyHex) {
      throw new Error(`derived admin pubkey ${adminKey.publicKeyHex} does not match --admin-pubkey ${args.adminPubkey}`);
    }
  }

  const artifactSha256 = await sha256File(artifactPath);
  const merkle = await buildMerkleManifest(artifactPath, args.chunkSize);
  let artifactRoot = merkle.root;
  let artifactRootVerified = true;
  if (args.artifactRoot) {
    assertHex64(args.artifactRoot, '--artifact-root');
    artifactRoot = args.artifactRoot.toLowerCase();
    artifactRootVerified = merkle.root.toLowerCase() === artifactRoot;
    if (!artifactRootVerified && !args.allowArtifactRootMismatchForSmoke) {
      throw new Error(`artifact Merkle root mismatch: expected ${artifactRoot}, got ${merkle.root}`);
    }
    if (!artifactRootVerified) {
      console.error('warning: artifact root mismatch allowed for smoke only; this is not launch evidence');
    }
  }

  const binaryHash = args.binaryHash ? args.binaryHash.toLowerCase() : await blake3File(binaryPath);
  assertHex64(binaryHash, '--binary-hash');
  const caps = args.capsJson ? JSON.parse(args.capsJson) : undefined;

  const artifactName = safeArtifactName(artifactPath);
  const enclaveManifest = {
    schema_version: 1,
    kind: 'mayhem-enclave-manifest-v1',
    admin_pubkey: adminKey.publicKeyHex,
    model_id: args.modelId,
    backend: args.backend,
    artifact_root: artifactRoot,
    artifact_root_kind: 'blake3_merkle_v1',
    artifact_source: artifactSource,
    source_sha256: artifactSha256,
    artifact_root_verified: artifactRootVerified,
    binary_hash: binaryHash,
    artifact: {
      path: `artifacts/${artifactName}`,
      bytes: artifactStat.size,
      sha256: artifactSha256,
      merkle,
    },
    ...(caps === undefined ? {} : { caps }),
  };
  const enclaveManifestBytes = Buffer.from(`${stableJson(enclaveManifest)}\n`);
  const manifestHash = await blake3HexBytes(enclaveManifestBytes);
  const enclaveId = await blake3HexText(`${adminKey.publicKeyHex}${args.modelId}${artifactRoot}${manifestHash}${binaryHash}`);

  const outDir = resolveRepo(args.outDir);
  const enclaveOutDir = path.join(outDir, enclaveId);
  fs.rmSync(enclaveOutDir, { recursive: true, force: true });
  fs.mkdirSync(enclaveOutDir, { recursive: true });

  const tempDir = fs.mkdtempSync(path.join(enclaveOutDir, '.stage-'));
  const stageDir = path.join(tempDir, enclaveId);
  fs.mkdirSync(path.join(stageDir, 'artifacts'), { recursive: true });
  fs.writeFileSync(path.join(stageDir, 'enclave-manifest.json'), enclaveManifestBytes);
  fs.copyFileSync(artifactPath, path.join(stageDir, 'artifacts', artifactName));

  const ext = args.format === 'tar.zst' ? 'tar.zst' : 'tar.gz';
  const archivePath = path.join(enclaveOutDir, `${enclaveId}.${ext}`);
  if (args.format === 'tar.zst') {
    assertTool('tar');
    assertTool('zstd', 'install zstd or pass --format tar.gz for local rehearsal');
    const tarPath = path.join(tempDir, `${enclaveId}.tar`);
    run('tar', ['-cf', tarPath, '-C', tempDir, enclaveId]);
    run('zstd', ['-q', '-f', '-o', archivePath, tarPath]);
  } else {
    assertTool('tar');
    run('tar', ['-czf', archivePath, '-C', tempDir, enclaveId]);
  }
  fs.rmSync(tempDir, { recursive: true, force: true });

  const bundleSha256 = await sha256File(archivePath);
  const bundleBytes = fs.statSync(archivePath).size;
  const bundleFile = path.basename(archivePath);
  const publicManifestFile = `${enclaveId}.json`;
  const bundleUrl = args.bundleUrl || urlJoin(args.baseUrl, bundleFile);
  const manifestUrl = args.manifestUrl || urlJoin(args.baseUrl, publicManifestFile);
  requirePublicHttpsUrl(bundleUrl, '--bundle-url');
  requirePublicHttpsUrl(manifestUrl, '--manifest-url');
  for (const [index, mirror] of args.mirrors.entries()) {
    requirePublicHttpsUrl(mirror, `--mirror[${index}]`);
  }

  const enclaveForSignature = {
    enclave_id: enclaveId,
    model_id: args.modelId,
    backend: args.backend,
    artifact_root: artifactRoot,
    artifact_root_kind: 'blake3_merkle_v1',
    artifact_source: artifactSource,
    source_sha256: artifactSha256,
    manifest_hash: manifestHash,
    binary_hash: binaryHash,
    distribution: {
      bundle_url: bundleUrl,
      manifest_url: manifestUrl,
      bundle_sha256: bundleSha256,
      bundle_bytes: bundleBytes,
      mirrors: args.mirrors,
    },
  };
  const adminSignature = signDistribution(adminKey, adminKey.publicKeyHex, enclaveForSignature);
  const distribution = {
    ...enclaveForSignature.distribution,
    admin_signature: adminSignature,
  };

  const publicManifest = {
    schema_version: 1,
    kind: 'mayhem-enclave-download-manifest-v1',
    admin_pubkey: adminKey.publicKeyHex,
    enclave_id: enclaveId,
    model_id: args.modelId,
    backend: args.backend,
    artifact_root: artifactRoot,
    artifact_root_kind: 'blake3_merkle_v1',
    artifact_source: artifactSource,
    source_sha256: artifactSha256,
    artifact_root_verified: artifactRootVerified,
    manifest_hash: manifestHash,
    binary_hash: binaryHash,
    enclave_manifest_file: 'enclave-manifest.json',
    artifact: enclaveManifest.artifact,
    bundle: {
      file: bundleFile,
      format: args.format,
      sha256: bundleSha256,
      bytes: bundleBytes,
    },
    distribution,
  };
  const publicManifestPath = path.join(enclaveOutDir, publicManifestFile);
  writeJson(publicManifestPath, publicManifest);

  const evidence = {
    distributions: [
      {
        enclave_id: enclaveId,
        admin_signed: true,
        ...distribution,
      },
    ],
  };
  const evidencePath = path.join(enclaveOutDir, 'enclave-downloads.json');
  writeJson(evidencePath, evidence);

  const launchPatch = {
    admin: {
      peer_pubkey: adminKey.publicKeyHex,
    },
    canonical_enclave_fields: {
      enclave_id: enclaveId,
      model_id: args.modelId,
      backend: args.backend,
      artifact_root: artifactRoot,
      artifact_root_kind: 'blake3_merkle_v1',
      artifact_source: artifactSource,
      source_sha256: artifactSha256,
      manifest_hash: manifestHash,
      binary_hash: binaryHash,
      ...(caps === undefined ? {} : { caps }),
      distribution,
    },
    evidence: {
      enclave_downloads: [
        `file:${relativeFile(evidencePath)}#sha256:${await sha256File(evidencePath)}`,
      ],
    },
  };
  const patchPath = path.join(enclaveOutDir, 'launch-manifest-patch.json');
  writeJson(patchPath, launchPatch);

  return {
    ok: true,
    artifact_root_verified: artifactRootVerified,
    admin_pubkey: adminKey.publicKeyHex,
    enclave_id: enclaveId,
    model_id: args.modelId,
    backend: args.backend,
    artifact_root: artifactRoot,
    artifact_root_kind: 'blake3_merkle_v1',
    artifact_source: artifactSource,
    source_sha256: artifactSha256,
    manifest_hash: manifestHash,
    binary_hash: binaryHash,
    distribution,
    files: {
      bundle: relativeFile(archivePath),
      public_manifest: relativeFile(publicManifestPath),
      evidence: relativeFile(evidencePath),
      launch_manifest_patch: relativeFile(patchPath),
    },
    copy_paste: {
      upload_bundle_to: bundleUrl,
      upload_manifest_to: manifestUrl,
      launch_patch_file: relativeFile(patchPath),
    },
  };
}

async function main() {
  try {
    const args = parseArgs(process.argv.slice(2));
    const report = await build(args);
    if (args.json) {
      console.log(JSON.stringify(report, null, 2));
    } else {
      console.log('Mayhem enclave distribution build: ok');
      console.log(`Copy/paste bundle path: ${report.files.bundle}`);
      console.log(`Copy/paste public manifest path: ${report.files.public_manifest}`);
      console.log(`Copy/paste launch patch path: ${report.copy_paste.launch_patch_file}`);
      console.log(`Copy/paste upload bundle URL: ${report.copy_paste.upload_bundle_to}`);
      console.log(`Copy/paste upload manifest URL: ${report.copy_paste.upload_manifest_to}`);
    }
    if (!report.ok) process.exitCode = 1;
  } catch (error) {
    console.error(`error: ${error.message}`);
    process.exitCode = 1;
  }
}

await main();

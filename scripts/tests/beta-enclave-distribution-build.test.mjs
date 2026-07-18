import assert from 'node:assert/strict';
import crypto from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import {
  buildMerkleManifest,
  distributionSigningPayload,
} from '../beta-enclave-distribution-build.mjs';
import { catalogEnclaveId } from '../catalog-enclave-id.mjs';

const scriptPath = fileURLToPath(new URL('../beta-enclave-distribution-build.mjs', import.meta.url));
const ed25519SpkiPrefix = Buffer.from('302a300506032b6570032100', 'hex');
const revision = '12'.repeat(20);
const chunkSize = 16;
const parakeetSidecars = {
  transformers_config: 'config.json',
  transformers_generation_config: 'generation_config.json',
  transformers_processor_config: 'processor_config.json',
  transformers_tokenizer_json: 'tokenizer.json',
  transformers_tokenizer_config: 'tokenizer_config.json',
};

function sha256(bytes) {
  return crypto.createHash('sha256').update(bytes).digest('hex');
}

async function catalogFile(pathname, repo) {
  const bytes = fs.readFileSync(pathname);
  const merkle = await buildMerkleManifest(pathname, chunkSize);
  return {
    source: {
      kind: 'huggingface',
      repo,
      revision,
    },
    artifact_root: merkle.root,
    artifact_root_kind: 'blake3_merkle_v1',
    weights_bytes: bytes.length,
    source_sha256: sha256(bytes),
  };
}

async function distributionFixture(t, sidecarPaths, expectSuccess = true) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-enclave-distribution-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const snapshot = path.join(root, 'snapshot');
  const artifactPathInRepo = sidecarPaths ? 'model.safetensors' : 'model.gguf';
  const artifactPath = path.join(snapshot, artifactPathInRepo);
  fs.mkdirSync(path.dirname(artifactPath), { recursive: true });
  fs.writeFileSync(artifactPath, 'parakeet model payload\n');

  const modelId = sidecarPaths
    ? 'nvidia/parakeet-tdt-0.6b-v3'
    : 'test/zero-sidecar-model';
  const repo = sidecarPaths
    ? 'nvidia/parakeet-tdt-0.6b-v3'
    : 'test/zero-sidecar-model';
  const sidecars = {};
  for (const [name, sidecarPath] of Object.entries(sidecarPaths || {})) {
    const localPath = path.join(snapshot, sidecarPath);
    fs.mkdirSync(path.dirname(localPath), { recursive: true });
    fs.writeFileSync(localPath, `${name} payload\n`);
    sidecars[name] = {
      ...(await catalogFile(localPath, repo)),
      path: sidecarPath,
    };
  }

  const artifact = {
    ...(await catalogFile(artifactPath, repo)),
    engine: sidecarPaths ? 'transformers-asr' : 'llama.cpp',
    path: artifactPathInRepo,
    sidecars,
  };
  const catalogPath = path.join(root, 'catalog.json');
  fs.writeFileSync(catalogPath, JSON.stringify({
    models: [{
      model_id: modelId,
      artifacts: {
        safetensors: artifact,
      },
    }],
  }));

  const outDir = path.join(root, 'out');
  const child = spawnSync(process.execPath, [
    scriptPath,
    '--artifact', artifactPath,
    '--model-id', modelId,
    '--backend', sidecarPaths ? 'transformers-asr' : 'llama.cpp',
    '--artifact-repo', repo,
    '--artifact-revision', revision,
    '--artifact-path', artifactPathInRepo,
    '--base-url', 'https://downloads.mayhem.network/enclaves',
    '--admin-seed-hex', '01'.repeat(32),
    '--binary-hash', '99'.repeat(32),
    '--catalog', catalogPath,
    '--out-dir', outDir,
    '--chunk-size', String(chunkSize),
    '--format', 'tar.gz',
    '--json',
  ], {
    encoding: 'utf8',
    maxBuffer: 10 * 1024 * 1024,
  });
  if (!expectSuccess) return { child };
  assert.equal(child.status, 0, child.stderr || child.stdout);
  assert.equal(child.stderr, '');

  const report = JSON.parse(child.stdout);
  const publicManifest = JSON.parse(fs.readFileSync(report.files.public_manifest, 'utf8'));
  const launchPatch = JSON.parse(fs.readFileSync(report.files.launch_manifest_patch, 'utf8'));
  const archive = spawnSync('tar', ['-tzf', report.files.bundle], {
    encoding: 'utf8',
  });
  assert.equal(archive.status, 0, archive.stderr);
  const archiveFiles = archive.stdout.trim().split('\n');
  return { report, publicManifest, launchPatch, archiveFiles };
}

function assertCanonicalId(report) {
  const roots = Object.fromEntries(
    Object.entries(report.artifact_sidecars)
      .map(([name, sidecar]) => [name, sidecar.artifact_root])
  );
  assert.equal(report.enclave_id, catalogEnclaveId({
    adminPubkey: report.admin_pubkey,
    modelId: report.model_id,
    artifactRoot: report.artifact_root,
    artifactSidecarRoots: roots,
    manifestHash: report.manifest_hash,
  }));
}

function verifyDistributionSignature(report) {
  const publicKey = crypto.createPublicKey({
    key: Buffer.concat([ed25519SpkiPrefix, Buffer.from(report.admin_pubkey, 'hex')]),
    format: 'der',
    type: 'spki',
  });
  return crypto.verify(
    null,
    distributionSigningPayload(report.admin_pubkey, report),
    publicKey,
    Buffer.from(report.distribution.admin_signature, 'hex'),
  );
}

test('catalog enclave ID matches Rust v2 vectors for zero and five sidecars', () => {
  assert.equal(
    catalogEnclaveId({
      adminPubkey: 'admin',
      modelId: 'model',
      artifactRoot: 'artifact',
      manifestHash: 'manifest',
    }),
    '8f1f159d6d6bfa4c57361a26613dd2b2ecc0e73e6cab6d5614b1d3097aefe8a5',
  );

  const roots = {
    transformers_config: '11'.repeat(32),
    transformers_generation_config: '22'.repeat(32),
    transformers_processor_config: '33'.repeat(32),
    transformers_tokenizer_config: '44'.repeat(32),
    transformers_tokenizer_json: '55'.repeat(32),
  };
  const parakeetId = catalogEnclaveId({
    adminPubkey: 'admin',
    modelId: 'nvidia/parakeet-tdt-0.6b-v3',
    artifactRoot: 'aa'.repeat(32),
    artifactSidecarRoots: roots,
    manifestHash: '66'.repeat(32),
  });
  assert.equal(parakeetId, '92f874cc308be95ad33bc745139891d73c4f0e7fc873d5a49c4f5b4e86745db5');
  assert.equal(parakeetId, catalogEnclaveId({
    adminPubkey: 'admin',
    modelId: 'nvidia/parakeet-tdt-0.6b-v3',
    artifactRoot: 'aa'.repeat(32),
    artifactSidecarRoots: Object.fromEntries(Object.entries(roots).reverse()),
    manifestHash: '66'.repeat(32),
  }));
  assert.equal(
    catalogEnclaveId({
      adminPubkey: 'admin',
      modelId: 'model'.repeat(300),
      artifactRoot: 'artifact',
      artifactSidecarRoots: roots,
      manifestHash: 'manifest',
    }),
    '2d15fa52f561095c18e412e6249c27e127079cb85467ac3492bf2b40a7dbd9f6',
  );
});

test('distribution builder preserves the zero-sidecar current format', async (t) => {
  const { report, publicManifest, launchPatch, archiveFiles } =
    await distributionFixture(t, null);

  assert.deepEqual(report.artifact_sidecars, {});
  assert.deepEqual(report.artifact_sidecar_roots, {});
  assert.deepEqual(publicManifest.artifact_sidecars, {});
  assert.deepEqual(launchPatch.canonical_enclave_fields.artifact_sidecars, {});
  assertCanonicalId(report);
  assert.equal(verifyDistributionSignature(report), true);
  assert.deepEqual(archiveFiles.sort(), [
    `${report.enclave_id}/`,
    `${report.enclave_id}/artifacts/`,
    `${report.enclave_id}/artifacts/model.gguf`,
    `${report.enclave_id}/enclave-manifest.json`,
  ].sort());
});

test('distribution builder verifies, binds, and archives Parakeet five sidecars', async (t) => {
  const { report, publicManifest, launchPatch, archiveFiles } =
    await distributionFixture(t, parakeetSidecars);
  const names = Object.keys(parakeetSidecars).sort();

  assert.deepEqual(Object.keys(report.artifact_sidecars), names);
  assert.deepEqual(Object.keys(report.artifact_sidecar_roots), names);
  assert.deepEqual(Object.keys(publicManifest.artifact_sidecars), names);
  assert.deepEqual(Object.keys(launchPatch.canonical_enclave_fields.artifact_sidecars), names);
  for (const [name, sidecarPath] of Object.entries(parakeetSidecars)) {
    const bundled = publicManifest.artifact_sidecars[name];
    assert.equal(bundled.bundle_path, `artifacts/${sidecarPath}`);
    assert.equal(bundled.artifact_root, report.artifact_sidecar_roots[name]);
    assert.ok(archiveFiles.includes(`${report.enclave_id}/artifacts/${sidecarPath}`));
  }
  assertCanonicalId(report);
  assert.equal(verifyDistributionSignature(report), true);

  const tampered = structuredClone(report);
  tampered.artifact_sidecars.transformers_config.artifact_root = '00'.repeat(32);
  assert.equal(verifyDistributionSignature(tampered), false);
});

test('distribution builder rejects incomplete transformers-asr sidecars', async (t) => {
  const incomplete = { ...parakeetSidecars };
  delete incomplete.transformers_processor_config;
  const { child } = await distributionFixture(t, incomplete, false);

  assert.notEqual(child.status, 0);
  assert.match(
    child.stderr,
    /sidecars\.transformers_processor_config must be an object/,
  );
});

import assert from 'node:assert/strict';
import crypto from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import {
  CONTRACT_CODE_PATHS,
  INTERCOM_BUNDLE_ASSET_PREFIX,
  RELEASE_MANIFEST_PATH,
  createIntercomBundleManifest,
  resolveIntercomRoot,
  verifyIntercomBundleManifest,
  verifyReleaseIdentity,
  verifyStartupReleaseIdentity,
} from '../src/release-identity.js';
import { createServer } from '../src/rpc.js';

const INTERCOM_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const CONTRACT_CODE_DIGEST_DOMAIN = 'mayhem-intercom-contract-code-v1\0';
const SHA256_PATTERN = /^[0-9a-f]{64}$/;

const sha256 = (bytes) => crypto.createHash('sha256').update(bytes).digest('hex');

const contractCodeDigest = (files) => {
  const digest = crypto.createHash('sha256');
  digest.update(CONTRACT_CODE_DIGEST_DOMAIN);
  for (const file of files) {
    digest.update(file.path);
    digest.update('\0');
    digest.update(String(file.bytes.length));
    digest.update('\0');
    digest.update(file.bytes);
    digest.update('\0');
  }
  return digest.digest('hex');
};

const readManifest = (rootDir) =>
  JSON.parse(fs.readFileSync(path.join(rootDir, RELEASE_MANIFEST_PATH), 'utf8'));

const writeManifest = (rootDir, manifest) => {
  fs.writeFileSync(
    path.join(rootDir, RELEASE_MANIFEST_PATH),
    `${JSON.stringify(manifest)}\n`
  );
};

const rehashFixtureManifest = (rootDir, { syncContractVersion = false } = {}) => {
  const manifest = readManifest(rootDir);
  const files = CONTRACT_CODE_PATHS.map((relativePath) => {
    const bytes = fs.readFileSync(path.join(rootDir, relativePath));
    return { path: relativePath, sha256: sha256(bytes), bytes };
  });
  manifest.files = files.map(({ path: filePath, sha256: fileSha256 }) => ({
    path: filePath,
    sha256: fileSha256,
  }));
  manifest.contract_code_sha256 = contractCodeDigest(files);
  if (syncContractVersion) {
    const source = files
      .find((file) => file.path === 'contract/contract.js')
      .bytes.toString('utf8');
    const match = source.match(
      /^\s*export\s+const\s+CONTRACT_VERSION\s*=\s*([1-9][0-9]*)\s*;\s*$/m
    );
    assert.ok(match, 'fixture contract.js has an explicit CONTRACT_VERSION export');
    manifest.contract_version = Number(match[1]);
  }
  writeManifest(rootDir, manifest);
};

const fixtureRoot = (t) => {
  const rootDir = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-release-identity-'));
  t.after(() => fs.rmSync(rootDir, { recursive: true, force: true }));
  for (const relativePath of [RELEASE_MANIFEST_PATH, ...CONTRACT_CODE_PATHS]) {
    const destination = path.join(rootDir, relativePath);
    fs.mkdirSync(path.dirname(destination), { recursive: true });
    fs.copyFileSync(path.join(INTERCOM_ROOT, relativePath), destination);
  }
  rehashFixtureManifest(rootDir, { syncContractVersion: true });
  return rootDir;
};

const bundleFixtureRoot = (t) => {
  const rootDir = fixtureRoot(t);
  const extraFiles = new Map([
    ['package.json', '{"name":"intercom-fixture","type":"module"}\n'],
    ['features/relay/index.js', 'export const relay = true;\n'],
    ['src/main.js', 'export const main = true;\n'],
  ]);
  for (const [relativePath, contents] of extraFiles) {
    const destination = path.join(rootDir, relativePath);
    fs.mkdirSync(path.dirname(destination), { recursive: true });
    fs.writeFileSync(destination, contents);
  }
  return rootDir;
};

const clone = (value) => JSON.parse(JSON.stringify(value));

test('checked-in Intercom release identity verifies exact sorted contract code bytes', () => {
  const identity = verifyReleaseIdentity({ rootDir: INTERCOM_ROOT });

  assert.equal(identity.releaseVersion, '0.2.71');
  assert.equal(identity.contractVersion, 19);
  assert.match(identity.contractCodeSha256, /^[0-9a-f]{64}$/);
  assert.deepEqual(
    identity.files.map((file) => file.path),
    [...CONTRACT_CODE_PATHS]
  );
});

test('Pear disk runs use the authoritative physical app root and reject link-only launch', () => {
  assert.equal(
    resolveIntercomRoot({
      moduleUrl: 'pear://dev/src/release-identity.js',
      app: { key: null, dir: INTERCOM_ROOT },
    }),
    INTERCOM_ROOT
  );
  assert.throws(
    () => resolveIntercomRoot({
      moduleUrl: 'pear://aaaaaaaa/src/release-identity.js',
      app: { key: Buffer.alloc(32), dir: INTERCOM_ROOT },
    }),
    /local authenticated release tree.*link-only launch is unsupported/i
  );
});

test('startup verifier fails loudly when a contract code file is mutated', (t) => {
  const rootDir = fixtureRoot(t);
  fs.appendFileSync(path.join(rootDir, 'features/mayhem/index.js'), '\n// mutated\n');

  assert.throws(
    () => verifyStartupReleaseIdentity({ rootDir }),
    (error) => {
      assert.equal(error.code, 'INTERCOM_STARTUP_RELEASE_IDENTITY_INVALID');
      assert.match(error.message, /startup release identity verification failed/i);
      assert.match(error.message, /features\/mayhem\/index\.js SHA-256 mismatch/);
      return true;
    }
  );
});

test('startup verifier fails loudly when a contract code file is missing', (t) => {
  const rootDir = fixtureRoot(t);
  fs.unlinkSync(path.join(rootDir, 'contract/protocol.js'));

  assert.throws(
    () => verifyStartupReleaseIdentity({ rootDir }),
    (error) => {
      assert.equal(error.code, 'INTERCOM_STARTUP_RELEASE_IDENTITY_INVALID');
      assert.match(error.message, /contract code file contract\/protocol\.js is missing/);
      return true;
    }
  );
});

test('startup verifier rejects a manifest contract version that differs from contract.js', (t) => {
  const rootDir = fixtureRoot(t);
  const manifestPath = path.join(rootDir, RELEASE_MANIFEST_PATH);
  const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
  manifest.contract_version += 1;
  fs.writeFileSync(manifestPath, `${JSON.stringify(manifest)}\n`);

  assert.throws(
    () => verifyStartupReleaseIdentity({ rootDir }),
    (error) => {
      assert.equal(error.code, 'INTERCOM_STARTUP_RELEASE_IDENTITY_INVALID');
      assert.match(error.message, /contract_version mismatch/);
      return true;
    }
  );
});

test('source CONTRACT_VERSION mismatch fails after contract file digests are refreshed', (t) => {
  const rootDir = fixtureRoot(t);
  const contractPath = path.join(rootDir, 'contract/contract.js');
  const manifest = readManifest(rootDir);
  const source = fs.readFileSync(contractPath, 'utf8');
  const changedSource = source.replace(
    /^(\s*export\s+const\s+CONTRACT_VERSION\s*=\s*)[1-9][0-9]*(\s*;\s*)$/m,
    `$1${manifest.contract_version + 1}$2`
  );
  assert.notEqual(changedSource, source, 'fixture contract version was changed');
  fs.writeFileSync(
    contractPath,
    changedSource
  );
  rehashFixtureManifest(rootDir);

  assert.throws(
    () => verifyStartupReleaseIdentity({ rootDir }),
    (error) => {
      assert.equal(error.code, 'INTERCOM_STARTUP_RELEASE_IDENTITY_INVALID');
      assert.match(error.message, /contract_version mismatch/);
      return true;
    }
  );
});

test('startup verifier fails loudly when the explicit release manifest is missing', (t) => {
  const rootDir = fixtureRoot(t);
  fs.unlinkSync(path.join(rootDir, RELEASE_MANIFEST_PATH));

  assert.throws(
    () => verifyStartupReleaseIdentity({ rootDir }),
    /release manifest contract\/release\.json is missing/
  );
});

test('bundle manifest covers the complete runtime tree in deterministic path order', (t) => {
  const rootDir = bundleFixtureRoot(t);
  const manifest = createIntercomBundleManifest({ rootDir });
  const expectedPaths = [
    'contract/contract.js',
    'contract/protocol.js',
    'contract/release.json',
    'features/mayhem/index.js',
    'features/relay/index.js',
    'package.json',
    'src/main.js',
  ].map((relativePath) => `${INTERCOM_BUNDLE_ASSET_PREFIX}${relativePath}`);

  assert.deepEqual(manifest.assets.map((asset) => asset.path), expectedPaths);
  assert.ok(manifest.assets.every((asset) => SHA256_PATTERN.test(asset.sha256)));
  assert.equal(
    verifyIntercomBundleManifest(manifest, { rootDir }).assets.length,
    expectedPaths.length
  );
});

test('bundle verifier rejects mutated, missing, and extra runtime files', (t) => {
  const rootDir = bundleFixtureRoot(t);
  const manifest = createIntercomBundleManifest({ rootDir });
  const runtimePath = path.join(rootDir, 'src/main.js');

  fs.appendFileSync(runtimePath, '// changed\n');
  assert.throws(
    () => verifyIntercomBundleManifest(manifest, { rootDir }),
    /runtime file src\/main\.js SHA-256 mismatch/
  );

  fs.writeFileSync(runtimePath, 'export const main = true;\n');
  fs.unlinkSync(runtimePath);
  assert.throws(
    () => verifyIntercomBundleManifest(manifest, { rootDir }),
    /listed runtime file is missing: src\/main\.js/
  );

  fs.writeFileSync(runtimePath, 'export const main = true;\n');
  fs.writeFileSync(path.join(rootDir, 'unlisted.js'), 'export default false;\n');
  assert.throws(
    () => verifyIntercomBundleManifest(manifest, { rootDir }),
    /runtime tree contains unlisted extra file: unlisted\.js/
  );
});

test('bundle verifier rejects omitted, duplicate, unsorted, and unsafe asset paths', (t) => {
  const rootDir = bundleFixtureRoot(t);
  const manifest = createIntercomBundleManifest({ rootDir });

  const omitted = clone(manifest);
  omitted.assets.pop();
  assert.throws(
    () => verifyIntercomBundleManifest(omitted, { rootDir }),
    /runtime tree contains unlisted extra file/
  );

  const duplicate = clone(manifest);
  duplicate.assets.splice(1, 0, { ...duplicate.assets[0] });
  assert.throws(
    () => verifyIntercomBundleManifest(duplicate, { rootDir }),
    /assets contains duplicate path/
  );

  const unsorted = clone(manifest);
  [unsorted.assets[0], unsorted.assets[1]] = [unsorted.assets[1], unsorted.assets[0]];
  assert.throws(
    () => verifyIntercomBundleManifest(unsorted, { rootDir }),
    /assets paths must be sorted/
  );

  const unsafe = clone(manifest);
  unsafe.assets[0].path = `${INTERCOM_BUNDLE_ASSET_PREFIX}../outside.js`;
  assert.throws(
    () => verifyIntercomBundleManifest(unsafe, { rootDir }),
    /assets\[0\]\.path is unsafe/
  );
});

test('bundle builder rejects symbolic links in the runtime tree', (t) => {
  const rootDir = bundleFixtureRoot(t);
  const linkPath = path.join(rootDir, 'src/main-link.js');
  try {
    fs.symlinkSync('main.js', linkPath);
  } catch (error) {
    if (error?.code === 'EPERM' || error?.code === 'EACCES') {
      t.skip(`symbolic links unavailable: ${error.code}`);
      return;
    }
    throw error;
  }

  assert.throws(
    () => createIntercomBundleManifest({ rootDir }),
    /runtime tree path src\/main-link\.js must not be a symbolic link/
  );
});

test('health exposes only a verified Intercom contract identity', async (t) => {
  const releaseIdentity = verifyReleaseIdentity({ rootDir: INTERCOM_ROOT });
  const server = createServer({}, { releaseIdentity });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise((resolve) => server.close(resolve)));

  const address = server.address();
  const response = await fetch(`http://127.0.0.1:${address.port}/v1/health`);

  assert.equal(response.status, 200);
  assert.deepEqual(await response.json(), {
    ok: true,
    contract_version: 19,
    contract_code_sha256: releaseIdentity.contractCodeSha256,
  });
});

test('health has no compatibility fallback when release identity is unavailable', async (t) => {
  const server = createServer({});
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise((resolve) => server.close(resolve)));

  const address = server.address();
  const response = await fetch(`http://127.0.0.1:${address.port}/v1/health`);

  assert.equal(response.status, 500);
  assert.deepEqual(await response.json(), { error: 'An internal error occurred.' });
});

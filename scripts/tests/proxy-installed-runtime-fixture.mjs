// Disposable test signing keys never become trusted production release keys.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { pathToFileURL } from 'node:url';

const [action, source, stage] = process.argv.slice(2);
if (![source, stage].every(p => p && path.isAbsolute(p))) throw Error('absolute fixture roots required');
const output = path.dirname(stage);
const app = path.join(stage, 'share/mayhem/intercom');
const sha256 = file => crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
if (action === 'prepare') {
  // Materialize cached dependencies. Ignore executable symlinks; runtime imports
  // use packages, and the signed archive deliberately forbids all symlink entries.
  for (const relative of ['intercom', 'contracts']) {
    const input = path.join(source, relative, 'node_modules');
    const destination = path.join(stage, 'share/mayhem', relative, 'node_modules');
    if (!fs.statSync(input).isDirectory()) throw Error(`missing cached ${relative} dependencies`);
    fs.cpSync(input, destination, {
      recursive: true, dereference: true, errorOnExist: true, force: false,
      filter: entry => !path.relative(input, entry).split(path.sep).includes('.bin'),
    });
  }
  const { CONTRACT_CODE_PATHS, RELEASE_MANIFEST_PATH, verifyReleaseIdentity } = await import(pathToFileURL(path.join(app, 'src/release-identity.js')));
  const release = JSON.parse(fs.readFileSync(path.join(app, RELEASE_MANIFEST_PATH)));
  const digest = crypto.createHash('sha256').update('mayhem-intercom-contract-code-v1\0');
  release.files = CONTRACT_CODE_PATHS.map(relative => {
    const bytes = fs.readFileSync(path.join(app, relative));
    digest.update(relative).update('\0').update(String(bytes.length)).update('\0').update(bytes).update('\0');
    return { path: relative, sha256: sha256(path.join(app, relative)) };
  });
  release.contract_code_sha256 = digest.digest('hex');
  fs.writeFileSync(path.join(app, RELEASE_MANIFEST_PATH), JSON.stringify(release) + '\n');
  verifyReleaseIdentity({ rootDir: app });
} else if (action === 'sign') {
  const bytes = fs.readFileSync(path.join(stage, 'manifest.json'));
  const manifest = JSON.parse(bytes);
  const { privateKey, publicKey } = crypto.generateKeyPairSync('ed25519');
  const hex = publicKey.export({ format: 'der', type: 'spki' }).subarray(-32).toString('hex');
  const keyId = 'proxy-installed-runtime-fixture';
  fs.writeFileSync(path.join(output, 'fixture-key.json'), JSON.stringify({ key_id: keyId, alg: 'ed25519', public_key: hex, status: 'active', created_at: manifest.built_at_utc }) + '\n');
  fs.writeFileSync(path.join(output, 'fixture.sig'), JSON.stringify({
    schema_version: 1, alg: 'ed25519', signed_path: `mayhem-${manifest.version}-${manifest.target}.manifest.json`,
    key_id: keyId, public_key: hex, sha256: sha256(path.join(stage, 'manifest.json')),
    sig: crypto.sign(null, Buffer.concat([Buffer.from('mayhem.release-manifest.v1\n'), bytes]), privateKey).toString('hex'),
  }) + '\n');
} else if (action === 'review') {
  const bytes = fs.readFileSync(path.join(stage, 'manifest.json'));
  const manifest = JSON.parse(bytes);
  const key = JSON.parse(fs.readFileSync(path.join(output, 'fixture-key.json')));
  const signature = JSON.parse(fs.readFileSync(path.join(output, 'fixture.sig')));
  const publicKey = crypto.createPublicKey({ format: 'der', type: 'spki', key: Buffer.concat([
    Buffer.from('302a300506032b6570032100', 'hex'), Buffer.from(key.public_key, 'hex'),
  ]) });
  if (signature.key_id !== key.key_id || signature.public_key !== key.public_key
      || signature.signed_path !== `mayhem-${manifest.version}-${manifest.target}.manifest.json`
      || signature.sha256 !== sha256(path.join(stage, 'manifest.json'))
      || !crypto.verify(null, Buffer.concat([Buffer.from('mayhem.release-manifest.v1\n'), bytes]), publicKey, Buffer.from(signature.sig, 'hex'))) {
    throw Error('fixture manifest authentication failed');
  }
  const installed = path.join(output, 'installed');
  // The installer activates bin/ and share/mayhem/, not archive-root README or
  // release metadata. Verify every activated runtime asset, not unstaged docs.
  const runtimeAssets = manifest.assets.filter(a => a.path.startsWith('bin/') || a.path.startsWith('share/mayhem/'));
  for (const asset of runtimeAssets) {
    if (sha256(path.join(installed, asset.path)) !== asset.sha256) throw Error(`installed asset drift: ${asset.path}`);
  }
  const bootstrap = JSON.parse(fs.readFileSync(path.join(output, 'bootstrap.json')));
  const run = JSON.parse(fs.readFileSync(path.join(output, 'run.json')));
  if (bootstrap.upstream_calls !== 0 || run.upstream_calls !== 2 || run.child_count !== 1 || run.budget.used_attempts !== 2) throw Error('acceptance evidence mismatch');
  fs.writeFileSync(path.join(output, 'review.json'), JSON.stringify({
    schema_version: 1, source_git_sha: manifest.source_git_sha, version: manifest.version, target: manifest.target,
    fixture: 'signed disposable installation of actual candidate binaries; synthetic loopback services and wallet',
    binaries: manifest.binaries, installed_assets_verified: runtimeAssets.length,
    no_asset_override: true, no_checkout_cwd: true, bootstrap, run,
    limitations: ['debug candidate binaries; not final release-profile qualification', 'cached dependency snapshot; not fresh package hydration', 'no production signing key, registration, ledger, external model or payment'],
  }, null, 2) + '\n');
} else throw Error('unknown fixture action');

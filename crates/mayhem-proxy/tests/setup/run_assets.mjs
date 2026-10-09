// Disposable acceptance assets, not a release or a bypass of startup verification.
// Uncommitted parallel Intercom work must not affect the actual wallet helper.
import fs from 'node:fs';
import crypto from 'node:crypto';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';
const [root, destination] = process.argv.slice(2);
if (!root || !destination || !path.isAbsolute(root) || !path.isAbsolute(destination)) throw Error('explicit absolute fixture roots required');
fs.mkdirSync(destination, { mode: 0o700 });
const archive = spawnSync('git', ['archive', '--format=tar', 'HEAD', 'intercom'], { cwd: root, maxBuffer: 64 * 1024 * 1024 });
if (archive.status !== 0) throw Error('committed candidate asset snapshot unavailable');
const extracted = spawnSync('tar', ['-xf', '-', '-C', destination], { input: archive.stdout, maxBuffer: 1024 * 1024 });
if (extracted.status !== 0) throw Error('candidate fixture extraction failed');
fs.symlinkSync(path.join(root, 'intercom/node_modules'), path.join(destination, 'intercom/node_modules'), 'dir');
const { verifyReleaseIdentity, CONTRACT_CODE_PATHS, RELEASE_MANIFEST_PATH } = await import(pathToFileURL(path.join(destination, 'intercom/src/release-identity.js')));
// Same isolated-fixture sealing algorithm as release-identity.test.js. This
// manifest never leaves the disposable directory and retains candidate versions.
const app = path.join(destination, 'intercom');
const manifest = JSON.parse(fs.readFileSync(path.join(app, RELEASE_MANIFEST_PATH)));
const digest = crypto.createHash('sha256').update('mayhem-intercom-contract-code-v1\0');
manifest.files = CONTRACT_CODE_PATHS.map(relative => {
  const bytes = fs.readFileSync(path.join(app, relative));
  digest.update(relative); digest.update('\0'); digest.update(String(bytes.length)); digest.update('\0'); digest.update(bytes); digest.update('\0');
  return { path: relative, sha256: crypto.createHash('sha256').update(bytes).digest('hex') };
});
manifest.contract_code_sha256 = digest.digest('hex');
fs.writeFileSync(path.join(app, RELEASE_MANIFEST_PATH), JSON.stringify(manifest) + '\n');
const identity = verifyReleaseIdentity({ rootDir: path.join(destination, 'intercom') });
console.log(JSON.stringify({ kind: 'verified_committed_fixture_assets', release_version: identity.release_version, contract_version: identity.contract_version }));

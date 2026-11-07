import { createRequire } from 'node:module';
import { readFileSync, writeFileSync, renameSync, statSync } from 'node:fs';
import { createHash } from 'node:crypto';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const sha = bytes => createHash('sha256').update(bytes).digest('hex');
export function applyRecoveryDiagnostics(root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')) {
  const require = createRequire(path.join(root, 'package.json'));
  const msbRequire = createRequire(require.resolve('trac-msb/package.json'));
  const autobaseRoot = path.dirname(msbRequire.resolve('autobase/package.json'));
  const payloadRoot = path.join(root, 'patches/autobase-7.20.1');
  const manifest = JSON.parse(readFileSync(path.join(payloadRoot, 'manifest.json'), 'utf8'));
  if (JSON.parse(readFileSync(path.join(autobaseRoot, 'package.json'), 'utf8')).version !== manifest.version) {
    throw new Error('MSB read recovery requires review for this Autobase version');
  }
  const target = path.join(autobaseRoot, 'lib/fast-forward.js');
  const payload = readFileSync(path.join(payloadRoot, 'fast-forward.js'));
  if (sha(payload) !== manifest.after) throw new Error('MSB recovery payload checksum mismatch');
  const current = sha(readFileSync(target));
  if (current === manifest.after) return false;
  if (current !== manifest.before) throw new Error('MSB recovery dependency baseline differs');
  const temporary = target + '.msb-recovery-new';
  writeFileSync(temporary, payload, { mode: statSync(target).mode & 0o777 });
  renameSync(temporary, target);
  return true;
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  applyRecoveryDiagnostics();
  console.log('MSB read recovery dependency verified');
}

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } from 'node:fs';
import { createHash } from 'node:crypto';
import os from 'node:os';
import path from 'node:path';
import { applyRecoveryDiagnostics } from '../scripts/apply-msb-read-recovery.mjs';
const sha = s => createHash('sha256').update(s).digest('hex');
test('dependency install is exact, idempotent, and rejects version/source/payload drift', () => {
  const root = mkdtempSync(path.join(os.tmpdir(), 'msb-recovery-install-'));
  try {
    const msb = path.join(root, 'node_modules/trac-msb'), ab = path.join(msb, 'node_modules/autobase');
    const payload = path.join(root, 'patches/autobase-7.20.1');
    mkdirSync(path.join(ab, 'lib'), { recursive: true }); mkdirSync(payload, { recursive: true });
    writeFileSync(path.join(root, 'package.json'), '{}'); writeFileSync(path.join(msb, 'package.json'), '{}');
    const pkg = path.join(ab, 'package.json'), target = path.join(ab, 'lib/fast-forward.js');
    writeFileSync(pkg, JSON.stringify({version:'7.20.1'})); writeFileSync(target, 'before');
    writeFileSync(path.join(payload, 'manifest.json'), JSON.stringify({version:'7.20.1',before:sha('before'),after:sha('after')}));
    writeFileSync(path.join(payload, 'fast-forward.js'), 'after');
    assert.equal(applyRecoveryDiagnostics(root), true); assert.equal(readFileSync(target, 'utf8'), 'after');
    assert.equal(applyRecoveryDiagnostics(root), false);
    writeFileSync(target, 'other'); assert.throws(() => applyRecoveryDiagnostics(root), /baseline differs/);
    assert.equal(readFileSync(target, 'utf8'), 'other');
    writeFileSync(pkg, JSON.stringify({version:'7.20.2'})); assert.throws(() => applyRecoveryDiagnostics(root), /requires review/);
    writeFileSync(pkg, JSON.stringify({version:'7.20.1'})); writeFileSync(path.join(payload,'fast-forward.js'), 'altered');
    assert.throws(() => applyRecoveryDiagnostics(root), /checksum mismatch/);
  } finally { rmSync(root, {recursive:true,force:true}); }
});

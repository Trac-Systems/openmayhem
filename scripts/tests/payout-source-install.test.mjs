import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));
const validator = 'scripts/validate-tap-settlement-checkpoint.mjs';

test('a source install replaces the TAP verifier dependency and preserves other operator scripts', t => {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'payout-install-'));
  t.after(() => fs.rmSync(temp, { recursive: true, force: true }));
  const source = path.join(temp, 'source');
  const share = path.join(temp, 'share');
  for (const dir of ['intercom', 'contracts', 'catalog', 'scripts', 'crates/mayhem-cli/src']) {
    fs.mkdirSync(path.join(source, dir), { recursive: true });
  }
  fs.writeFileSync(path.join(source, 'RULES.md'), '');
  fs.writeFileSync(path.join(source, 'crates/mayhem-cli/src/helper.mjs'), '');
  fs.copyFileSync(path.join(root, validator), path.join(source, validator));
  fs.mkdirSync(path.join(share, 'scripts'), { recursive: true });
  fs.writeFileSync(path.join(share, validator), 'old validator');
  fs.writeFileSync(path.join(share, 'scripts/local.sh'), 'operator script');

  // Execute the installer's actual asset-copy functions without running its
  // unrelated build, tool installation or shell profile setup.
  const windows = process.platform === 'win32';
  const installer = fs.readFileSync(path.join(root, windows ? 'install.ps1' : 'install.sh'), 'utf8');
  const names = windows
    ? ['Join-RelativePath', 'Reset-RuntimeAssetDir', 'Copy-FilteredDirectory', 'Copy-SourceAssets']
    : ['reset_runtime_asset_dir', 'copy_runtime_tree', 'copy_source_assets'];
  const functions = names.map(name => {
    const declaration = windows ? 'function ' + name : name + '\\(\\)';
    const match = installer.match(new RegExp('^' + declaration + ' \\{[\\s\\S]*?^\\}', 'm'));
    assert.ok(match, name);
    return match[0];
  });
  const program = windows
    ? [ "$ErrorActionPreference='Stop'", '$SourceDir=$env:SOURCE_DIR', '$ShareDir=$env:SHARE_DIR',
        'function Write-Log {}', ...functions, 'Copy-SourceAssets' ].join('\n')
    : ['set -eu', 'log() { :; }', ...functions, 'copy_source_assets'].join('\n');
  const result = windows
    ? spawnSync('powershell.exe', ['-NoProfile', '-NonInteractive', '-EncodedCommand',
        Buffer.from(program, 'utf16le').toString('base64')],
        { env: { ...process.env, SOURCE_DIR: source, SHARE_DIR: share }, encoding: 'utf8' })
    : spawnSync('bash', ['-c', program],
        { env: { ...process.env, SOURCE_DIR: source, SHARE_DIR: share }, encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(fs.readFileSync(path.join(share, validator)), fs.readFileSync(path.join(root, validator)));
  assert.equal(fs.readFileSync(path.join(share, 'scripts/local.sh'), 'utf8'), 'operator script');
});

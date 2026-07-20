import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const syntheticScript = path.join(repoRoot, 'scripts/beta-synthetic-metrics-smoke.mjs');
const auditScript = path.join(repoRoot, 'scripts/beta-canonical-service-audit.mjs');

function run(args) {
  return spawnSync(process.execPath, args, {
    cwd: repoRoot,
    encoding: 'utf8',
  });
}

test('canonical beta audit accepts provider-owned bindings and rejects legacy or forged targets', () => {
  const tempRoot = path.join(repoRoot, '.mayhem-local');
  fs.mkdirSync(tempRoot, { recursive: true });
  const outDir = fs.mkdtempSync(path.join(tempRoot, 'canonical-payout-test-'));
  try {
    const rehearsal = run([
      syntheticScript,
      '--out-dir',
      outDir,
      '--tracker-recorded',
      '--json',
    ]);
    assert.equal(rehearsal.status, 0, rehearsal.stderr || rehearsal.stdout);
    const positive = JSON.parse(rehearsal.stdout);
    const acceptedAudit = JSON.parse(
      fs.readFileSync(positive.canonical_service_audit, 'utf8'),
    );
    assert.equal(acceptedAudit.canonical_service.provider_payout_bindings_verified, true);
    assert.equal(acceptedAudit.controls.provider_payout_bindings_permissionless, true);
    assert.equal(acceptedAudit.controls.provider_payout_bindings_ownership_verified, true);
    assert.equal(acceptedAudit.canonical_service.counts.provider_payout_bindings, 20);

    const snapshot = JSON.parse(fs.readFileSync(positive.contract_snapshot, 'utf8'));
    const bindingKey = Object.keys(snapshot)
      .find((key) => key.startsWith('payout/binding/'));
    assert.ok(bindingKey, 'synthetic snapshot must contain a payout binding');
    const provider = snapshot[bindingKey].provider;
    snapshot[bindingKey].provider_signature = '00'.repeat(64);
    const providerRecord = snapshot.providers.find((record) => record.provider === provider);
    providerRecord.payouts = {
      tnk: {
        method: 'tnk',
        addr: 'retired-admin-target',
        set_by: snapshot.admin,
        set_by_role: 'admin',
      },
    };
    const tamperedSnapshot = path.join(outDir, 'contract-state-tampered.json');
    fs.writeFileSync(tamperedSnapshot, `${JSON.stringify(snapshot, null, 2)}\n`);

    const rejected = run([
      auditScript,
      '--snapshot',
      tamperedSnapshot,
      '--catalog',
      path.join(outDir, 'catalog/models.json'),
      '--catalog-signature',
      path.join(outDir, 'catalog/signatures/models.json.sig'),
      '--catalog-key-dir',
      path.join(outDir, 'catalog/keys'),
      '--out',
      path.join(outDir, 'canonical-service-audit-tampered.json'),
      '--json',
    ]);
    assert.equal(rejected.status, 1, rejected.stderr);
    const report = JSON.parse(rejected.stdout);
    assert.equal(report.canonical_service.provider_payout_bindings_verified, false);
    assert.match(report.errors.join('\n'), /retired admin-set payout-target shape/);
    assert.match(report.errors.join('\n'), /provider_signature does not verify/);
  } finally {
    fs.rmSync(outDir, { recursive: true, force: true });
  }
});

import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const launchScript = path.join(repoRoot, 'scripts/beta-launch.mjs');
const templatePath = path.join(repoRoot, 'config/beta/testnet.template.json');
const template = JSON.parse(fs.readFileSync(templatePath, 'utf8'));

function runLaunch(manifest = template, { commands = true } = {}) {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-beta-payoutfree-'));
  const manifestPath = path.join(tmp, 'manifest.json');
  fs.writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
  const args = [
    launchScript,
    '--manifest',
    manifestPath,
    '--allow-placeholders',
    '--json',
  ];
  if (!commands) args.push('--no-commands');
  const child = spawnSync(process.execPath, args, {
    cwd: repoRoot,
    encoding: 'utf8',
  });
  fs.rmSync(tmp, { recursive: true, force: true });
  assert.equal(child.signal, null, child.stderr);
  return {
    child,
    report: JSON.parse(child.stdout),
  };
}

test('beta launch emits only provider-signed permissionless payout setup', () => {
  const { child, report } = runLaunch();
  assert.equal(child.status, 0, child.stderr);
  assert.equal(report.ok, true, report.errors?.join('\n'));

  const commands = report.commands;
  assert.equal(Object.hasOwn(commands, 'adminPayoutTxs'), false);
  assert.deepEqual(commands.allTxs, commands.adminSetupTxs);
  assert.deepEqual(commands.providerCommands, [
    ...commands.providerLifecycleCommands,
    ...commands.providerPayoutCommands,
  ]);
  assert.equal(
    commands.orderedCommands.at(-1).label,
    'provider payment-rail and payout commands',
  );

  const payoutText = commands.providerPayoutCommands.join('\n');
  assert.match(
    payoutText,
    /mayhem provider rails set .* --rails 'fiat,tap,tnk' --submit/,
  );
  assert.match(
    payoutText,
    /mayhem provider stripe onboard .* --country '<ISO-2-provider-country>' --no-open/,
  );
  assert.match(
    payoutText,
    /mayhem provider payout set .* --rail 'tap' --submit/,
  );
  assert.match(
    payoutText,
    /mayhem provider payout set .* --rail 'tnk' --submit/,
  );
  assert.doesNotMatch(JSON.stringify(commands), /set_provider_payout/);
  assert.doesNotMatch(JSON.stringify(commands), /admin provider payout/i);
  assert.doesNotMatch(payoutText, /^\/tx /m);
});

test('beta launch rejects the retired admin-approved payout manifest shape', () => {
  const manifest = structuredClone(template);
  const provider = manifest.seed_providers[0];
  delete provider.accepted_rails;
  delete provider.stripe_country;
  provider.payouts = {
    tnk: {
      admin_approved: true,
      addr: 'testtrac1retiredadminpayouttarget',
    },
  };
  delete manifest.controls.provider_payout_bindings_permissionless;
  delete manifest.controls.provider_payout_bindings_ownership_verified;
  manifest.controls.admin_sets_provider_payout_targets = true;
  manifest.controls.provider_payout_targets_admin_verified = true;

  const { child, report } = runLaunch(manifest, { commands: false });
  assert.equal(child.status, 1);
  const errors = report.errors.join('\n');
  assert.match(errors, /controls contains unsupported field\(s\): .*admin_sets_provider_payout_targets/);
  assert.match(errors, /seed_providers\[0\] contains unsupported field\(s\): payouts/);
  assert.match(errors, /seed_providers\[0\]\.accepted_rails must be an array/);
});

test('Stripe onboarding data is required only when the provider accepts fiat', () => {
  const missingCountry = structuredClone(template);
  delete missingCountry.seed_providers[0].stripe_country;
  const missing = runLaunch(missingCountry, { commands: false });
  assert.equal(missing.child.status, 1);
  assert.match(
    missing.report.errors.join('\n'),
    /seed_providers\[0\]\.stripe_country must be a non-empty string/,
  );

  const tapOnly = structuredClone(template);
  tapOnly.seed_providers[0].accepted_rails = ['tap'];
  delete tapOnly.seed_providers[0].stripe_country;
  const accepted = runLaunch(tapOnly);
  assert.equal(accepted.child.status, 0, accepted.report.errors?.join('\n'));
  const payoutText = accepted.report.commands.providerPayoutCommands.join('\n');
  assert.match(payoutText, /--rails 'tap' --submit/);
  assert.match(payoutText, /--rail 'tap' --submit/);
  assert.doesNotMatch(payoutText, /stripe onboard|--rail 'tnk'/);
});

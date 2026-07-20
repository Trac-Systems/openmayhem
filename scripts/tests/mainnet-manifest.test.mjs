import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';

import {
  buildCommands,
  validateMainnetManifest,
} from '../mainnet-manifest.mjs';

const template = JSON.parse(fs.readFileSync(
  new URL('../../config/beta/mainnet.template.json', import.meta.url),
  'utf8',
));

test('mainnet template matches current admin parameter schema', () => {
  const result = validateMainnetManifest(template, { allowPlaceholders: true });
  assert.deepEqual(result.errors, []);
});

test('mainnet manifest keeps atto-USD parameters as decimal strings', () => {
  const manifest = structuredClone(template);
  manifest.contract.params.probe_reward_au = 5_000_000_000_000_000;
  const result = validateMainnetManifest(manifest, { allowPlaceholders: true });
  assert.match(result.errors.join('\n'), /probe_reward_au must be a non-negative decimal string/);
});

test('mainnet manifest requires unique inference relay public keys', () => {
  const invalid = structuredClone(template);
  invalid.network.inference_relays = ['11'.repeat(32), '11'.repeat(32)];
  const duplicate = validateMainnetManifest(invalid, { allowPlaceholders: true });
  assert.match(duplicate.errors.join('\n'), /inference_relays must contain unique public keys/);

  invalid.network.inference_relays = ['not-a-public-key'];
  const malformed = validateMainnetManifest(invalid, { allowPlaceholders: true });
  assert.match(malformed.errors.join('\n'), /inference_relays\[0\] has invalid format/);
});

test('mainnet manifest rejects retired admin payout-target controls', () => {
  const manifest = structuredClone(template);
  delete manifest.controls.provider_payout_bindings_permissionless;
  delete manifest.controls.provider_payout_bindings_ownership_verified;
  manifest.controls.admin_sets_provider_payout_targets = true;
  manifest.controls.provider_payout_targets_admin_verified = true;

  const result = validateMainnetManifest(manifest, { allowPlaceholders: true });
  const errors = result.errors.join('\n');
  assert.match(errors, /unsupported field\(s\): .*admin_sets_provider_payout_targets/);
  assert.match(errors, /provider_payout_bindings_permissionless must be true/);
  assert.match(errors, /provider_payout_bindings_ownership_verified must be true/);
});

test('mainnet Pear command wires private auth files and the loopback Stripe worker', () => {
  const command = buildCommands(template).start_intercom.at(-1);
  assert.match(command, /--sc-bridge-token-file "\$MAYHEM_SC_BRIDGE_TOKEN_FILE"/);
  assert.match(
    command,
    /--paygate-internal-auth-secret-file "\$MAYHEM_PAYGATE_INTERNAL_AUTH_SECRET_FILE"/,
  );
  assert.match(command, /--stripe-worker-url http:\/\/127\.0\.0\.1:11436/);
  assert.doesNotMatch(command, /--sc-bridge-token /);
});

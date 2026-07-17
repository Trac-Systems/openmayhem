import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';

import { validateMainnetManifest } from '../mainnet-manifest.mjs';

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

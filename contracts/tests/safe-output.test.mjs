import assert from 'node:assert/strict';
import test from 'node:test';

import { redactSensitiveText, safeErrorMessage } from '../scripts/safe-output.mjs';

test('operational errors retain useful RPC hosts without leaking credentials or endpoint paths', () => {
  const output = safeErrorMessage(new Error(
    'request failed at https://user:password@rpc.example/private-key?token=secret',
  ));

  assert.match(output, /rpc\.example/);
  assert.doesNotMatch(output, /user|password|private-key|token=secret/);
});

test('operational output redacts payment keys and named secrets', () => {
  const syntheticStripeKey = ['pk', 'live', 'abcdefghijklmnopqrstuvwxyz'].join('_');
  const output = redactSensitiveText(
    `secret=very-secret ${syntheticStripeKey} bearer:opaque-value`,
  );

  assert.equal(output.includes('very-secret'), false);
  assert.equal(output.includes(syntheticStripeKey), false);
  assert.equal(output.includes('opaque-value'), false);
});

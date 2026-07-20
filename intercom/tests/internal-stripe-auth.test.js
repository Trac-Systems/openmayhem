import test from 'node:test';
import assert from 'node:assert/strict';
import b4a from 'b4a';

import { createInternalStripeAuthHeaders } from '../src/internal-stripe-auth.js';

test('internal Stripe auth uses Pear-compatible HMAC-SHA256', () => {
  const headers = createInternalStripeAuthHeaders({
    endpoint: new URL('http://127.0.0.1:11436/stripe/checkout'),
    body: '{"amount":1}',
    secret: '0123456789abcdef'.repeat(4),
    timestampSeconds: 1_700_000_000,
    nonceBytes: b4a.from(
      '000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f',
      'hex'
    ),
  });

  assert.deepEqual(headers, {
    'x-mayhem-paygate-timestamp': '1700000000',
    'x-mayhem-paygate-nonce':
      '000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f',
    'x-mayhem-paygate-signature':
      '31d67d5fde7b9540a6bace7724757453e58d2f31eef1a2e270cd70f5cb9c12b6',
  });
});

test('internal Stripe auth rejects malformed nonces', () => {
  assert.throws(
    () => createInternalStripeAuthHeaders({
      endpoint: new URL('http://127.0.0.1:11436/stripe/checkout'),
      body: '{}',
      secret: 'a'.repeat(64),
      timestampSeconds: 1_700_000_000,
      nonceBytes: b4a.alloc(31),
    }),
    /exactly 32 bytes/
  );
});

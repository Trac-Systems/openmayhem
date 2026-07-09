import assert from 'node:assert/strict';
import test from 'node:test';

import { MAINNET_MSB, proveMainnet, validateMainnetState } from '../mainnet-proof.mjs';

function status(overrides = {}) {
  return {
    msb: {
      ready: true,
      networkId: MAINNET_MSB.networkId,
      bootstrapHex: MAINNET_MSB.bootstrap,
      channel: MAINNET_MSB.channel,
      signedLength: 54_349,
      connectedValidators: 3,
      ...overrides,
    },
  };
}

test('mainnet proof requires both Ethereum and live official MSB evidence', () => {
  assert.equal(validateMainnetState(1, status()).ok, true);
  assert.match(validateMainnetState(5, status()).failures.join(' '), /chainId/);
  assert.match(validateMainnetState(1, status({ networkId: 919 })).failures.join(' '), /networkId/);
  assert.match(validateMainnetState(1, status({ channel: 'testnet' })).failures.join(' '), /channel/);
  assert.match(validateMainnetState(1, status({ connectedValidators: 0 })).failures.join(' '), /validator/);
});

test('mainnet proof returns only verified public network evidence', async () => {
  const fetchImpl = async (_url, options) => {
    if (options?.method === 'POST') {
      return { ok: true, json: async () => ({ jsonrpc: '2.0', result: '0x1' }) };
    }
    return { ok: true, json: async () => status() };
  };
  const report = await proveMainnet({
    ethRpc: 'https://rpc.invalid/key',
    peerRpc: 'http://127.0.0.1:49223/v1',
    timeoutSeconds: 1,
    pollMs: 1,
    fetchImpl,
  });
  assert.equal(report.ok, true);
  assert.deepEqual(report.ethereum, { chain_id: 1 });
  assert.equal(JSON.stringify(report).includes('rpc.invalid'), false);
});

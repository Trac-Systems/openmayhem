import assert from 'node:assert/strict';
import test from 'node:test';

import {
  MAINNET_MSB,
  proveMainnet,
  validateCanonicalPaymentsState,
  validateCanonicalReceiptFinalizationState,
  validateMainnetState,
} from '../mainnet-proof.mjs';

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

function payments(overrides = {}) {
  return {
    key: 'payments/current',
    confirmed: true,
    value: {
      denom: 'au_usd',
      rails: ['fiat', 'tap', 'tnk'],
      fiat: {
        processor: 'stripe',
        integration_currency: 'usd',
        adaptive_pricing: true,
        payout_currencies: ['eur', 'gbp', 'usd'],
        locale: 'en',
      },
      tap: {
        chain_id: 1,
        token_address: '0x5e7f6e008c6d9d7ad4c7eb75bd4ce62864cc7454',
        pool_address: '0xcfea9a256f1f96269d848cabf1ecb00fd2dd6a28',
      },
      tnk: {
        network: 'mainnet',
        treasury_address: 'trac1f3w8ja3qxcnmzzmxxt8m0ystdf683sy5arnhxvz0h7a8ydd0kqwq3lcgdh',
      },
      ver: 5,
      updated_at: 'a'.repeat(64),
      set_by: 'b'.repeat(64),
      set_by_role: 'admin',
      ...overrides,
    },
  };
}

function applyState(overrides = {}) {
  return {
    key: 'epoch/apply/state',
    confirmed: true,
    value: {
      updated_epoch: 7,
      pending_epoch: null,
      ...overrides,
    },
  };
}

function receiptIndex(epoch = 8, overrides = {}) {
  return {
    key: `receipt/epoch/${epoch}/index`,
    confirmed: true,
    value: {
      type: 'canonical_receipt_epoch_index',
      epoch,
      count: 2,
      page_size: 128,
      page_count: 1,
      revision: 3,
      updated_at: 'f'.repeat(64),
      ...overrides,
    },
  };
}

test('mainnet proof requires both Ethereum and live official MSB evidence', () => {
  assert.equal(validateMainnetState(1, status(), payments()).ok, true);
  assert.match(validateMainnetState(5, status(), payments()).failures.join(' '), /chainId/);
  assert.match(validateMainnetState(1, status({ networkId: 919 }), payments()).failures.join(' '), /networkId/);
  assert.match(validateMainnetState(1, status({ channel: 'testnet' }), payments()).failures.join(' '), /channel/);
  assert.match(validateMainnetState(1, status({ connectedValidators: 0 }), payments()).failures.join(' '), /validator/);
});

test('mainnet proof rejects the retired fiat currencies schema before service startup', () => {
  const legacy = payments();
  legacy.value.fiat = { processor: 'stripe', currencies: ['usd', 'eur'], locale: 'en' };
  const report = validateCanonicalPaymentsState(legacy);
  assert.equal(report.ok, false);
  assert.match(report.failures.join(' '), /fiat schema is incompatible/);
});

test('mainnet proof accepts null as the canonical empty next receipt epoch', () => {
  const report = validateCanonicalReceiptFinalizationState(
    applyState(),
    { key: 'receipt/epoch/8/index', confirmed: true, value: null },
  );
  assert.equal(report.ok, true);
  assert.deepEqual(report.index, {
    count: 0,
    page_count: 0,
    revision: 0,
    updated_at: null,
  });
});

test('mainnet proof binds a pending apply to the exact receipt index identity', () => {
  const index = receiptIndex(8);
  const accepted = validateCanonicalReceiptFinalizationState(
    applyState({
      pending_epoch: 8,
      pending_receipt_index_count: 2,
      pending_receipt_index_revision: 3,
      pending_receipt_index_page_count: 1,
      pending_receipt_index_updated_at: 'f'.repeat(64),
    }),
    index,
  );
  assert.equal(accepted.ok, true);
  const drifted = validateCanonicalReceiptFinalizationState(
    applyState({
      pending_epoch: 8,
      pending_receipt_index_count: 2,
      pending_receipt_index_revision: 2,
      pending_receipt_index_page_count: 1,
      pending_receipt_index_updated_at: 'f'.repeat(64),
    }),
    index,
  );
  assert.equal(drifted.ok, false);
  assert.match(drifted.failures.join(' '), /not bound/);
});

test('mainnet proof returns only verified public network evidence', async () => {
  const fetchImpl = async (url, options) => {
    if (options?.method === 'POST') {
      return { ok: true, json: async () => ({ jsonrpc: '2.0', result: '0x1' }) };
    }
    if (String(url).includes('state?key=payments%2Fcurrent')) {
      return { ok: true, json: async () => payments() };
    }
    if (String(url).includes('state?key=epoch%2Fapply%2Fstate')) {
      return { ok: true, json: async () => applyState() };
    }
    if (String(url).includes('state?key=receipt%2Fepoch%2F8%2Findex')) {
      return { ok: true, json: async () => receiptIndex() };
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

test('mainnet proof timeout zero keeps polling until healthy convergence', async () => {
  let statusCalls = 0;
  const fetchImpl = async (url, options) => {
    if (options?.method === 'POST') {
      return { ok: true, json: async () => ({ jsonrpc: '2.0', result: '0x1' }) };
    }
    if (String(url).includes('state?key=payments%2Fcurrent')) {
      return { ok: true, json: async () => payments() };
    }
    if (String(url).includes('state?key=epoch%2Fapply%2Fstate')) {
      return { ok: true, json: async () => applyState() };
    }
    if (String(url).includes('state?key=receipt%2Fepoch%2F8%2Findex')) {
      return { ok: true, json: async () => receiptIndex() };
    }
    statusCalls += 1;
    return {
      ok: true,
      json: async () => status(statusCalls === 1 ? { connectedValidators: 0 } : {}),
    };
  };

  const report = await proveMainnet({
    ethRpc: 'https://rpc.invalid/key',
    peerRpc: 'http://127.0.0.1:49223/v1',
    timeoutSeconds: 0,
    pollMs: 1,
    attemptTimeoutMs: 100,
    fetchImpl,
  });
  assert.equal(report.ok, true);
  assert.equal(statusCalls, 2);
});

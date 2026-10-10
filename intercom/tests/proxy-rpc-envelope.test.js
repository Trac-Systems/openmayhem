import test from 'node:test';
import assert from 'node:assert/strict';
import {
  discoverProxyCatalog, requestProxyAdmissionPolicy, requestProxyProviderState,
  requestProxyOperatorState, requestProxyIntentState, requestProxyOfferState,
  requestProxyQuoteState, requestProxyFinancialState, requestStripeCheckout,
} from '../src/rpc.js';

const cases = [
  [discoverProxyCatalog, 'discoverProxyCatalog', { query: { kind: 'families' } }],
  [requestProxyAdmissionPolicy, 'proxyAdmissionPolicy', { request_nonce: 'nonce' }],
  [requestProxyProviderState, 'proxyProviderState', { initial_operation_digest: 'op', provider_pubkey: 'provider', request_nonce: 'nonce' }],
  [requestProxyOperatorState, 'proxyOperatorState', { provider_pubkey: 'provider', request_nonce: 'nonce' }],
  [requestProxyIntentState, 'proxyIntentState', { intent: {}, request_nonce: 'nonce' }],
  [requestProxyOfferState, 'proxyOfferState', { offer: {}, rail: 'fiat', request_nonce: 'nonce', settlement_policy_hash: 'policy' }],
  [requestProxyQuoteState, 'proxyQuoteState', { billing_id: 'bill', offer: {}, rail: 'fiat', request_nonce: 'nonce', settlement_policy_hash: 'policy' }],
  [requestProxyFinancialState, 'proxyFinancialState', { accepted_terms: 'terms', request_nonce: 'nonce' }],
];
const payload = { ok: true, lane: 'proxy', request_nonce: 'nonce', proof: { signed_length: 1 },
  financial: { amount: '1000000', relayed: 'payload-field-must-stay' } };
const metadata = { relayed: true, request_id: 'a'.repeat(64) };
const peer = (method, result) => ({ protocol: { instance: { features: { mayhem: {
  [method]: async () => result,
} } } } });

test('all proxy RPC reads expose the same strict service payload for direct and relayed responses', async () => {
  for (const [read, method, query] of cases) {
    for (const relay of [false, true]) {
      const result = structuredClone({ ...payload, ...(relay ? metadata : {}) });
      const before = structuredClone(result);
      assert.deepEqual(await read(peer(method, result), query), payload, method);
      assert.deepEqual(result, before, 'do not mutate the relay response/cache');
    }
    for (const invalid of [
      { relayed: true }, { request_id: metadata.request_id },
      { ...metadata, relayed: false }, { ...metadata, relayed: 'true' },
      { ...metadata, request_id: 'not-a-request-id' },
    ]) {
      await assert.rejects(read(peer(method, { ...payload, ...invalid }), query), /relay metadata/);
    }
    const extra = { ...payload, ...metadata, unexpected: true };
    assert.equal((await read(peer(method, extra), query)).unexpected, true,
      'unknown service fields must reach strict clients and be rejected, not silently removed');
  }
});

test('native service relay responses retain their existing transport metadata', async () => {
  const result = { ok: true, ...metadata };
  assert.deepEqual(await requestStripeCheckout(peer('requestService', result), { payload: { who: 'owner' } }), result);
});

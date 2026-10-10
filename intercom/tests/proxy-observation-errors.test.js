import test from 'node:test';
import assert from 'node:assert/strict';
import b4a from 'b4a';
import MayhemFeature from '../features/mayhem/index.js';
import { readProxyOfferState } from '../features/mayhem/proxy-offer-state.js';
import { proxyReservationFixture } from './helpers/proxy-finance.js';

const h = n => n.toString(16).padStart(64, '0');

test('missing payment setup retains its real rejection through the provider observation boundary', async () => {
  for (const rail of ['fiat', 'tnk', 'tap']) {
    const f = await proxyReservationFixture(rail);
    const feature = Object.create(MayhemFeature.prototype);
    feature.peer = { ...f.peer, wallet: { publicKey: f.provider.publicKey,
      sign: bytes => b4a.toString(f.provider.wallet.sign(bytes), 'hex') } };
    feature._adminKey = async () => f.admin.publicKey;
    feature.requestService = async (_service, envelope) => {
      try {
        return await readProxyOfferState({ request: envelope.payload,
          withCanonicalSnapshot: body => body({ context: f.context,
            proof: { view_key: h(1), tree_hash: h(2), signed_length: 1, fork: 0 },
            read: f.read, assertCurrent: async () => {} }) });
      } catch (error) {
        // Authenticated relay failures do not contain a success snapshot.
        return { ok: false, accepted: false, message: error.message, relayed: true };
      }
    };
    const query = { request_nonce: h(33), offer: f.offer, rail,
      settlement_policy_hash: f.terms.settlement_policy_hash };
    const registrationKey = `prov/${f.provider.publicKey}`;
    await f.storage.del(registrationKey);
    await assert.rejects(feature.proxyOfferState(query), /provider payment registration is missing/);
    await f.storage.put(registrationKey, { status: 'active', accepted_rails: [] });
    await assert.rejects(feature.proxyOfferState(query), /provider payment rail is not active/);
    await f.storage.put(registrationKey, { status: 'active', accepted_rails: [rail] });
    await f.storage.del(`payout/current/${rail}/${f.provider.publicKey}`);
    await assert.rejects(feature.proxyOfferState(query), /payout pointer is missing/);
  }
});

test('failed proxy observations never expose arbitrary service exceptions or claim a broken network', async () => {
  const f = await proxyReservationFixture();
  const feature = Object.create(MayhemFeature.prototype);
  feature.peer = { ...f.peer, wallet: { publicKey: f.provider.publicKey,
    sign: bytes => b4a.toString(f.provider.wallet.sign(bytes), 'hex') } };
  feature._adminKey = async () => f.admin.publicKey;
  const query = { request_nonce: h(33), offer: f.offer, rail: 'tnk',
    settlement_policy_hash: f.terms.settlement_policy_hash };
  for (const value of [null, {}, { ok: false, message: '/private/path token=private' },
    { ok: false, message: 'Proxy offer state: payout pointer is missing.\nprivate' },
    { ok: false, message: 'x'.repeat(200000) }]) {
    feature.requestService = async () => value;
    await assert.rejects(feature.proxyOfferState(query), error => {
      assert.equal(error.message, 'Proxy offer state is unavailable; refresh the same observation.');
      return true;
    });
  }
  feature.requestService = async () => ({ ok: false, message: 'private exception' });
  await assert.rejects(feature.proxyOperatorState({ provider_pubkey: f.provider.publicKey,
    request_nonce: h(34) }), /Proxy operator state is unavailable/);
});

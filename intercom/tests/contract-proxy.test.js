import { proxyContractFixture as fixture } from './helpers/proxy.js';
import assert from 'node:assert/strict';
import test from 'node:test';
import b4a from 'b4a';
import { CONTRACT_VERSION } from '../contract/contract.js';
import { proxyPolicyFeatureKey } from '../contract/proxy-policy.js';
import { proxyRuntimeContext } from '../contract/proxy-context.js';
import { proxyRegistryKeys, readActiveProxyOffer } from '../contract/proxy-registry.js';
import { proxyMarketId, proxyAdmissionSigningBytes, proxyRegistryFeatureKey } from '../contract/proxy-protocol.js';
import { executeFeature } from './helpers/contract.js';

const clone = value => JSON.parse(JSON.stringify(value));
const hex = value => value.toString(16).padStart(64, '0');
const sign = (wallet, bytes) => b4a.toString(wallet.sign(bytes), 'hex');

for (const family of ['llm', 'decisions']) for (const rail of ['fiat', 'tnk', 'tap']) {
  test(`contract admits and prices a ${family} proxy through ${rail} fee without touching native balances`, async () => {
    const f = await fixture(family, rail);
    const first = await f.create();
    const admitted = await f.submit(first);
    assert.equal(admitted.ok, true);
    assert.equal(admitted.duplicate, false);
    assert.equal((await f.submit(first)).duplicate, true);
    const state = await f.read(proxyRegistryKeys.provider(f.provider.publicKey));
    assert.equal(state.entitlement.rail, rail);
    assert.equal((await f.submit(await f.envelope({ kind: 'set_offer', offer: f.offer }))).ok, true);
    const selected = await readActiveProxyOffer(f.offer, f.context, f.read);
    assert.deepEqual(selected.offer.accepted_rails, f.offer.accepted_rails);
    assert.deepEqual(await f.read('bal/existing-customer'), { fiat: '10', tnk: '20', tap: '30' });
    assert.deepEqual(await f.read('payout/epoch/542'), { status: 'prepared', native: true });
    assert.equal(await f.read('encl/' + f.offer.market_id), null);
  });
}

test('contract refuses unpaid, forged and foreign-writer registration without consuming entitlement', async () => {
  for (const kind of ['unpaid', 'forged', 'foreign_writer', 'wrong_ledger', 'invalid_first_action']) {
    const f = await fixture();
    let value = await f.create();
    if (kind === 'unpaid') value.admission = null;
    if (kind === 'forged') value.provider_signature = '0'.repeat(128);
    if (kind === 'wrong_ledger') f.peer.config.bootstrap = hex(888);
    if (kind === 'invalid_first_action') value = await f.envelope({ kind: 'create_market', market: f.market,
      membership: { ...f.membership, market_id: hex(888) } }, { admission: true });
    const result = await f.submit(value, kind === 'foreign_writer' ? f.provider.publicKey : f.admin.publicKey);
    assert.ok(result instanceof Error, kind);
    assert.equal(await f.read(proxyRegistryKeys.provider(f.provider.publicKey)), null, kind);
    assert.equal(await f.read('proxy/v1/admission-used/entitlement/' + f.permit.entitlement_id), null, kind);
  }
});

test('only canonical admin can configure or revoke proxy policy', async () => {
  const f = await fixture();
  const action = { kind: 'configure', config: { ...f.config, fee_policy_hash: hex(44) } };
  assert.ok(await f.policy(action, f.provider.publicKey) instanceof Error);
  assert.deepEqual(await f.read(proxyRegistryKeys.config), f.config);
  assert.equal((await f.policy(action)).ok, true);
});

test('an admin adds and renames a new family as data on the unchanged running contract', async () => {
  const f = await fixture();
  const family = 'new_family_2042';
  f.market.model.family_id = family;
  f.membership.market_id = await proxyMarketId(f.market);
  f.offer.market_id = f.membership.market_id;
  const registration = await f.create();
  assert.match((await f.submit(registration)).message, /unknown proxy model family/);
  assert.equal(await f.read(proxyRegistryKeys.provider(f.provider.publicKey)), null);
  assert.equal((await f.policy({ kind: 'set_family', family_id: family,
    label: 'New family', enabled: true })).ok, true);
  assert.equal((await f.submit(registration)).ok, true);
  assert.equal((await f.submit(await f.envelope({ kind: 'set_offer', offer: f.offer }))).ok, true);
  const before = await readActiveProxyOffer(f.offer, f.context, f.read);
  assert.equal((await f.policy({ kind: 'set_family', family_id: family,
    label: 'Updated label', enabled: true })).ok, true);
  assert.deepEqual(await readActiveProxyOffer(f.offer, f.context, f.read), before);
  assert.equal(f.context.contract_version, CONTRACT_VERSION);
});

test('policy retry after lost acknowledgement is idempotent and conflicting revisions are rejected', async () => {
  const f = await fixture();
  const head = await f.read('proxy/v1/policy-head');
  const value = { op: 'proxy_policy', context: f.network, revision: head.revision + 1,
    action: { kind: 'set_family', family_id: 'future_family', label: 'Future', enabled: true } };
  const apply = async envelope => executeFeature(f.contract, f.storage, 'mayhem_feature',
    await proxyPolicyFeatureKey(envelope), envelope, f.admin.publicKey);
  assert.equal((await apply(value)).ok, true);
  assert.equal((await apply(clone(value))).duplicate, true);
  const conflict = clone(value); conflict.action.label = 'Conflicting update';
  assert.match((await apply(conflict)).message, /out-of-order/);
  assert.equal((await f.read('proxy/v1/family/future_family')).label, 'Future');
});

test('policy hash meaning is immutable; disable and revocation affect new admission only', async () => {
  const f = await fixture();
  assert.equal((await f.submit(await f.create())).ok, true);
  assert.equal((await f.submit(await f.envelope({ kind: 'set_offer', offer: f.offer }))).ok, true);
  const accepted = await readActiveProxyOffer(f.offer, f.context, f.read);
  const changed = { kind: 'set_metering', policy_hash: f.market.metering.policy_hash, policy: { enabled: true, units: ['hidden_fee'] } };
  assert.match((await f.policy(changed)).message, /cannot change meaning/);
  assert.equal((await f.policy({ ...changed, policy: { enabled: false, units: f.market.metering.units } })).ok, true);
  await assert.rejects(readActiveProxyOffer(f.offer, f.context, f.read), /metering/);
  assert.deepEqual(accepted.offer, f.offer);
  assert.equal((await f.submit(await f.envelope({ kind: 'leave_market', market_id: f.offer.market_id, revision: 2 }))).ok, true);
});

test('paid permit reissue fence invalidates old issuance without consuming or charging the fee', async () => {
  const f = await fixture();
  const old = await f.create();
  const renewed = clone(old);
  renewed.admission.permit.issuance_revision = 2;
  renewed.admission.permit.nonce = hex(765);
  renewed.admission.issuer_signature = sign(f.issuer.wallet, proxyAdmissionSigningBytes(renewed.admission.permit));
  const { proxyAdmissionDigest } = await import('../contract/proxy-protocol.js');
  assert.equal((await f.policy({ kind: 'set_admission_generation', entitlement_id: f.permit.entitlement_id,
    issuance_revision: 2, permit_digest: await proxyAdmissionDigest(renewed.admission.permit) })).ok, true);
  assert.match((await f.submit(old)).message, /superseded/);
  assert.equal((await f.submit(renewed)).ok, true);
  assert.match((await f.policy({ kind: 'set_admission_generation', entitlement_id: f.permit.entitlement_id,
    issuance_revision: 3, permit_digest: hex(66) })).message, /already consumed/);
});

test('contract propagates storage failure instead of reporting a terminal proxy rejection', async () => {
  const f = await fixture();
  const get = f.storage.get.bind(f.storage);
  f.storage.get = async key => {
    if (key === proxyRegistryKeys.config) throw new Error('injected read failure');
    return get(key);
  };
  await assert.rejects(f.submit(await f.create()), /injected read failure/);
  f.storage.get = get;
  assert.equal(await f.read(proxyRegistryKeys.provider(f.provider.publicKey)), null);
  assert.equal((await f.submit(await f.create())).ok, true);
});

test('proxy runtime context derives both bootstrap keys and rejects absent configuration', () => {
  const peer = { msbClient: { networkId: 919, bootstrapHex: hex(1) }, base: { key: b4a.from(hex(2), 'hex') } };
  assert.deepEqual(proxyRuntimeContext(peer, 30, 100), { network_id: '919', msb_bootstrap: hex(1), subnet_bootstrap: hex(2), contract_version: 30, epoch: 100 });
  assert.throws(() => proxyRuntimeContext({}, 30, 100), /unavailable/);
});

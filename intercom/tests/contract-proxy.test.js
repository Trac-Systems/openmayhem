import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import b4a from 'b4a';
import MayhemContract, { CONTRACT_VERSION } from '../contract/contract.js';
import { proxyPolicyFeatureKey } from '../contract/proxy-policy.js';
import { proxyRuntimeContext } from '../contract/proxy-context.js';
import { proxyRegistryKeys, readActiveProxyOffer } from '../contract/proxy-registry.js';
import { proxyMarketId, proxyOperationDigest, proxyOperationSigningBytes, proxyAdmissionSigningBytes, proxyRegistryFeatureKey } from '../contract/proxy-protocol.js';
import { MemoryStorage, makeIdentity, makeVerifier, executeFeature } from './helpers/contract.js';

const wire = JSON.parse(fs.readFileSync(new URL('../../crates/mayhem-proto/tests/fixtures/proxy-wire-v1.json', import.meta.url)));
const clone = value => JSON.parse(JSON.stringify(value));
const hex = value => value.toString(16).padStart(64, '0');
const sign = (wallet, bytes) => b4a.toString(wallet.sign(bytes), 'hex');

async function fixture(family = 'llm', rail = 'fiat') {
  const admin = await makeIdentity();
  const provider = await makeIdentity();
  const issuer = await makeIdentity();
  const peer = { config: { bootstrap: hex(5) }, msbClient: { networkId: 918, bootstrapHex: hex(4) }, wallet: makeVerifier(admin.wallet) };
  const contract = new MayhemContract({ peer }, {});
  const storage = new MemoryStorage({ admin: admin.publicKey, 'epoch/apply/state': { epoch: 100 },
    'payout/epoch/542': { status: 'prepared', native: true }, 'bal/existing-customer': { fiat: '10', tnk: '20', tap: '30' } });
  const context = proxyRuntimeContext(peer, CONTRACT_VERSION, 100);
  const { epoch, ...network } = context;
  const row = clone(wire.cases.find(c => c.name === family));
  row.market.creator_pubkey = provider.publicKey;
  row.membership.provider_pubkey = provider.publicKey;
  row.membership.market_id = await proxyMarketId(row.market);
  row.offer.provider_pubkey = provider.publicKey;
  row.offer.market_id = row.membership.market_id;
  const read = async key => (await storage.get(key))?.value ?? null;
  let policyRevision = 0;
  const policy = async (action, sender = admin.publicKey) => {
    const value = { op: 'proxy_policy', context: network, revision: policyRevision + 1, action };
    const result = await executeFeature(contract, storage, 'mayhem_feature', await proxyPolicyFeatureKey(value), value, sender);
    if (!(result instanceof Error)) policyRevision++;
    return result;
  };
  const config = { ...network, enabled: true, fee_policy_hash: row.permit.fee_policy_hash, active_issuers: [issuer.publicKey],
    max_permit_epochs: 20, max_mutations_per_provider_epoch: 50, max_mutations_per_epoch: 200,
    max_active_memberships: 4, max_created_markets_per_provider_epoch: 4, max_offer_slots_per_membership: 8 };
  for (const action of [
    { kind: 'configure', config },
    { kind: 'set_family', family_id: 'other', label: 'Other / Undisclosed', enabled: true },
    { kind: 'set_metering', policy_hash: row.market.metering.policy_hash, policy: { enabled: true, units: row.market.metering.units } },
    ...row.market.endpoints.map(endpoint => ({ kind: 'set_endpoint', contract_hash: endpoint.contract_hash, policy: {
      enabled: true, endpoint: endpoint.endpoint, family: row.market.family, max_context: 262144,
      ctx_brackets: ['le128k', 'le256k', 'le32k', 'le8k'], outcome_classes: [...new Set(['', row.offer.outcome_class])].sort(),
    } })),
  ]) assert.equal((await policy(action)).ok, true);

  const envelope = async (action, { admission = false } = {}) => {
    const state = await read(proxyRegistryKeys.provider(provider.publicKey));
    const intent = { schema_version: 1, lane: 'proxy', ...network, provider_pubkey: provider.publicKey,
      sequence: (state?.sequence ?? 0) + 1, action };
    const result = { op: 'proxy_registry', intent, provider_signature: sign(provider.wallet, proxyOperationSigningBytes(intent)), admission: null };
    if (admission) {
      const permit = { ...row.permit, ...network, provider_pubkey: provider.publicKey, issuer_pubkey: issuer.publicKey,
        initial_operation_digest: await proxyOperationDigest(intent), rail };
      result.admission = { permit, issuer_signature: sign(issuer.wallet, proxyAdmissionSigningBytes(permit)) };
    }
    return result;
  };
  const submit = async (value, sender = admin.publicKey) => executeFeature(contract, storage, 'mayhem_feature', await proxyRegistryFeatureKey(value), value, sender);
  const create = async () => envelope({ kind: 'create_market', market: row.market, membership: row.membership }, { admission: true });
  return { ...row, admin, provider, issuer, peer, contract, storage, context, network, read, policy, config, envelope, submit, create };
}

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

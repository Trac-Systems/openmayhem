import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import b4a from 'b4a';
import { makeIdentity, makeVerifier, MemoryStorage } from './helpers/contract.js';
import {
  proxyMarketId, proxyOperationDigest, proxyOperationSigningBytes, validateProxyOperation,
  proxyAdmissionSigningBytes, proxyAdmissionDigest,
  proxyOfferSlotId,
} from '../contract/proxy-protocol.js';
import { prepareProxyRegistryMutation, readActiveProxyOffer, PROXY_PREFIX, proxyRegistryKeys as keys } from '../contract/proxy-registry.js';
import { admitProxyRegistryFeature, proxyRegistryFeatureKey } from '../features/mayhem/proxy-admission.js';

const wire = JSON.parse(fs.readFileSync(new URL('../../crates/mayhem-proto/tests/fixtures/proxy-wire-v1.json', import.meta.url)));
const operations = JSON.parse(fs.readFileSync(new URL('../../crates/mayhem-proto/tests/fixtures/proxy-operations-v1.json', import.meta.url)));
const copy = value => JSON.parse(JSON.stringify(value));
const hex = n => n.toString(16).padStart(64, '0');
const prefix = path => PROXY_PREFIX + path;
const sign = (wallet, bytes) => b4a.toString(wallet.sign(bytes), 'hex');

for (const row of operations.cases) {
  test(`proxy operation wire parity: ${row.name}`, async () => {
    assert.equal(await proxyOperationDigest(row.intent), row.digest);
    assert.equal(proxyOperationSigningBytes(row.intent).toString('utf8'), row.signing_utf8);
  });
}
for (const bad of operations.invalid) {
  test(`proxy operation rejects ${bad.name}`, () => {
    const intent = copy(operations.cases.find(r => r.name === bad.base).intent);
    const path = bad.path.split('/').slice(1);
    let target = intent;
    for (const part of path.slice(0, -1)) target = target[part];
    target[path.at(-1)] = bad.value;
    assert.throws(() => validateProxyOperation(intent));
  });
}

async function fixture(family = 'llm', rail = 'fiat') {
  const provider = await makeIdentity();
  const issuer = await makeIdentity();
  const verifier = makeVerifier(provider.wallet);
  const row = copy(wire.cases.find(c => c.name === family));
  const context = { network_id: row.permit.network_id, msb_bootstrap: row.permit.msb_bootstrap,
    subnet_bootstrap: row.permit.subnet_bootstrap, contract_version: row.permit.contract_version, epoch: 100 };
  row.market.creator_pubkey = provider.publicKey;
  row.membership.market_id = await proxyMarketId(row.market);
  row.membership.provider_pubkey = provider.publicKey;
  row.offer.market_id = row.membership.market_id;
  row.offer.provider_pubkey = provider.publicKey;
  const config = { ...context, enabled: true, fee_policy_hash: row.permit.fee_policy_hash, active_issuers: [issuer.publicKey],
    max_permit_epochs: 20, max_mutations_per_provider_epoch: 100, max_mutations_per_epoch: 200,
    max_active_memberships: 2, max_created_markets_per_provider_epoch: 2, max_offer_slots_per_membership: 4 };
  delete config.epoch;
  const storage = new MemoryStorage({
    [keys.config]: config,
    [prefix('family/other')]: { enabled: true },
    [prefix(`metering-policy/${row.market.metering.policy_hash}`)]: { enabled: true, units: row.market.metering.units },
    // Sentinel data: this module must never touch a native or money namespace.
    'earn/fiat/existing': { pending: '101', paid: '21' },
    'earn/tnk/existing': { pending: '202', paid: '22' },
    'earn/tap/existing': { pending: '303', paid: '23' },
    'payout/plan/542': { immutable: 'pre-existing payout' },
    'bal/customer': { available: '404', reserved: '24' },
    'enclave/native': { native: true },
  });
  for (const endpoint of row.market.endpoints) {
    await storage.put(prefix(`endpoint-policy/${endpoint.contract_hash}`), {
      enabled: true, endpoint: endpoint.endpoint, family: row.market.family, max_context: 262144,
      ctx_brackets: ['le8k', 'le32k', 'le128k', 'le256k'], outcome_classes: ['', row.offer.outcome_class],
    });
  }
  let reads = 0;
  const read = async path => { reads++; return (await storage.get(path))?.value ?? null; };
  const prepare = envelope => prepareProxyRegistryMutation(envelope, context, read, verifier.verify);
  const apply = async envelope => {
    const mutation = await prepare(envelope);
    for (const change of mutation.writes) {
      assert.ok(change.key.startsWith(PROXY_PREFIX));
      if (change.value === null) await storage.del(change.key);
      else await storage.put(change.key, change.value);
    }
    return mutation;
  };
  const envelope = async (action, options = {}) => {
    const signer = options.provider ?? provider;
    const state = await read(keys.provider(signer.publicKey));
    const intent = { schema_version: 1, lane: 'proxy', network_id: context.network_id,
      msb_bootstrap: context.msb_bootstrap, subnet_bootstrap: context.subnet_bootstrap, contract_version: context.contract_version,
      provider_pubkey: signer.publicKey, sequence: options.sequence ?? (state?.sequence ?? 0) + 1, action: copy(action) };
    const result = { op: 'proxy_registry', intent, provider_signature: sign(signer.wallet, proxyOperationSigningBytes(intent)), admission: null };
    if (!state && options.admission !== false) {
      const permit = { ...row.permit, provider_pubkey: signer.publicKey, issuer_pubkey: issuer.publicKey, rail,
        initial_operation_digest: await proxyOperationDigest(intent), ...(options.permit ?? {}) };
      result.admission = { permit, issuer_signature: sign(issuer.wallet, proxyAdmissionSigningBytes(permit)) };
    }
    return result;
  };
  const create = () => envelope({ kind: 'create_market', market: row.market, membership: row.membership });
  return { ...row, provider, issuer, context, config, storage, read, prepare, apply, envelope, create, reads: () => reads };
}

for (const rail of ['fiat', 'tnk', 'tap']) {
  for (const family of ['llm', 'decisions']) {
    test(`fee admission plan and full registry lifecycle: ${family}/${rail}`, async () => {
      const f = await fixture(family, rail);
      const before = await f.create();
      const snapshot = f.storage.snapshotBytes();
      const checked = await f.prepare(before);
      assert.equal(f.storage.snapshotBytes(), snapshot, 'validation mutated storage');
      assert.ok(checked.writes.length > 0);
      await f.apply(before);
      const admitted = f.storage.snapshotBytes();
      const duplicate = await f.apply(before);
      assert.equal(duplicate.duplicate, true);
      assert.deepEqual(duplicate.writes, []);
      assert.equal(f.storage.snapshotBytes(), admitted);
      assert.equal((await f.read(keys.provider(f.provider.publicKey))).entitlement.rail, rail);

      await f.apply(await f.envelope({ kind: 'set_offer', offer: f.offer }));
      const offerKey = await keys.offer(f.offer.market_id, f.provider.publicKey, f.offer.endpoint, f.offer.ctx_bracket, f.offer.outcome_class);
      const accepted = copy(await f.read(offerKey));
      const repriced = { ...f.offer, revision: 2, per_request_au: '100' };
      await f.apply(await f.envelope({ kind: 'set_offer', offer: repriced }));
      assert.deepEqual(accepted.offer, f.offer, 'accepted snapshot repriced');
      assert.notEqual((await f.read(offerKey)).digest, accepted.digest);
      await f.apply(await f.envelope({ kind: 'withdraw_offer', market_id: f.offer.market_id, endpoint: f.offer.endpoint,
        ctx_bracket: f.offer.ctx_bracket, outcome_class: f.offer.outcome_class, revision: 3 }));
      await assert.rejects(f.apply(await f.envelope({ kind: 'set_offer', offer: repriced })), /stale proxy offer/);
      await f.apply(await f.envelope({ kind: 'leave_market', market_id: f.offer.market_id, revision: 2 }));
      assert.equal((await f.read(keys.provider(f.provider.publicKey))).active_memberships, 0);
      await assert.rejects(f.apply(await f.envelope({ kind: 'join_market', membership: f.membership })), /stale proxy membership/);
      const member = { ...f.membership, revision: 3 };
      await f.apply(await f.envelope({ kind: 'join_market', membership: member }));
      await assert.rejects(f.apply(await f.envelope({ kind: 'set_offer', offer: { ...f.offer, revision: 4 } })), /membership binding/);
      await f.apply(await f.envelope({ kind: 'set_offer', offer: { ...f.offer, revision: 4, membership_revision: 3 } }));
      assert.equal((await f.read(offerKey)).revision, 4);
      const native = entries => JSON.stringify(entries.filter(([key]) => !key.startsWith(PROXY_PREFIX)));
      assert.equal(native(JSON.parse(snapshot)), native(JSON.parse(f.storage.snapshotBytes())));
    });
  }
}

test('invalid first action cannot consume payment or mutate native state', async () => {
  const f = await fixture();
  const envelope = await f.envelope({ kind: 'create_market', market: f.market, membership: { ...f.membership, market_id: hex(88) } });
  const snapshot = f.storage.snapshotBytes();
  await assert.rejects(f.apply(envelope), /membership market mismatch/);
  assert.equal(f.storage.snapshotBytes(), snapshot);
  await f.apply(await f.create());
});

test('unpaid, forged, expired, revoked and superseded admissions fail without a write', async () => {
  for (const type of ['unpaid', 'provider_sig', 'issuer_sig', 'expired', 'revoked', 'generation', 'fee_policy', 'issuer_rotation']) {
    const f = await fixture();
    const envelope = await f.create();
    if (type === 'unpaid') envelope.admission = null;
    if (type === 'provider_sig') envelope.provider_signature = '0'.repeat(128);
    if (type === 'issuer_sig') envelope.admission.issuer_signature = '0'.repeat(128);
    if (type === 'expired') f.context.epoch = envelope.admission.permit.expires_after_epoch + 1;
    if (type === 'revoked') await f.storage.put(prefix(`admission-revoked/${envelope.admission.permit.entitlement_id}`), true);
    if (type === 'generation') await f.storage.put(prefix(`admission-generation/${envelope.admission.permit.entitlement_id}`), { revision: 2, permit_digest: hex(9) });
    if (type === 'fee_policy') await f.storage.put(keys.config, { ...f.config, fee_policy_hash: hex(23) });
    if (type === 'issuer_rotation') await f.storage.put(keys.config, { ...f.config, active_issuers: [hex(23)] });
    const snapshot = f.storage.snapshotBytes();
    await assert.rejects(f.apply(envelope), undefined, type);
    assert.equal(f.storage.snapshotBytes(), snapshot, type);
  }
});

test('each invoice, evidence or entitlement cannot admit a second identity', async () => {
  for (const field of ['invoice_commitment', 'evidence_commitment', 'entitlement_id']) {
    const f = await fixture();
    const first = await f.create();
    await f.apply(first);
    const second = await makeIdentity();
    const member = { ...f.membership, provider_pubkey: second.publicKey };
    const permit = { invoice_commitment: hex(81), evidence_commitment: hex(82), entitlement_id: hex(83),
      [field]: first.admission.permit[field] };
    const request = await f.envelope({ kind: 'join_market', membership: member }, { provider: second, permit });
    const snapshot = f.storage.snapshotBytes();
    await assert.rejects(f.apply(request), /already consumed/);
    assert.equal(f.storage.snapshotBytes(), snapshot);
  }
});

test('independent admitted provider joins the same market with its own offer', async () => {
  const f = await fixture();
  await f.apply(await f.create());
  await f.apply(await f.envelope({ kind: 'set_offer', offer: f.offer }));
  const second = await makeIdentity();
  const member = { ...f.membership, provider_pubkey: second.publicKey, recipe_hash: hex(65) };
  await f.apply(await f.envelope({ kind: 'join_market', membership: member }, { provider: second,
    permit: { invoice_commitment: hex(71), evidence_commitment: hex(72), entitlement_id: hex(73) } }));
  const offer = { ...f.offer, provider_pubkey: second.publicKey, per_request_au: '123' };
  await f.apply(await f.envelope({ kind: 'set_offer', offer }, { provider: second }));
  assert.equal((await f.read(await keys.offer(offer.market_id, second.publicKey, offer.endpoint, offer.ctx_bracket, offer.outcome_class))).offer.per_request_au, '123');
  await assert.rejects(f.envelope({ kind: 'set_offer', offer }), /signer mismatch/);
});

test('strict sequence and offer revisions survive checkpoint restore', async () => {
  const f = await fixture();
  const first = await f.create();
  await f.apply(first);
  const second = await f.envelope({ kind: 'set_offer', offer: f.offer });
  await f.apply(second);
  const restored = MemoryStorage.fromSnapshotBytes(f.storage.snapshotBytes());
  const read = async path => (await restored.get(path))?.value ?? null;
  const verify = makeVerifier(f.provider.wallet).verify;
  const result = await prepareProxyRegistryMutation(second, f.context, read, verify);
  assert.equal(result.duplicate, true);
  await assert.rejects(prepareProxyRegistryMutation(first, f.context, read, verify), /out-of-order/);
  const gap = await f.envelope({ kind: 'set_offer', offer: { ...f.offer, revision: 2 } }, { sequence: 4 });
  await assert.rejects(prepareProxyRegistryMutation(gap, f.context, read, verify), /out-of-order/);
});

test('policy/quotas fail closed with constant work and no historical prefix reads', async () => {
  for (const type of ['disabled', 'unknown_endpoint', 'unknown_meter', 'unknown_family', 'context', 'quota']) {
    const f = await fixture();
    if (type === 'disabled') await f.storage.put(keys.config, { ...f.config, enabled: false });
    if (type === 'unknown_endpoint') await f.storage.del(prefix(`endpoint-policy/${f.market.endpoints[0].contract_hash}`));
    if (type === 'unknown_meter') await f.storage.del(prefix(`metering-policy/${f.market.metering.policy_hash}`));
    if (type === 'unknown_family') await f.storage.del(prefix('family/other'));
    if (type === 'context') f.membership.served_context = 262145;
    if (type === 'quota') await f.storage.put(prefix('mutation-budget'), { epoch: 100, mutations: f.config.max_mutations_per_epoch });
    const snapshot = f.storage.snapshotBytes();
    const envelope = await f.create();
    await assert.rejects(f.apply(envelope), undefined, type);
    assert.equal(f.storage.snapshotBytes(), snapshot);
    assert.ok(f.reads() < 32, `${type} read count ${f.reads()}`);
  }
});

test('revocation blocks new offers but preserves withdrawal and old acceptance data', async () => {
  const f = await fixture();
  await f.apply(await f.create());
  await f.apply(await f.envelope({ kind: 'set_offer', offer: f.offer }));
  await f.storage.put(prefix(`provider-revoked/${f.provider.publicKey}`), true);
  await assert.rejects(f.apply(await f.envelope({ kind: 'set_offer', offer: { ...f.offer, revision: 2 } })), /revoked/);
  await f.apply(await f.envelope({ kind: 'leave_market', market_id: f.membership.market_id, revision: 2 }));
  assert.equal((await f.read(await keys.offer(f.offer.market_id, f.provider.publicKey, f.offer.endpoint, f.offer.ctx_bracket, f.offer.outcome_class))).offer.revision, 1);
});

test('new offer selection invalidates membership changes immediately without scanning old offers', async () => {
  const f = await fixture();
  await f.apply(await f.create());
  await f.apply(await f.envelope({ kind: 'set_offer', offer: f.offer }));
  const select = () => readActiveProxyOffer(f.offer, f.context, f.read);
  const before = f.reads();
  const accepted = await select();
  assert.ok(f.reads() - before < 16);
  assert.deepEqual(accepted.offer, f.offer);
  const member = { ...f.membership, revision: 2, max_concurrency: 1 };
  await f.apply(await f.envelope({ kind: 'update_membership', membership: member }));
  assert.equal(await select(), null);
  await f.apply(await f.envelope({ kind: 'set_offer', offer: { ...f.offer, revision: 2, membership_revision: 2 } }));
  assert.equal((await select()).membership.max_concurrency, 1);
  await f.storage.put(prefix(`admission-revoked/${(await f.read(keys.provider(f.provider.publicKey))).entitlement.id}`), true);
  assert.equal(await select(), null);
  assert.deepEqual(accepted.offer, f.offer, 'new-admission revocation changed accepted terms');
});

test('restart and expired permit never charge an admitted provider a second fee', async () => {
  const f = await fixture();
  const first = await f.create();
  await f.apply(first);
  f.context.epoch = first.admission.permit.expires_after_epoch + 100;
  await f.apply(await f.envelope({ kind: 'set_offer', offer: f.offer }));
  const offer = await f.envelope({ kind: 'set_offer', offer: { ...f.offer, revision: 2 } });
  offer.admission = first.admission;
  await assert.rejects(f.apply(offer), /no second admission/);
});

test('same sequence with changed signed contents cannot overwrite accepted state', async () => {
  const f = await fixture();
  await f.apply(await f.create());
  await f.apply(await f.envelope({ kind: 'set_offer', offer: f.offer }));
  const conflicting = await f.envelope({ kind: 'set_offer', offer: { ...f.offer, revision: 2, per_request_au: '999' } }, { sequence: 2 });
  const before = f.storage.snapshotBytes();
  await assert.rejects(f.apply(conflicting), /out-of-order/);
  assert.equal(f.storage.snapshotBytes(), before);
});

test('revoked issuer and policy changes do not revoke an existing paid entitlement', async () => {
  const f = await fixture();
  await f.apply(await f.create());
  await f.storage.put(keys.config, { ...f.config, active_issuers: [hex(765)], fee_policy_hash: hex(766) });
  await f.apply(await f.envelope({ kind: 'set_offer', offer: f.offer }));
  assert.ok(await readActiveProxyOffer(f.offer, f.context, f.read));
});

test('pre-forwarding gate never forwards unpaid, forged or incorrectly keyed proxy operations', async () => {
  for (const type of ['unpaid', 'signature', 'wrong_key', 'wrong_fork', 'stale_during_validation', 'configuration_missing']) {
    const f = await fixture();
    const envelope = await f.create();
    let forwarded = 0;
    let checks = 0;
    let closed = 0;
    let featureKey = await proxyRegistryFeatureKey(envelope);
    if (type === 'unpaid') envelope.admission = null;
    if (type === 'signature') envelope.provider_signature = '0'.repeat(128);
    if (type === 'wrong_key') featureKey = 'provider/register/alias';
    const snapshot = {
      context: f.context, read: f.read,
      assertCurrent: async () => {
        checks++;
        if (type === 'wrong_fork' || (type === 'stale_during_validation' && checks === 2)) throw new Error('canonical view unavailable');
      },
    };
    const withCanonicalSnapshot = type === 'configuration_missing' ? null : async body => {
      try { return await body(snapshot); } finally { closed++; }
    };
    const before = f.storage.snapshotBytes();
    await assert.rejects(admitProxyRegistryFeature({ featureKey, envelope, withCanonicalSnapshot,
      verifySignature: makeVerifier(f.provider.wallet).verify, forward: async () => { forwarded++; } }), undefined, type);
    assert.equal(forwarded, 0, type);
    assert.equal(f.storage.snapshotBytes(), before, type);
    if (checks > 0) assert.equal(closed, 1, type);
  }
});

test('pre-forwarding gate returns exact applied retry without another append', async () => {
  const f = await fixture();
  const envelope = await f.create();
  const featureKey = await proxyRegistryFeatureKey(envelope);
  let forwarded = 0;
  const withCanonicalSnapshot = async body => {
    const storage = MemoryStorage.fromSnapshotBytes(f.storage.snapshotBytes());
    return await body({ context: copy(f.context), read: async key => (await storage.get(key))?.value ?? null, assertCurrent: async () => {} });
  };
  const send = () => admitProxyRegistryFeature({ featureKey, envelope, withCanonicalSnapshot,
    verifySignature: makeVerifier(f.provider.wallet).verify, forward: async request => {
      forwarded++;
      assert.deepEqual(Object.keys(request).sort(), ['envelope', 'featureKey', 'fences']);
      assert.deepEqual(Object.keys(request.fences).sort(), ['reads', 'writes']);
      assert.ok(request.fences.reads.every(key => typeof key === 'string' && key.startsWith('proxy/v1/')));
      assert.ok(request.fences.writes.every(key => typeof key === 'string' && key.startsWith('proxy/v1/')));
      return await f.apply(request.envelope);
    } });
  assert.equal((await send()).duplicate, false);
  assert.equal((await send()).duplicate, true);
  assert.equal(forwarded, 1);
});

test('caller mutation during asynchronous validation cannot change a verified operation', async () => {
  const f = await fixture();
  const envelope = await f.create();
  const expected = await proxyOperationDigest(envelope.intent);
  let calls = 0;
  const verify = makeVerifier(f.provider.wallet).verify;
  const delayedVerifier = async (...args) => {
    if (++calls === 1) {
      envelope.intent.action.market.slug = 'changed-after-signature';
      envelope.admission.permit.entitlement_id = hex(9999);
    }
    return verify(...args);
  };
  const plan = await prepareProxyRegistryMutation(envelope, f.context, f.read, delayedVerifier);
  assert.equal(plan.result.operation_digest, expected);
  assert.equal(plan.writes.find(w => w.key === keys.market(f.membership.market_id)).value.slug, f.market.slug);
});

test('same network label and authority cannot replay an operation onto a different ledger', async () => {
  for (const field of ['msb_bootstrap', 'subnet_bootstrap']) {
    const f = await fixture();
    const envelope = await f.create();
    f.context[field] = hex(999);
    await f.storage.put(keys.config, { ...f.config, [field]: f.context[field] });
    const before = f.storage.snapshotBytes();
    await assert.rejects(f.apply(envelope), /network\/contract mismatch/);
    assert.equal(f.storage.snapshotBytes(), before);
  }
});

test('offer slots share cross-language identity and obey the state key limit at maximal input lengths', async () => {
  for (const row of wire.cases) {
    const { endpoint, ctx_bracket, outcome_class } = row.offer;
    assert.equal(await proxyOfferSlotId({ endpoint, ctx_bracket, outcome_class }), row.digests.offer_slot);
  }
  const maximum = await keys.offer(hex(1), hex(2), 'mayhem_decisions', 'a'.repeat(64), 'f'.repeat(64));
  assert.ok(maximum.length <= 256);
  assert.notEqual(maximum, await keys.offer(hex(1), hex(2), 'mayhem_decisions', 'a'.repeat(63), 'f'.repeat(64)));
});

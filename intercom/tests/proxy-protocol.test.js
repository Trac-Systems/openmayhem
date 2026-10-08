import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import {
  validateProxyMarket, validateProxyMembership, validateProxyOffer,
  validateProxyMembershipForMarket, validateProxyOfferForMembership,
  proxyMarketSigningBytes, proxyMembershipSigningBytes, proxyOfferSigningBytes,
  proxyMarketId, proxyMembershipDigest, proxyOfferDigest, proxyOfferCost,
  validateProxyOfferRevision,
  validateProxyAdmissionPermit, proxyAdmissionSigningBytes, proxyAdmissionDigest, verifyProxyAdmissionPermit,
} from '../contract/proxy-protocol.js';
import b4a from 'b4a';
import { makeIdentity, makeVerifier } from './helpers/contract.js';

const fixture = JSON.parse(fs.readFileSync(new URL('../../crates/mayhem-proto/tests/fixtures/proxy-wire-v1.json', import.meta.url)));
const clone = value => JSON.parse(JSON.stringify(value));
const validation = {market: validateProxyMarket, membership: validateProxyMembership, offer: validateProxyOffer, permit: validateProxyAdmissionPermit};
const signing = {market: proxyMarketSigningBytes, membership: proxyMembershipSigningBytes, offer: proxyOfferSigningBytes, permit: proxyAdmissionSigningBytes};
const digest = {market: proxyMarketId, membership: proxyMembershipDigest, offer: proxyOfferDigest, permit: proxyAdmissionDigest};

for (const row of fixture.cases) {
  test(`proxy canonical bytes and BLAKE3 match Rust: ${row.name}`, async () => {
    await validateProxyOfferForMembership(row.offer, row.market, row.membership);
    for (const kind of ['market', 'membership', 'offer', 'permit']) {
      assert.equal(signing[kind](row[kind]).toString('utf8'), row.signing_utf8[kind]);
      assert.equal(await digest[kind](row[kind]), row.digests[kind]);
      // Reordering JSON object keys must not change the signed meaning.
      const reordered = Object.fromEntries(Object.entries(row[kind]).reverse());
      assert.equal(await digest[kind](reordered), row.digests[kind]);
    }
  });
}

for (const change of fixture.invalid) {
  test(`proxy rejects ${change.name}`, async () => {
    const row = clone(fixture.cases.find(r => r.name === change.base));
    const path = change.path.split('/').slice(1);
    let target = row[change.record];
    for (const key of path.slice(0, -1)) target = target[key];
    if (change.op === 'remove') delete target[path.at(-1)];
    else target[path.at(-1)] = change.value;
    if (!change.binding) assert.throws(() => validation[change.record](row[change.record]));
    else if (change.record === 'membership') await assert.rejects(validateProxyMembershipForMarket(row.membership, row.market));
    else await assert.rejects(validateProxyOfferForMembership(row.offer, row.market, row.membership));
  });
}

test('proxy prices and providers vary without splitting a market', async () => {
  const first = clone(fixture.cases[0]);
  const second = clone(first);
  second.membership.provider_pubkey = 'c'.repeat(64);
  second.membership.recipe_hash = 'd'.repeat(64);
  second.membership.max_concurrency = 1;
  second.offer.provider_pubkey = second.membership.provider_pubkey;
  second.offer.rates[0].per_unit_au = (100n * BigInt(second.offer.rates[0].per_unit_au)).toString();
  await validateProxyOfferForMembership(second.offer, second.market, second.membership);
  assert.equal(await proxyMarketId(first.market), await proxyMarketId(second.market));
  assert.notEqual(await proxyOfferDigest(first.offer), await proxyOfferDigest(second.offer));
  second.market.model.revision = 'different-weights';
  assert.notEqual(await proxyMarketId(first.market), await proxyMarketId(second.market));
});

test('proxy offer revisions and immutable acceptance snapshots', async () => {
  const accepted = clone(fixture.cases[0].offer);
  const next = clone(accepted);
  assert.throws(() => validateProxyOfferRevision(next, 1));
  next.revision++;
  next.rates[1].per_unit_au = (BigInt(next.rates[1].per_unit_au) * 3n).toString();
  validateProxyOfferRevision(next, 1);
  assert.throws(() => validateProxyOfferRevision(next, 2));
  assert.notEqual(await proxyOfferDigest(accepted), await proxyOfferDigest(next));
  assert.equal(3n * BigInt(proxyOfferCost(accepted, {output_token: 10})), BigInt(proxyOfferCost(next, {output_token: 10})));
});

test('proxy exact costs reject hidden usage and u128 overflow', () => {
  const offer = clone(fixture.cases[0].offer);
  offer.rates[0].per_unit_au = '1'; offer.rates[0].granularity = 3;
  offer.rates[1].per_unit_au = '2'; offer.rates[1].granularity = 3;
  offer.per_request_au = '7';
  assert.equal(proxyOfferCost(offer, {input_token: 1, output_token: 1}), '9');
  offer.min_session_au = '10';
  assert.equal(proxyOfferCost(offer, {input_token: 1, output_token: 1}), '10');
  assert.throws(() => proxyOfferCost(offer, {hidden_compute: 1}));
  offer.rates[0].per_unit_au = ((1n << 128n) - 1n).toString();
  assert.throws(() => proxyOfferCost(offer, {input_token: 2}));
  offer.rates[0].granularity = 1;
  assert.throws(() => proxyOfferCost(offer, {input_token: 1}));
  offer.per_request_au = '0';
  assert.equal(proxyOfferCost(offer, {input_token: 1}), ((1n << 128n) - 1n).toString());
});

test('proxy signed JSON rejects unpaired Unicode and missing claim fields', () => {
  const market = clone(fixture.cases[0].market);
  market.model.model_id = '\ud800';
  assert.throws(() => validateProxyMarket(market));
  market.model.model_id = 'valid';
  delete market.model.revision;
  assert.throws(() => validateProxyMarket(market));
});

test('proxy accepted rail subset is checked against the provider', async () => {
  const row = clone(fixture.cases[0]);
  row.membership.accepted_rails = ['fiat'];
  await assert.rejects(validateProxyOfferForMembership(row.offer, row.market, row.membership));
});

test('proxy admission verifies a real signature and all canonical context bindings', async () => {
  const issuer = await makeIdentity();
  const stranger = await makeIdentity();
  const permit = clone(fixture.cases[0].permit);
  permit.issuer_pubkey = issuer.publicKey;
  const verify = makeVerifier(issuer.wallet).verify;
  const context = {
    network_id: permit.network_id, msb_bootstrap: permit.msb_bootstrap, subnet_bootstrap: permit.subnet_bootstrap,
    contract_version: permit.contract_version, provider_pubkey: permit.provider_pubkey,
    initial_operation_digest: permit.initial_operation_digest, fee_policy_hash: permit.fee_policy_hash,
    epoch: 105, max_permit_epochs: 11, active_issuers: [issuer.publicKey],
  };
  const signed = (p, wallet = issuer.wallet) => ({permit: p,
    issuer_signature: b4a.toString(wallet.sign(proxyAdmissionSigningBytes(p)), 'hex')});
  for (const rail of ['fiat', 'tnk', 'tap']) {
    const candidate = {...permit, rail};
    await verifyProxyAdmissionPermit(signed(candidate), context, verify);
  }
  const envelope = signed(permit);
  for (const alteration of [
    {network_id: 'another-network'}, {msb_bootstrap: '1'.repeat(64)}, {subnet_bootstrap: '2'.repeat(64)},
    {contract_version: 30}, {provider_pubkey: stranger.publicKey},
    {initial_operation_digest: '0'.repeat(64)}, {fee_policy_hash: '0'.repeat(64)}, {epoch: 99}, {epoch: 111},
    {max_permit_epochs: 10}, {active_issuers: []}, {active_issuers: [stranger.publicKey]},
  ]) await assert.rejects(verifyProxyAdmissionPermit(envelope, {...context, ...alteration}, verify));
  await assert.rejects(verifyProxyAdmissionPermit(signed(permit, stranger.wallet), context, verify));
  await assert.rejects(verifyProxyAdmissionPermit({...envelope, issuer_signature: '0'.repeat(128)}, context, verify));
  await assert.rejects(verifyProxyAdmissionPermit(envelope, context, undefined));
  const changed = clone(envelope);
  changed.permit.rail = 'tap';
  await assert.rejects(verifyProxyAdmissionPermit(changed, context, verify));
  changed.permit = {...permit, evidence_commitment: '0'.repeat(64)};
  await assert.rejects(verifyProxyAdmissionPermit(changed, context, verify));
});

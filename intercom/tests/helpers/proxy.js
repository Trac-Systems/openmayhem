import assert from 'node:assert/strict';
import fs from 'node:fs';
import b4a from 'b4a';
import MayhemContract, { CONTRACT_VERSION } from '../../contract/contract.js';
import { proxyPolicyFeatureKey } from '../../contract/proxy-policy.js';
import { proxyRuntimeContext } from '../../contract/proxy-context.js';
import { proxyRegistryKeys } from '../../contract/proxy-registry.js';
import { proxyMarketId, proxyOperationDigest, proxyOperationSigningBytes, proxyAdmissionSigningBytes, proxyRegistryFeatureKey } from '../../contract/proxy-protocol.js';
import { MemoryStorage, makeIdentity, makeVerifier, executeFeature } from './contract.js';
const wire = JSON.parse(fs.readFileSync(new URL('../../../crates/mayhem-proto/tests/fixtures/proxy-wire-v1.json', import.meta.url)));
const clone = value => JSON.parse(JSON.stringify(value));
const hex = value => value.toString(16).padStart(64, '0');
const sign = (wallet, bytes) => b4a.toString(wallet.sign(bytes), 'hex');

export async function proxyContractFixture(family = 'llm', rail = 'fiat', execution = null) {
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
  if (execution) {
    // Real Rust adapter commitments supplied by the isolated paid-execution fixture.
    row.market.endpoints = [{endpoint:execution.endpoint,contract_hash:execution.endpoint_contract}];
    row.market.metering = clone(execution.metering);
    row.membership.endpoints = clone(row.market.endpoints);
    row.membership.recipe_hash = execution.recipe_hash;
    row.membership.connection_revision = execution.connection_revision;
    row.membership.capacity_group = execution.capacity_group;
    row.offer.endpoint = execution.endpoint;
    row.offer.metering_policy_hash = execution.metering.policy_hash;
    row.offer.rates = execution.metering.units.map(unit=>({unit,per_unit_au:'1',granularity:1}));
  }
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

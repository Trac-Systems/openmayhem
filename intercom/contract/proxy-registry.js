// Deterministic proxy registry transition planner. Both pre-append admission and
// contract application use this function against one canonical snapshot. It never
// writes, queries an external payment service, or scans a ledger prefix. The
// contract must commit the returned writes in its existing atomic apply batch;
// a writer must recheck at application, not trust a previous admission result.
import {
  validateProxyOperationEnvelope, proxyOperationSigningBytes, proxyOperationDigest,
  proxyMarketId, proxyMarketHandle, validateProxyMembershipForMarket,
  validateProxyOfferForMembership, verifyProxyAdmissionPermit, proxyAdmissionDigest,
  proxyOfferDigest, proxyOfferSlotId, ProxyValidationError,
} from './proxy-protocol.js';
import { withProxyDiscoveryWrites } from './proxy-discovery.js';

export const PROXY_PREFIX = 'proxy/v1/';
const key = (...parts) => PROXY_PREFIX + parts.join('/');
const copy = value => value === null ? null : JSON.parse(JSON.stringify(value));
const check = (ok, message) => { if (!ok) throw new ProxyValidationError(message); };
const count = (value, name, minimum = 0) => {
  check(Number.isSafeInteger(value) && value >= minimum, `invalid proxy ${name}`);
  return value;
};
const increment = value => count(value + 1, 'counter overflow');

export const proxyRegistryKeys = Object.freeze({
  config: key('config'),
  provider: provider => key('provider', provider),
  market: market => key('market', market),
  membership: (market, provider) => key('membership', market, provider),
  offer: async (market, provider, endpoint, context, outcome) =>
    key('offer', market, provider, await proxyOfferSlotId({ endpoint, ctx_bracket: context, outcome_class: outcome })),
});

export function validateProxyRegistryConfig(config, context) {
  check(config?.network_id === context.network_id && config?.contract_version === context.contract_version
    && config?.msb_bootstrap === context.msb_bootstrap && config?.subnet_bootstrap === context.subnet_bootstrap,
    'proxy registry not configured for this network/contract');
  check(typeof config.enabled === 'boolean', 'invalid proxy enablement policy');
  for (const name of ['max_permit_epochs', 'max_mutations_per_provider_epoch', 'max_mutations_per_epoch',
    'max_active_memberships', 'max_created_markets_per_provider_epoch', 'max_offer_slots_per_membership']) {
    count(config[name], name, 1);
  }
  check(typeof config.fee_policy_hash === 'string' && /^[0-9a-f]{64}$/.test(config.fee_policy_hash), 'invalid proxy fee policy');
  check(Array.isArray(config.active_issuers) && config.active_issuers.length > 0 && config.active_issuers.length <= 16
    && config.active_issuers.every((issuer, i, all) => /^[0-9a-f]{64}$/.test(issuer) && (i === 0 || all[i - 1] < issuer)),
  'invalid proxy issuer policy');
}

async function checkMarketPolicy(market, read) {
  check((await read(key('family', market.model.family_id)))?.enabled === true, 'unknown proxy model family');
  const meter = await read(key('metering-policy', market.metering.policy_hash));
  check(meter?.enabled === true && JSON.stringify(meter.units) === JSON.stringify(market.metering.units),
    'unsupported proxy metering contract');
  const policies = new Map();
  for (const endpoint of market.endpoints) {
    const policy = await read(key('endpoint-policy', endpoint.contract_hash));
    check(policy?.enabled === true && policy.endpoint === endpoint.endpoint && policy.family === market.family,
      'unsupported proxy endpoint contract');
    policies.set(endpoint.endpoint, policy);
  }
  return policies;
}

// `read` returns a decoded value or null, always from the same immutable snapshot.
// `context` comes from the application consensus/network, never from the intent.
export async function prepareProxyRegistryMutation(envelope, context, read, verifySignature) {
  validateProxyOperationEnvelope(envelope);
  envelope = copy(envelope);
  context = copy(context);
  check(typeof read === 'function' && typeof verifySignature === 'function', 'proxy registry dependencies missing');
  count(context.epoch, 'epoch', 1);
  const intent = copy(envelope.intent);
  check(intent.network_id === context.network_id && intent.contract_version === context.contract_version
    && intent.msb_bootstrap === context.msb_bootstrap && intent.subnet_bootstrap === context.subnet_bootstrap,
    'proxy operation network/contract mismatch');
  check(await verifySignature(envelope.provider_signature, proxyOperationSigningBytes(intent), intent.provider_pubkey) === true,
    'invalid proxy provider signature');
  const digest = await proxyOperationDigest(intent);
  const providerKey = proxyRegistryKeys.provider(intent.provider_pubkey);
  const previous = copy(await read(providerKey));
  if (previous && intent.sequence === previous.sequence && digest === previous.operation_digest) {
    // A signed exact retry returns the historical result without a new append or
    // effect. This is not a new admission or authorization to serve after revocation.
    return { duplicate: true, result: copy(previous.result), writes: [] };
  }
  check(intent.sequence === increment(count(previous?.sequence ?? 0, 'provider sequence')), 'stale or out-of-order proxy operation');
  const config = copy(await read(proxyRegistryKeys.config));
  validateProxyRegistryConfig(config, context);
  const action = intent.action;
  const withdrawal = action.kind === 'leave_market' || action.kind === 'withdraw_offer';
  check(config.enabled || withdrawal, 'new proxy activity is disabled');
  check(!await read(key('provider-revoked', intent.provider_pubkey)) || withdrawal, 'proxy provider is revoked');

  // Queue all state changes only after validation; no caller gets a partial plan.
  const writes = new Map();
  const put = (path, value) => {
    check(path.startsWith(PROXY_PREFIX), 'proxy mutation escaped its namespace');
    writes.set(path, { key: path, value: copy(value) });
  };
  const provider = previous ?? { sequence: 0, active_memberships: 0, epoch: context.epoch, mutations: 0, created_markets: 0 };
  count(provider.active_memberships, 'active membership count');
  if (provider.epoch !== context.epoch) {
    check(provider.epoch < context.epoch, 'proxy epoch regressed');
    provider.epoch = context.epoch;
    provider.mutations = 0;
    provider.created_markets = 0;
  }
  provider.mutations = increment(count(provider.mutations, 'provider mutations'));
  check(provider.mutations <= config.max_mutations_per_provider_epoch, 'proxy provider mutation quota exceeded');
  const globalKey = key('mutation-budget');
  const global = copy(await read(globalKey)) ?? { epoch: context.epoch, mutations: 0 };
  check(global.epoch <= context.epoch, 'proxy epoch regressed');
  if (global.epoch !== context.epoch) { global.epoch = context.epoch; global.mutations = 0; }
  global.mutations = increment(count(global.mutations, 'global mutations'));
  check(global.mutations <= config.max_mutations_per_epoch, 'proxy global mutation quota exceeded');

  if (!previous) {
    check(action.kind === 'create_market' || action.kind === 'join_market', 'first proxy operation must create or join a market');
    check(envelope.admission !== null, 'verified proxy admission permit required');
    const permit = await verifyProxyAdmissionPermit(envelope.admission, {
      network_id: context.network_id, msb_bootstrap: context.msb_bootstrap, subnet_bootstrap: context.subnet_bootstrap,
      contract_version: context.contract_version,
      provider_pubkey: intent.provider_pubkey, initial_operation_digest: digest,
      fee_policy_hash: config.fee_policy_hash, epoch: context.epoch,
      max_permit_epochs: config.max_permit_epochs, active_issuers: config.active_issuers,
    }, verifySignature);
    for (const [type, id] of [['entitlement', permit.entitlement_id], ['invoice', permit.invoice_commitment], ['evidence', permit.evidence_commitment]]) {
      check(!await read(key('admission-used', type, id)), `proxy admission ${type} already consumed`);
      put(key('admission-used', type, id), { provider_pubkey: intent.provider_pubkey, entitlement_id: permit.entitlement_id });
    }
    check(!await read(key('admission-revoked', permit.entitlement_id)), 'proxy admission is revoked');
    const issuance = await read(key('admission-generation', permit.entitlement_id));
    check(!issuance || (issuance.revision === permit.issuance_revision && issuance.permit_digest === await proxyAdmissionDigest(permit)),
      'proxy admission issuance was superseded');
    // Off-ledger payment verifier must enforce global cross-purpose evidence
    // ownership before issuing. This proxy index is a second replay barrier; it
    // does not by itself fence evidence already credited through retail.
    provider.entitlement = { id: permit.entitlement_id, permit_digest: await proxyAdmissionDigest(permit),
      issuance_revision: permit.issuance_revision, rail: permit.rail, accepted_amount: permit.accepted_amount,
      accepted_value_au: permit.accepted_value_au, issuer_pubkey: permit.issuer_pubkey };
  } else {
    check(envelope.admission === null, 'proxy entitlement already exists; no second admission payment');
    check(typeof provider.entitlement?.id === 'string', 'proxy entitlement missing');
    check(!await read(key('admission-revoked', provider.entitlement.id)) || withdrawal, 'proxy admission is revoked');
  }

  const marketId = action.membership?.market_id ?? action.offer?.market_id ?? action.market_id;
  const marketKey = proxyRegistryKeys.market(marketId);
  let market = copy(await read(marketKey));
  if (action.kind === 'create_market') {
    check(!market, 'proxy market already exists');
    check(await proxyMarketId(action.market) === marketId, 'proxy membership market mismatch');
    market = copy(action.market);
    const handleKey = key('handle', proxyMarketHandle(market));
    check(!await read(handleKey), 'proxy public handle already exists');
    provider.created_markets = increment(count(provider.created_markets, 'created market count'));
    check(provider.created_markets <= config.max_created_markets_per_provider_epoch, 'proxy market creation quota exceeded');
    put(marketKey, market);
    put(handleKey, marketId);
    put(key('family-market', market.model.family_id, marketId), { market_id: marketId });
  }
  check(market !== null, 'unknown proxy market');
  const policies = !withdrawal ? await checkMarketPolicy(market, read) : null;
  const membershipKey = proxyRegistryKeys.membership(marketId, intent.provider_pubkey);
  const existingMember = copy(await read(membershipKey));
  if (['create_market', 'join_market', 'update_membership'].includes(action.kind)) {
    const member = action.membership;
    await validateProxyMembershipForMarket(member, market);
    if (action.kind === 'update_membership') check(existingMember?.active === true, 'proxy membership is not active');
    else check(existingMember?.active !== true, 'proxy membership already active');
    check(member.revision > count(existingMember?.revision ?? 0, 'membership revision'), 'stale proxy membership revision');
    if (existingMember) check(member.connection_revision >= existingMember.member.connection_revision, 'proxy connection revision regressed');
    for (const endpoint of member.endpoints) {
      const policy = policies.get(endpoint.endpoint);
      check(member.served_context <= count(policy.max_context, 'endpoint context', 1), 'proxy context exceeds supported endpoint contract');
    }
    if (existingMember?.active !== true) provider.active_memberships = increment(provider.active_memberships);
    check(provider.active_memberships <= config.max_active_memberships, 'proxy active membership quota exceeded');
    put(membershipKey, { active: true, revision: member.revision, member, offer_slots: existingMember?.offer_slots ?? 0 });
    put(key('provider-market', intent.provider_pubkey, marketId), { market_id: marketId });
    put(key('market-provider', marketId, intent.provider_pubkey), { provider_pubkey: intent.provider_pubkey });
  } else if (action.kind === 'leave_market') {
    check(existingMember?.active === true, 'proxy membership is not active');
    check(action.revision > existingMember.revision, 'stale proxy membership revision');
    check(provider.active_memberships > 0, 'invalid proxy active membership count');
    provider.active_memberships--;
    put(membershipKey, { ...existingMember, active: false, revision: action.revision });
    put(key('provider-market', intent.provider_pubkey, marketId), null);
    put(key('market-provider', marketId, intent.provider_pubkey), null);
  } else {
    // Offers are invalid for routing whenever active membership/revision differs.
    // Keep the record/high-water mark across withdrawal, leave and rejoin. Never
    // scan/rewrite all historical offers when changing one membership.
    check(existingMember?.active === true, 'proxy membership is not active');
    const offer = action.offer ?? action;
    const offerKey = await proxyRegistryKeys.offer(marketId, intent.provider_pubkey, offer.endpoint, offer.ctx_bracket, offer.outcome_class);
    const oldOffer = copy(await read(offerKey));
    check(offer.revision > count(oldOffer?.revision ?? 0, 'offer revision'), 'stale proxy offer revision');
    if (action.kind === 'withdraw_offer') {
      check(oldOffer?.active === true, 'proxy offer is not active');
      put(offerKey, { ...oldOffer, active: false, revision: offer.revision });
    } else {
      await validateProxyOfferForMembership(offer, market, existingMember.member);
      const policy = policies.get(offer.endpoint);
      check(Array.isArray(policy.ctx_brackets) && policy.ctx_brackets.includes(offer.ctx_bracket), 'unsupported proxy context bracket');
      check(Array.isArray(policy.outcome_classes) && policy.outcome_classes.includes(offer.outcome_class), 'unsupported proxy outcome class');
      if (!oldOffer) {
        existingMember.offer_slots = increment(count(existingMember.offer_slots, 'offer slot count'));
        check(existingMember.offer_slots <= config.max_offer_slots_per_membership, 'proxy offer slot quota exceeded');
        put(membershipKey, existingMember);
      }
      put(offerKey, { active: true, revision: offer.revision, digest: await proxyOfferDigest(offer), offer });
    }
  }
  const result = { lane: 'proxy', provider_pubkey: intent.provider_pubkey, sequence: intent.sequence,
    operation_digest: digest, market_id: marketId, action: action.kind };
  provider.sequence = intent.sequence;
  provider.operation_digest = digest;
  provider.result = result;
  put(providerKey, provider);
  put(globalKey, global);
  return { duplicate: false, result, writes: withProxyDiscoveryWrites([...writes.values()]) };
}

// New-acceptance eligibility only. Never use this to reprice or refuse settlement
// of previously accepted work: that work retains its signed offer snapshot.
// Liveness, capacity, buyer caps and probe evidence are additional gateway checks.
export async function readActiveProxyOffer(selection, context, read) {
  check(selection !== null && typeof selection === 'object', 'invalid proxy offer selection');
  check(/^[0-9a-f]{64}$/.test(selection.market_id) && /^[0-9a-f]{64}$/.test(selection.provider_pubkey), 'invalid proxy offer identity');
  check(['openai_chat_completions', 'openai_completions', 'openai_responses', 'mayhem_decisions'].includes(selection.endpoint), 'invalid proxy offer endpoint');
  check(typeof selection.ctx_bracket === 'string' && /^[a-z][a-z0-9_-]{0,63}$/.test(selection.ctx_bracket), 'invalid proxy offer context');
  check(selection.outcome_class === '' || (selection.endpoint === 'mayhem_decisions' && /^[0-9a-f]{64}$/.test(selection.outcome_class)), 'invalid proxy offer outcome');
  selection = copy(selection);
  context = copy(context);
  const config = await read(proxyRegistryKeys.config);
  validateProxyRegistryConfig(config, context);
  if (!config.enabled) return null;
  const provider = await read(proxyRegistryKeys.provider(selection.provider_pubkey));
  if (!provider?.entitlement?.id || await read(key('provider-revoked', selection.provider_pubkey))
    || await read(key('admission-revoked', provider.entitlement.id))) return null;
  const member = await read(proxyRegistryKeys.membership(selection.market_id, selection.provider_pubkey));
  if (member?.active !== true) return null;
  const record = await read(await proxyRegistryKeys.offer(selection.market_id, selection.provider_pubkey,
    selection.endpoint, selection.ctx_bracket, selection.outcome_class));
  if (record?.active !== true || record.offer.membership_revision !== member.revision) return null;
  const market = await read(proxyRegistryKeys.market(selection.market_id));
  check(market !== null, 'proxy offer refers to missing market');
  const policies = await checkMarketPolicy(market, read);
  await validateProxyOfferForMembership(record.offer, market, member.member);
  const policy = policies.get(record.offer.endpoint);
  check(member.member.served_context <= count(policy.max_context, 'endpoint context', 1), 'proxy context exceeds supported endpoint contract');
  check(Array.isArray(policy.ctx_brackets) && policy.ctx_brackets.includes(record.offer.ctx_bracket), 'unsupported proxy context bracket');
  check(Array.isArray(policy.outcome_classes) && policy.outcome_classes.includes(record.offer.outcome_class), 'unsupported proxy outcome class');
  check(record.revision === record.offer.revision && record.digest === await proxyOfferDigest(record.offer), 'proxy offer record is inconsistent');
  return copy({ market, membership: member.member, offer: record.offer, offer_digest: record.digest });
}

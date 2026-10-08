// Deterministic public read model, updated in the SAME atomic registry batch.
// No history reconstruction, fee evidence, endpoint secrets or native catalog writes.
import { ProxyValidationError } from './proxy-protocol.js';

export const PROXY_CATALOG_PREFIX = 'proxy/v1/catalog/';
export const PROXY_CATALOG_INDEX_PREFIX = 'proxy/v1/catalog-index/';
const root = 'proxy/v1/';
const clone = value => value === null ? null : JSON.parse(JSON.stringify(value));
const kinds = new Map([
  ['market', 'markets'], ['membership', 'memberships'], ['offer', 'offers'],
  ['family', 'families'], ['endpoint-policy', 'endpoints'], ['metering-policy', 'metering'],
  ['provider-revoked', 'provider_status'], ['admission-revoked', 'admission_status'],
]);

export function withProxyDiscoveryWrites(writes) {
  const result = new Map(writes.map(write => [write.key, write]));
  const put = (key, value) => {
    if (result.has(key)) throw new ProxyValidationError('duplicate proxy discovery write');
    result.set(key, { key, value: clone(value) });
  };
  for (const { key, value } of writes) {
    if (!key.startsWith(root)) throw new ProxyValidationError('proxy discovery escaped registry');
    const [type, ...id] = key.slice(root.length).split('/');
    let kind = kinds.get(type);
    let publicValue = value;
    if (type === 'provider') {
      kind = 'providers';
      publicValue = value === null ? null : { provider_pubkey: id[0],
        admission_id: value.entitlement.id, sequence: value.sequence, active_memberships: value.active_memberships };
    } else if (type === 'config') {
      kind = 'network'; id.push('current');
      publicValue = value === null ? null : { enabled: value.enabled, network_id: value.network_id,
        contract_version: value.contract_version, msb_bootstrap: value.msb_bootstrap, subnet_bootstrap: value.subnet_bootstrap };
    }
    if (!kind) continue;
    const target = `${PROXY_CATALOG_PREFIX}${kind}/${id.join('/')}`;
    put(target, publicValue);
    const reference = value === null ? null : { key: target,
      stamp: [value.revision ?? 0, value.offer_slots ?? 0, value.active ?? null] };
    if (type === 'market') {
      // Descriptors are immutable; a model/family change creates another market.
      // There is deliberately no mutable display-name alias in market identity.
      put(`${PROXY_CATALOG_INDEX_PREFIX}type/${value.family}/${id[0]}`, reference);
      put(`${PROXY_CATALOG_INDEX_PREFIX}family/${value.model.family_id}/${id[0]}`, reference);
      put(`${PROXY_CATALOG_INDEX_PREFIX}type-family/${value.family}/${value.model.family_id}/${id[0]}`, reference);
    } else if (type === 'membership' || type === 'offer') {
      put(`${PROXY_CATALOG_INDEX_PREFIX}${kind}-provider/${id[1]}/${id[0]}${id[2] ? `/${id[2]}` : ''}`, reference);
    }
  }
  return [...result.values()];
}

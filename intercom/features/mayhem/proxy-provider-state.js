// Exact canonical onboarding facts. Absence is not evidence of an unpaid fee:
// an invoice or verifier permit may still exist off-ledger. No writes or scans.
import b4a from 'b4a';
import { proxyRegistryKeys, validateProxyRegistryConfig } from '../../contract/proxy-registry.js';

export const PROXY_PROVIDER_STATE_SERVICE = 'proxy_provider_state';
export const PROXY_PROVIDER_STATE_MAX_BYTES = 8192;
export const PROXY_PROVIDER_STATE_MAX_AGE_MS = 15000;
const active = new WeakMap();
const hex = v => typeof v === 'string' && /^[0-9a-f]{64}$/.test(v);
const need = (ok, message) => { if (!ok) throw new Error(`Proxy provider state: ${message}.`); };

export function validateProxyProviderStateRequest(value) {
  need(value && typeof value === 'object' && !Array.isArray(value)
    && Object.keys(value).sort().join('|') === 'initial_operation_digest|provider_pubkey|request_nonce|requester'
    && Object.values(value).every(hex) && value.requester === value.provider_pubkey
    && b4a.byteLength(JSON.stringify(value)) <= 1024, 'invalid owned-provider query');
}

export async function readProxyProviderState({ request, withCanonicalSnapshot }) {
  validateProxyProviderStateRequest(request);
  request = JSON.parse(JSON.stringify(request));
  need(typeof withCanonicalSnapshot === 'function', 'canonical service is unavailable');
  const count = active.get(withCanonicalSnapshot) ?? 0;
  need(count < 4, 'read capacity is busy');
  active.set(withCanonicalSnapshot, count + 1);
  let timer;
  // A timed-out snapshot keeps its read permit until actual cleanup finishes.
  const work = Promise.resolve().then(() => withCanonicalSnapshot(async snapshot => {
    await snapshot.assertCurrent();
    const config = await snapshot.read(proxyRegistryKeys.config);
    validateProxyRegistryConfig(config, snapshot.context);
    const record = await snapshot.read(proxyRegistryKeys.provider(request.provider_pubkey));
    const providerRevoked = await snapshot.read(`proxy/v1/provider-revoked/${request.provider_pubkey}`);
    let provider = null, admissionRevoked = null;
    if (record !== null) {
      need(Number.isSafeInteger(record.sequence) && record.sequence > 0 && hex(record.operation_digest)
        && hex(record.entitlement?.id), 'invalid canonical provider record');
      const id = record.entitlement.id;
      const consumed = await snapshot.read(`proxy/v1/admission-used/entitlement/${id}`);
      need(consumed?.provider_pubkey === request.provider_pubkey && consumed.entitlement_id === id,
        'canonical entitlement ownership differs');
      admissionRevoked = await snapshot.read(`proxy/v1/admission-revoked/${id}`);
      provider = { sequence: record.sequence, operation_digest: record.operation_digest, entitlement_id: id };
    }
    await snapshot.assertCurrent();
    const response = { ok: true, schema_version: 1, lane: 'proxy', ...request,
      context: snapshot.context, proof: snapshot.proof,
      registry_enabled: config.enabled, fee_policy_hash: config.fee_policy_hash,
      provider, provider_revoked: providerRevoked !== null, admission_revoked: admissionRevoked !== null };
    need(b4a.byteLength(JSON.stringify(response)) <= PROXY_PROVIDER_STATE_MAX_BYTES, 'response exceeds bound');
    return response;
  })).finally(() => {
    clearTimeout(timer);
    const remaining = active.get(withCanonicalSnapshot) - 1;
    if (remaining) active.set(withCanonicalSnapshot, remaining); else active.delete(withCanonicalSnapshot);
  });
  const expired = new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('Proxy provider state: observation expired.')), PROXY_PROVIDER_STATE_MAX_AGE_MS); });
  return await Promise.race([work, expired]);
}

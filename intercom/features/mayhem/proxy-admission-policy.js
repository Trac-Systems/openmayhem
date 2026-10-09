// Exact public admission policy from the authenticated canonical service.
// No provider ownership, payment evidence, ledger scans or writes.
import b4a from 'b4a';
import { proxyRegistryKeys, validateProxyRegistryConfig } from '../../contract/proxy-registry.js';

export const PROXY_ADMISSION_POLICY_SERVICE = 'proxy_admission_policy';
export const PROXY_ADMISSION_POLICY_MAX_BYTES = 8192;
export const PROXY_ADMISSION_POLICY_MAX_AGE_MS = 15000;
const active = new WeakMap();
const hex = v => typeof v === 'string' && /^[0-9a-f]{64}$/.test(v);
const need = (ok, message) => { if (!ok) throw new Error(`Proxy admission policy: ${message}.`); };

export function validateProxyAdmissionPolicyRequest(value) {
  need(value && typeof value === 'object' && !Array.isArray(value)
    && ['request_nonce|requester', 'provider_pubkey|request_nonce|requester'].includes(Object.keys(value).sort().join('|'))
    && Object.values(value).every(hex)
    && b4a.byteLength(JSON.stringify(value)) <= 1024, 'invalid policy query');
}

export async function readProxyAdmissionPolicy({ request, withCanonicalSnapshot }) {
  validateProxyAdmissionPolicyRequest(request);
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
    // Optional public enrollment lookup is for the admission collector. The
    // provider-owned setup query remains unchanged; neither read can authorize
    // a fee, countersign a permit or mutate canonical state.
    let enrollment;
    if (request.provider_pubkey !== undefined) {
      const key = request.provider_pubkey;
      const record = await snapshot.read(proxyRegistryKeys.provider(key));
      const revoked = await snapshot.read(`proxy/v1/provider-revoked/${key}`);
      let entitlement = null, admissionRevoked = null;
      if (record !== null) {
        need(hex(record.entitlement?.id), 'invalid canonical entitlement');
        entitlement = record.entitlement.id;
        const used = await snapshot.read(`proxy/v1/admission-used/entitlement/${entitlement}`);
        need(used?.provider_pubkey === key && used.entitlement_id === entitlement, 'entitlement ownership differs');
        admissionRevoked = await snapshot.read(`proxy/v1/admission-revoked/${entitlement}`);
      }
      enrollment = { provider_pubkey: key, entitlement_id: entitlement,
        provider_revoked: revoked !== null, admission_revoked: admissionRevoked !== null };
    }
    await snapshot.assertCurrent();
    const response = { ok: true, schema_version: 1, lane: 'proxy', ...request,
      context: snapshot.context, proof: snapshot.proof,
      registry_enabled: config.enabled, fee_policy_hash: config.fee_policy_hash,
      active_issuers: config.active_issuers, max_permit_epochs: config.max_permit_epochs,
      ...(enrollment === undefined ? {} : { enrollment }) };
    need(b4a.byteLength(JSON.stringify(response)) <= PROXY_ADMISSION_POLICY_MAX_BYTES, 'response exceeds bound');
    return response;
  })).finally(() => {
    clearTimeout(timer);
    const remaining = active.get(withCanonicalSnapshot) - 1;
    if (remaining) active.set(withCanonicalSnapshot, remaining); else active.delete(withCanonicalSnapshot);
  });
  const expired = new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('Proxy admission policy: observation expired.')), PROXY_ADMISSION_POLICY_MAX_AGE_MS); });
  return await Promise.race([work, expired]);
}

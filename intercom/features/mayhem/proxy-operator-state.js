// Public operator identity evidence from exact canonical keys. This does not
// attest model execution, hardware, data handling, or endpoint capabilities.
import b4a from 'b4a';

export const PROXY_OPERATOR_STATE_SERVICE = 'proxy_operator_state';
export const PROXY_OPERATOR_STATE_MAX_BYTES = 4096;
export const PROXY_OPERATOR_STATE_MAX_AGE_MS = 15000;
const active = new WeakMap();
const hex = value => typeof value === 'string' && /^[0-9a-f]{64}$/.test(value);
const need = (ok, message) => { if (!ok) throw new Error(`Proxy operator state: ${message}.`); };

export function validateProxyOperatorStateRequest(value) {
  need(value && typeof value === 'object' && !Array.isArray(value)
    && Object.keys(value).sort().join('|') === 'provider_pubkey|request_nonce|requester'
    && Object.values(value).every(hex) && b4a.byteLength(JSON.stringify(value)) <= 1024,
  'invalid provider query');
}

function observation(provider, kyb, id) {
  if (provider === null) return { status: 'not_registered', proof_hash: null };
  need(provider.provider === id && typeof provider.status === 'string', 'canonical provider identity is invalid');
  if (['banned', 'inactive'].includes(provider.status)) return { status: 'inactive', proof_hash: null };
  need(provider.status === 'active', 'canonical provider status is unknown');
  if (kyb === null) return { status: 'not_verified', proof_hash: null };
  need(kyb.provider === id, 'canonical KYB identity differs');
  if (kyb.status === 'revoked') return { status: 'revoked', proof_hash: null };
  // Same facts required by native active_provider_kyb_info. Only the canonical
  // admin command can create this record; provider.kyb summaries are not proof.
  need(kyb.status === 'verified' && kyb.verified_by_role === 'admin'
    && hex(kyb.proof_hash) && typeof kyb.admin_sig === 'string' && /^[0-9a-f]{128}$/.test(kyb.admin_sig)
    && ['legal_name', 'jurisdiction', 'kyb_ref'].every(key => typeof kyb[key] === 'string' && kyb[key].trim())
    && Number.isSafeInteger(kyb.schema_version) && kyb.schema_version > 0,
  'canonical KYB record is invalid');
  return { status: 'verified', proof_hash: kyb.proof_hash };
}

export async function readProxyOperatorState({ request, withCanonicalSnapshot }) {
  validateProxyOperatorStateRequest(request);
  request = structuredClone(request);
  need(typeof withCanonicalSnapshot === 'function', 'canonical service is unavailable');
  const count = active.get(withCanonicalSnapshot) ?? 0;
  need(count < 4, 'read capacity is busy');
  active.set(withCanonicalSnapshot, count + 1);
  let timer;
  // A timeout never frees a permit while underlying canonical work still runs.
  const work = Promise.resolve().then(() => withCanonicalSnapshot(async snapshot => {
    await snapshot.assertCurrent();
    const provider = await snapshot.read(`prov/${request.provider_pubkey}`);
    const kyb = await snapshot.read(`kyb/${request.provider_pubkey}`);
    const operator = observation(provider, kyb, request.provider_pubkey);
    await snapshot.assertCurrent();
    const response = { ok: true, schema_version: 1, lane: 'proxy', ...request,
      context: snapshot.context, proof: snapshot.proof, operator };
    need(b4a.byteLength(JSON.stringify(response)) <= PROXY_OPERATOR_STATE_MAX_BYTES, 'response exceeds bound');
    return response;
  }, { operator: true })).finally(() => {
    clearTimeout(timer);
    const remaining = active.get(withCanonicalSnapshot) - 1;
    if (remaining) active.set(withCanonicalSnapshot, remaining); else active.delete(withCanonicalSnapshot);
  });
  const expired = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error('Proxy operator state: observation expired.')), PROXY_OPERATOR_STATE_MAX_AGE_MS);
  });
  return await Promise.race([work, expired]);
}

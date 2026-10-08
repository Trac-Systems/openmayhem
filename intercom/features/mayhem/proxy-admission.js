// Proxy publication gate. The caller supplies a pinned, verified canonical view,
// never request-provided state. Run this before BOTH forwarding and writer append.
// The contract repeats the same transition checks during application. There is no
// fee worker/network/history lookup on subsequent publications or inference turns.
import { PROXY_MAX_RECORD_BYTES } from '../../contract/proxy-protocol.js';
import b4a from 'b4a';
import MayhemContract from '../../contract/contract.js';
import { validateProxyPublication, proxyPublicationFeatureKey, prepareProxyPublication } from '../../contract/proxy-publication.js';

export { proxyRegistryFeatureKey } from '../../contract/proxy-protocol.js';

export const PROXY_PREFLIGHT_SERVICE = 'proxy_admission_preflight';
export const PROXY_PREFLIGHT_MAX_AGE_MS = 15_000;

export function validateProxyPreflightRequest(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value) ||
      JSON.stringify(Object.keys(value).sort()) !== JSON.stringify(['envelope', 'feature_key', 'request_nonce', 'requester']) ||
      !/^[0-9a-f]{64}$/.test(value.requester) || !/^[0-9a-f]{64}$/.test(value.request_nonce) ||
      typeof value.feature_key !== 'string' || value.feature_key.length > 256 ||
      b4a.byteLength(JSON.stringify(value)) > PROXY_MAX_RECORD_BYTES + 512) {
    throw new Error('Invalid proxy admission preflight request.');
  }
  validateProxyPublication(value.envelope);
  if (value.envelope.op === 'proxy_policy') throw new Error('Proxy policy requires the canonical admin writer.');
}

// This service returns no publication permit and performs no write. The signed
// service transport binds a fresh challenge and exact operation to the admin's
// response; writer admission still revalidates immediately before append.
export async function preflightProxyRegistry({ request, withCanonicalSnapshot, verifySignature }) {
  validateProxyPreflightRequest(request);
  request = JSON.parse(JSON.stringify(request));
  if (typeof withCanonicalSnapshot !== 'function') throw new Error('Proxy canonical admission is not configured.');
  let context;
  let proof;
  const result = await admitProxyRegistryFeature({ featureKey: request.feature_key, envelope: request.envelope,
    verifySignature,
    withCanonicalSnapshot: (body, options) => withCanonicalSnapshot(snapshot => {
      context = snapshot.context;
      proof = snapshot.proof;
      return body(snapshot);
    }, options),
    forward: async () => ({ duplicate: false }),
  });
  return { ok: true, status: result.duplicate ? 'applied' : 'admissible',
    request_nonce: request.request_nonce, feature_key: request.feature_key,
    context, proof, ...(result.duplicate ? { result: result.result } : {}) };
}

// withCanonicalSnapshot pins the indexer-authenticated signed checkout and closes
// it in finally. Its assertCurrent() must reject a stale/wrong-fork view and changed
// admission policy, including revocation, before forwarding. A local view being
// merely signed is not enough to establish that it is the canonical current view.
// Snapshot acquisition, writer pending deduplication and dispatch journaling are
// transport responsibilities; this helper does not claim to implement them.
export async function admitProxyPublicationFeature({ featureKey, envelope, withCanonicalSnapshot, verifySignature, forward }) {
  validateProxyPublication(envelope);
  envelope = JSON.parse(JSON.stringify(envelope));
  if (typeof withCanonicalSnapshot !== 'function' || typeof forward !== 'function'
      || (envelope.op !== 'proxy_policy' && typeof verifySignature !== 'function')) {
    throw new Error('Proxy canonical admission is not configured.');
  }
  if (featureKey !== await proxyPublicationFeatureKey(envelope)) throw new Error('Invalid proxy publication feature key.');
  const financial = envelope.op === 'proxy_spend_reserve' || envelope.op === 'proxy_record_usage';
  return await withCanonicalSnapshot(async snapshot => {
    if (typeof snapshot?.assertCurrent !== 'function' || typeof snapshot?.read !== 'function') {
      throw new Error('Proxy canonical snapshot is incomplete.');
    }
    const reads = new Set();
    const ledger = Object.create(MayhemContract.prototype);
    ledger.get = async key => { reads.add(key); return await snapshot.read(key); };
    ledger.put = ledger.del = () => { throw new Error('Admission cannot mutate the canonical snapshot.'); };
    await snapshot.assertCurrent();
    const plan = await prepareProxyPublication(ledger, envelope, snapshot.context, verifySignature);
    await snapshot.assertCurrent();
    if (plan.duplicate) return { duplicate: true, result: plan.result };
    // Only dependency keys accompany this trusted local callback. Prepared values
    // never cross ingress; application derives them from its current atomic batch.
    return await forward({ featureKey, envelope, fences: {
      reads: [...reads].sort(), writes: [...new Set(plan.writes.map(write => write.key))].sort(),
    } });
  }, { financial });
}

// Retain internal call-site compatibility; both names use the same gate.
export const admitProxyRegistryFeature = admitProxyPublicationFeature;
export const admitProxyPolicyFeature = admitProxyPublicationFeature;

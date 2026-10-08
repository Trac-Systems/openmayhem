// Proxy publication gate. The caller supplies a pinned, verified canonical view,
// never request-provided state. Run this before BOTH forwarding and writer append.
// The contract repeats the same transition checks during application. There is no
// fee worker/network/history lookup on subsequent publications or inference turns.
import { validateProxyOperationEnvelope, proxyRegistryFeatureKey } from '../../contract/proxy-protocol.js';
import { prepareProxyRegistryMutation } from '../../contract/proxy-registry.js';
import { validateProxyPolicy, proxyPolicyFeatureKey, prepareProxyPolicyMutation } from '../../contract/proxy-policy.js';

export { proxyRegistryFeatureKey } from '../../contract/proxy-protocol.js';

// withCanonicalSnapshot pins the indexer-authenticated signed checkout and closes
// it in finally. Its assertCurrent() must reject a stale/wrong-fork view and changed
// admission policy, including revocation, before forwarding. A local view being
// merely signed is not enough to establish that it is the canonical current view.
// Snapshot acquisition, writer pending deduplication and dispatch journaling are
// transport responsibilities; this helper does not claim to implement them.
export async function admitProxyRegistryFeature({ featureKey, envelope, withCanonicalSnapshot, verifySignature, forward }) {
  validateProxyOperationEnvelope(envelope);
  envelope = JSON.parse(JSON.stringify(envelope));
  if (typeof withCanonicalSnapshot !== 'function' || typeof verifySignature !== 'function' || typeof forward !== 'function') {
    throw new Error('Proxy canonical admission is not configured.');
  }
  if (featureKey !== await proxyRegistryFeatureKey(envelope)) throw new Error('Invalid proxy registry feature key.');
  return await withCanonicalSnapshot(async snapshot => {
    if (typeof snapshot?.assertCurrent !== 'function' || typeof snapshot?.read !== 'function') {
      throw new Error('Proxy canonical snapshot is incomplete.');
    }
    await snapshot.assertCurrent();
    const plan = await prepareProxyRegistryMutation(envelope, snapshot.context, snapshot.read, verifySignature);
    await snapshot.assertCurrent();
    if (plan.duplicate) return { duplicate: true, result: plan.result };
    // No prepared writes cross the ingress boundary. Application re-derives them
    // against its then-current canonical state, preventing a forged write plan.
    return await forward({ featureKey, envelope });
  });
}

// Only the canonical admin may call this. Policy still needs the same snapshot
// and pre-append validation; a metadata edit cannot append an invalid policy.
export async function admitProxyPolicyFeature({ featureKey, envelope, withCanonicalSnapshot, forward }) {
  validateProxyPolicy(envelope);
  envelope = JSON.parse(JSON.stringify(envelope));
  if (typeof withCanonicalSnapshot !== 'function' || typeof forward !== 'function') {
    throw new Error('Proxy canonical admission is not configured.');
  }
  if (featureKey !== await proxyPolicyFeatureKey(envelope)) throw new Error('Invalid proxy policy feature key.');
  return await withCanonicalSnapshot(async snapshot => {
    if (typeof snapshot?.assertCurrent !== 'function' || typeof snapshot?.read !== 'function') {
      throw new Error('Proxy canonical snapshot is incomplete.');
    }
    await snapshot.assertCurrent();
    const plan = await prepareProxyPolicyMutation(envelope, snapshot.context, snapshot.read);
    await snapshot.assertCurrent();
    if (plan.duplicate) return { duplicate: true, result: plan.result };
    return await forward({ featureKey, envelope });
  });
}

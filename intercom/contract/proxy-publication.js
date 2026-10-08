// One operation classifier for the admitted feature path. No caller-supplied
// write plan, native transaction alias or alternate relay may bypass it.
import { ProxyValidationError, validateProxyOperationEnvelope, proxyRegistryFeatureKey } from './proxy-protocol.js';
import { validateProxyPolicy, proxyPolicyFeatureKey, prepareProxyPolicyMutation } from './proxy-policy.js';
import { prepareProxyRegistryMutation } from './proxy-registry.js';
import { validateProxyReservationEnvelope, validateProxyUsageEnvelope, proxyReservationFeatureKey,
  proxyUsageFeatureKey, prepareProxyReservation, prepareProxyUsageReceipt } from './proxy-reservations.js';

import { validateProxyCloseEnvelope, validateProxyExpireEnvelope, proxyCloseFeatureKey, proxyExpireFeatureKey,
  prepareProxyClose, prepareProxyExpiry } from './proxy-closure.js';

export function validateProxyPublication(value) {
  switch (value?.op) {
    case 'proxy_registry': return validateProxyOperationEnvelope(value);
    case 'proxy_policy': return validateProxyPolicy(value);
    case 'proxy_spend_reserve': return validateProxyReservationEnvelope(value);
    case 'proxy_record_usage': return validateProxyUsageEnvelope(value);
    case 'proxy_close_reservation': return validateProxyCloseEnvelope(value);
    case 'proxy_expire_reservation': return validateProxyExpireEnvelope(value);
    default: throw new ProxyValidationError('Unsupported proxy publication.');
  }
}

export async function proxyPublicationFeatureKey(value) {
  validateProxyPublication(value);
  switch (value.op) {
    case 'proxy_registry': return proxyRegistryFeatureKey(value);
    case 'proxy_policy': return proxyPolicyFeatureKey(value);
    case 'proxy_spend_reserve': return proxyReservationFeatureKey(value);
    case 'proxy_record_usage': return proxyUsageFeatureKey(value);
    case 'proxy_close_reservation': return proxyCloseFeatureKey(value);
    case 'proxy_expire_reservation': return proxyExpireFeatureKey(value);
  }
}

export function proxyPublicationParticipant(value) {
  const participant = value?.op === 'proxy_registry' ? value.intent?.provider_pubkey
    : value?.op === 'proxy_spend_reserve' ? value.authorization?.terms?.offer?.provider_pubkey
      : ['proxy_record_usage', 'proxy_close_reservation'].includes(value?.op) ? value.provider
        : value?.op === 'proxy_expire_reservation' ? value.buyer : null;
  return typeof participant === 'string' && /^[0-9a-f]{64}$/.test(participant) ? participant : null;
}

export async function prepareProxyPublication(ledger, value, context, verify) {
  validateProxyPublication(value);
  const read = key => ledger.get(key);
  switch (value.op) {
    case 'proxy_registry': return prepareProxyRegistryMutation(value, context, read, verify);
    case 'proxy_policy': return prepareProxyPolicyMutation(value, context, read);
    case 'proxy_spend_reserve': return prepareProxyReservation(ledger, value, context, verify);
    case 'proxy_record_usage': return prepareProxyUsageReceipt(ledger, value, context, verify);
    case 'proxy_close_reservation': return prepareProxyClose(ledger, value, context, verify);
    case 'proxy_expire_reservation': return prepareProxyExpiry(ledger, value, context, verify);
  }
}

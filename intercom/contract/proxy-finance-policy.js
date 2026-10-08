import { ProxyValidationError } from './proxy-protocol.js';

export const PROXY_FINANCE_POLICY_KEY = 'proxy/v1/finance-policy';
const need = (value, message) => { if (!value) throw new ProxyValidationError(message); };

// Operational publication limits are explicit admin policy. They do not change
// model context, prices, execution deadlines or the original payable outcomes.
export function validateProxyFinancePolicy(policy) {
  need(policy && typeof policy === 'object' && !Array.isArray(policy)
    && Object.keys(policy).sort().join('|') === ['enabled', 'max_checkpoints_per_reservation',
      'max_reservations_per_epoch', 'max_reservations_per_provider_epoch'].sort().join('|'), 'Invalid proxy finance policy.');
  need(typeof policy.enabled === 'boolean', 'Invalid proxy finance enablement.');
  for (const field of ['max_reservations_per_epoch', 'max_reservations_per_provider_epoch', 'max_checkpoints_per_reservation']) {
    need(Number.isSafeInteger(policy[field]) && policy[field] >= (field === 'max_checkpoints_per_reservation' ? 0 : 1),
      'Invalid proxy financial publication limit.');
  }
  return policy;
}

export async function prepareProxyFinancialAdmission(read, provider, epoch) {
  need(Number.isSafeInteger(epoch) && epoch > 0, 'Invalid proxy financial admission epoch.');
  const policy = validateProxyFinancePolicy(await read(PROXY_FINANCE_POLICY_KEY));
  need(policy.enabled, 'New proxy financial admission is disabled.');
  const writes = [];
  for (const [key, limit] of [
    ['proxy/v1/reservation-budget', policy.max_reservations_per_epoch],
    [`proxy/v1/reservation-budget/${provider}`, policy.max_reservations_per_provider_epoch],
  ]) {
    const previous = await read(key);
    need(previous === null || (Number.isSafeInteger(previous.epoch) && previous.epoch > 0
      && previous.epoch <= epoch && Number.isSafeInteger(previous.count) && previous.count >= 0),
    'Invalid proxy financial admission counter.');
    const count = (previous?.epoch === epoch ? previous.count : 0) + 1;
    need(Number.isSafeInteger(count) && count <= limit, 'Proxy financial admission quota reached.');
    writes.push({ key, value: { epoch, count } });
  }
  return { policy, writes };
}

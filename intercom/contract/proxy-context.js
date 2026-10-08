import b4a from 'b4a';
import { ProxyValidationError } from './proxy-protocol.js';

const hex = value => b4a.isBuffer(value) ? b4a.toString(value, 'hex') : String(value ?? '').toLowerCase();

// Modern settlement records use updated_epoch. Legacy fixtures/records may use
// epoch; a pending epoch has not completed and must not advance admission time.
export function proxyAppliedEpoch(state) {
  const epoch = state?.updated_epoch ?? state?.epoch ?? 0;
  if (!Number.isSafeInteger(epoch) || epoch < 0) throw new ProxyValidationError('Invalid canonical proxy epoch.');
  return epoch;
}

// Local protocol configuration supplies network identity. Epoch comes from the
// same canonical checkout used for admission, or the current consensus batch.
export function proxyRuntimeContext(peer, contractVersion, epoch) {
  const network = peer?.msbClient?.networkId;
  const context = {
    network_id: Number.isSafeInteger(network) && network > 0 ? String(network) : '',
    msb_bootstrap: hex(peer?.msbClient?.bootstrapHex),
    subnet_bootstrap: hex(peer?.config?.bootstrap ?? peer?.base?.key),
    contract_version: contractVersion,
    epoch,
  };
  if (!context.network_id || !/^[0-9a-f]{64}$/.test(context.msb_bootstrap)
    || !/^[0-9a-f]{64}$/.test(context.subnet_bootstrap)
    || !Number.isSafeInteger(contractVersion) || contractVersion < 1
    || !Number.isSafeInteger(epoch) || epoch < 0) {
    throw new ProxyValidationError('Proxy canonical network context is unavailable.');
  }
  return context;
}

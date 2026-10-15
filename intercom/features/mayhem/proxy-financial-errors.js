// Fixed diagnostics for read-only financial observation. These codes are not
// ledger operations and carry no authority to execute or settle a purchase.
const messages = Object.freeze({
  proxy_financial_pending: 'The accepted purchase is not yet visible in confirmed state.',
  proxy_financial_busy: 'The canonical financial service is busy.',
  proxy_financial_unavailable: 'The canonical financial service is temporarily unavailable.',
  proxy_financial_unknown: 'The financial service returned an unclassified failure.',
  proxy_financial_snapshot_changed: 'Canonical financial state changed during observation.',
  proxy_financial_invalid: 'The financial observation failed validation.',
  proxy_financial_rejected: 'The financial observation request was rejected.',
});
export const isProxyFinancialCode = code => typeof code === 'string'
  && Object.prototype.hasOwnProperty.call(messages, code);
export function proxyFinancialError(code, detail) {
  const safe = isProxyFinancialCode(code) ? code : 'proxy_financial_invalid';
  return Object.assign(new Error(detail ?? messages[safe]), { code: safe });
}
export function classifyProxyFinancialError(error) {
  if (isProxyFinancialCode(error?.code)) return proxyFinancialError(error.code);
  if (error?.code === 'proxy_snapshot_changed') return proxyFinancialError('proxy_financial_snapshot_changed');
  if (['proxy_snapshot_unavailable', 'REQUEST_TIMEOUT', 'SESSION_CLOSED', 'BLOCK_NOT_AVAILABLE',
    'ETIMEDOUT', 'ECONNRESET', 'ECONNREFUSED'].includes(error?.code)) {
    return proxyFinancialError('proxy_financial_unavailable');
  }
  // Unknown exceptions, malformed records and bad proofs must not be treated
  // as proof that funds are merely delayed. Never forward their arbitrary text.
  return proxyFinancialError('proxy_financial_invalid');
}
export function proxyFinancialHttpError(error) {
  const safe = classifyProxyFinancialError(error);
  const permanent = ['proxy_financial_invalid', 'proxy_financial_rejected'].includes(safe.code);
  return { status: permanent ? 400 : 503, body: { code: safe.code, error: safe.message } };
}

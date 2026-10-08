// Financial closure and remote execution knowledge are deliberately separate.
// Expiry can release a buyer's hold only under the signed policy; it never proves
// that an opaque upstream stopped or grants permission for a duplicate dispatch.
import { ProxyValidationError, proxyCanonicalSigningBytes } from './proxy-protocol.js';
import { validateProxyClosureBody, validateProxyExpiryBody, proxyClosureDigest, proxyExpiryDigest,
  verifyProxyClosure, verifyProxyExpiry } from './proxy-finance.js';
import { acceptedSpend, proxyReservationKeys, validateProxyBillingAnchor,
  validateProxyCanonicalReceiptHead } from './proxy-reservations.js';

const need = (ok, message) => { if (!ok) throw new ProxyValidationError(message); };
const checked = value => { if (value instanceof Error) throw new ProxyValidationError(value.message); return value; };
const copy = value => JSON.parse(JSON.stringify(value));
const shape = (value, fields) => need(value && typeof value === 'object' && !Array.isArray(value)
  && Object.keys(value).sort().join('|') === [...fields].sort().join('|'), 'Invalid proxy closure fields.');
const signature = value => need(typeof value === 'string' && /^[a-f0-9]{128}$/.test(value), 'Invalid proxy closure signature.');

export function validateProxyCloseEnvelope(value) {
  shape(value, ['op', 'provider', 'closure']);
  need(value.op === 'proxy_close_reservation' && /^[a-f0-9]{64}$/.test(value.provider), 'Invalid proxy closure operation.');
  shape(value.closure, ['body', 'buyer_sig', 'provider_sig']);
  validateProxyClosureBody(value.closure.body);
  signature(value.closure.buyer_sig); signature(value.closure.provider_sig);
  proxyCanonicalSigningBytes('proxy-close-bound', value);
}

export function validateProxyExpireEnvelope(value) {
  shape(value, ['op', 'buyer', 'expiry']);
  need(value.op === 'proxy_expire_reservation' && /^[a-f0-9]{64}$/.test(value.buyer), 'Invalid proxy expiry operation.');
  shape(value.expiry, ['body', 'buyer_sig']); validateProxyExpiryBody(value.expiry.body); signature(value.expiry.buyer_sig);
  proxyCanonicalSigningBytes('proxy-expire-bound', value);
}

export async function proxyCloseFeatureKey(value) {
  validateProxyCloseEnvelope(value); return `proxy/close/${await proxyClosureDigest(value.closure.body)}`;
}
export async function proxyExpireFeatureKey(value) {
  validateProxyExpireEnvelope(value); return `proxy/expire/${await proxyExpiryDigest(value.expiry.body)}`;
}

async function contextFor(ledger, body, context) {
  const accepted = await acceptedSpend(ledger, body.accepted_terms), terms = accepted.authorization.terms;
  for (const key of ['network_id', 'msb_bootstrap', 'subnet_bootstrap']) need(terms[key] === context[key], 'Proxy closure network mismatch.');
  return { accepted, terms };
}

async function closeUnfinalized(ledger, accepted, body, key, expired) {
  const t = accepted.authorization.terms;
  const anchorKey = ledger.receiptBillingKey(t.billing_id), anchor = await ledger.get(anchorKey);
  await validateProxyBillingAnchor(anchor, t, true);
  const head = await ledger.get(ledger.receiptHeadKey(t.billing_id, t.billing_attempt));
  if (head !== null) {
    await validateProxyCanonicalReceiptHead(ledger, head);
    need(head.accepted_terms === body.accepted_terms && !head.settlement_ready, 'A finalized proxy receipt cannot be waived or expired.');
    need(body.outcome !== 'not_executed', 'A checkpoint contradicts a claim of non-execution.');
  }
  need(await ledger.get(ledger.receiptConsumedKey(t.billing_id, t.billing_attempt)) === null, 'A consumed proxy attempt cannot be closed again.');
  const state = checked(await ledger.targetedSpendReservationState(t.buyer_pubkey, t.rail, t.reservation_id, t.session_id));
  need(state.kind === 'sharded' && state.session.lane === 'proxy' && state.session.accepted_terms === body.accepted_terms
    && !state.session.settlement_ready, 'Proxy closure does not match the live reserved session.');
  const reservationKey = ledger.receiptReservationKey(t.reservation_id), reservation = await ledger.get(reservationKey);
  need(reservation?.lane === 'proxy' && reservation.accepted_terms === body.accepted_terms, 'Proxy reservation identity is missing.');
  for (const field of ['billing_id', 'billing_attempt', 'billing_epoch', 'session_id', 'reservation_id', 'rail', 'payout_revision',
    'reservation_expires_after_epoch', 'reservation_receipt_grace_epochs']) need(reservation[field] === t[field], 'Proxy reservation identity differs.');
  need(reservation.user === t.buyer_pubkey && reservation.provider === t.offer.provider_pubkey, 'Proxy reservation parties differ.');
  const closeKey = ledger.receiptReservationCloseKey(t.reservation_id);
  need(await ledger.get(closeKey) === null, 'Proxy reservation closure already exists.');
  // A running checkpoint is not a payable final outcome. No synthetic receipt or
  // settlement allocation is manufactured to turn it into one.
  const closure = checked(ledger.prepareShardedTargetedReservationClosure({ summary: state.summary,
    session: state.session, reservation, head: null, closeRecordKey: closeKey,
    closedBy: expired ? t.buyer_pubkey : t.offer.provider_pubkey,
    closedByRole: expired ? 'user' : 'provider', at: body.at_ms,
    reason: expired ? 'proxy_expired_unresolved' : 'proxy_charge_waived' }));
  return { result: { accepted_terms: body.accepted_terms, billing_id: t.billing_id, billing_attempt: t.billing_attempt,
    released_au: closure.close_record.released_au, retained_au: '0', retry_safe: !expired }, writes: [
    { key: ledger.targetedSpendSummaryKey(t.buyer_pubkey, t.rail), value: closure.summary },
    { key: reservationKey, value: closure.reservation }, { key: closeKey, value: closure.close_record },
    { key: anchorKey, value: { ...anchor, reserved_au: '0', active_reservation_id: null, retry_blocked: expired, updated_at: key } },
    ...[state.sessionKey, state.sessionIndexKey, state.billingAttemptKey].map(key => ({ key, delete: true })),
  ] };
}

export async function prepareProxyClose(ledger, envelope, context, verify) {
  validateProxyCloseEnvelope(envelope); envelope = copy(envelope);
  const body = envelope.closure.body;
  const { accepted, terms } = await contextFor(ledger, body, context);
  need(envelope.provider === terms.offer.provider_pubkey, 'Proxy closure provider differs from accepted terms.');
  await verifyProxyClosure(envelope.closure, terms, verify);
  const resolutionKey = proxyReservationKeys.resolution(body.accepted_terms);
  const existing = await ledger.get(resolutionKey);
  if (existing !== null) {
    need(existing.type === 'proxy_execution_resolution' && existing.accepted_terms === body.accepted_terms
      && await proxyClosureDigest(existing.closure.body) === await proxyClosureDigest(body)
      && existing.closure.buyer_sig === envelope.closure.buyer_sig && existing.closure.provider_sig === envelope.closure.provider_sig, 'Proxy execution resolution cannot change.');
    return { duplicate: true, writes: [], result: copy(existing.result) };
  }
  const key = await proxyCloseFeatureKey(envelope), expiry = await ledger.get(proxyReservationKeys.expiry(body.accepted_terms));
  let plan;
  if (expiry !== null) {
    need(expiry.type === 'proxy_reservation_expiry' && expiry.accepted_terms === body.accepted_terms, 'Proxy expiry record differs.');
    await verifyProxyExpiry(expiry.expiry, terms, accepted.settlement_policy, context.epoch, verify);
    const close = await ledger.get(ledger.receiptReservationCloseKey(terms.reservation_id));
    need(close?.retained_au === '0' && close.released_au === terms.max_spend_au, 'Proxy expired hold closure differs.');
    const head = await ledger.get(ledger.receiptHeadKey(terms.billing_id, terms.billing_attempt));
    need(head === null || body.outcome !== 'not_executed', 'A checkpoint contradicts a claim of non-execution.');
    const anchorKey = ledger.receiptBillingKey(terms.billing_id), anchor = await ledger.get(anchorKey);
    await validateProxyBillingAnchor(anchor, terms, false);
    need(anchor.active_reservation_id === null && anchor.reserved_au === '0' && anchor.retry_blocked
      && anchor.spent_au === terms.prior_spend_au, 'Proxy expired billing state differs.');
    // Funds were already released. This later signed proof changes only whether
    // another attempt may be authorized; it never releases/debits funds again.
    const { execution: _unknown, ...expiredResult } = expiry.result;
    plan = { result: { ...expiredResult, released_au: '0', retry_safe: true }, writes: [
      { key: anchorKey, value: { ...anchor, retry_blocked: false, updated_at: key } },
    ] };
  } else plan = await closeUnfinalized(ledger, accepted, body, key, false);
  plan.result = { ...plan.result, ok: true, op: 'proxyCloseReservation', outcome: body.outcome };
  plan.writes.push({ key: resolutionKey, value: { type: 'proxy_execution_resolution', accepted_terms: body.accepted_terms,
    closure: envelope.closure, result: plan.result, recorded_at: key } });
  return { duplicate: false, ...plan };
}

export async function prepareProxyExpiry(ledger, envelope, context, verify) {
  validateProxyExpireEnvelope(envelope); envelope = copy(envelope);
  const body = envelope.expiry.body;
  const { accepted, terms } = await contextFor(ledger, body, context);
  need(envelope.buyer === terms.buyer_pubkey, 'Proxy expiry buyer differs from accepted terms.');
  await verifyProxyExpiry(envelope.expiry, terms, accepted.settlement_policy, context.epoch, verify);
  const expiryKey = proxyReservationKeys.expiry(body.accepted_terms), existing = await ledger.get(expiryKey);
  if (existing !== null) {
    need(existing.type === 'proxy_reservation_expiry' && existing.accepted_terms === body.accepted_terms, 'Proxy expiry record differs.');
    return { duplicate: true, writes: [], result: copy(existing.result) };
  }
  need(await ledger.get(proxyReservationKeys.resolution(body.accepted_terms)) === null, 'Proxy attempt is already resolved.');
  const key = await proxyExpireFeatureKey(envelope);
  const plan = await closeUnfinalized(ledger, accepted, body, key, true);
  plan.result = { ...plan.result, ok: true, op: 'proxyExpireReservation', execution: 'unknown' };
  plan.writes.push({ key: expiryKey, value: { type: 'proxy_reservation_expiry', accepted_terms: body.accepted_terms,
    expiry: envelope.expiry, result: plan.result, recorded_at: key } });
  return { duplicate: false, ...plan };
}

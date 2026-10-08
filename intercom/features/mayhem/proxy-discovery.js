// Read-only, authenticated canonical discovery. Snapshot cursors are stateless;
// incremental hydration uses Hyperbee's bounded-range B-tree diff, not ledger replay.
import b4a from 'b4a';
import PeerWallet from 'trac-wallet';
import { PROXY_CATALOG_PREFIX as CATALOG, PROXY_CATALOG_INDEX_PREFIX as INDEX } from '../../contract/proxy-discovery.js';
import { createProxyCanonicalReader, validateProxySnapshotProof } from './proxy-canonical-view.js';

export const PROXY_DISCOVERY_SERVICE = 'proxy_discovery';
export const PROXY_DISCOVERY_MAX_PAGE_BYTES = 128 * 1024;
const MAX_TOKEN_BYTES = 4096;
const READ_TIMEOUT_MS = 5000;
const kinds = new Set(['catalog', 'markets', 'memberships', 'offers', 'families', 'providers',
  'endpoints', 'metering', 'provider_status', 'admission_status', 'network']);
const hex = value => typeof value === 'string' && /^[0-9a-f]{64}$/.test(value);
const identifier = value => typeof value === 'string' && /^[a-z][a-z0-9_-]{0,63}$/.test(value);
const clone = value => JSON.parse(JSON.stringify(value));
const stable = value => JSON.stringify(value, (_, item) => item && typeof item === 'object' && !Array.isArray(item)
  ? Object.fromEntries(Object.keys(item).sort().map(key => [key, item[key]])) : item);
const keyHex = value => b4a.isBuffer(value) ? b4a.toString(value, 'hex') : value;
const bytes = value => b4a.byteLength(JSON.stringify(value));
const fail = (message, code = 'proxy_discovery_invalid') => { throw Object.assign(new Error(message), { code }); };
const object = value => value && typeof value === 'object' && !Array.isArray(value);
const allowed = (value, fields) => object(value) && Object.keys(value).every(key => fields.includes(key));
const tokenShape = value => value === null || (typeof value === 'string' && value.length <= MAX_TOKEN_BYTES && /^pdc1\.[A-Za-z0-9_-]+\.[0-9a-f]{128}$/.test(value));

export function normalizeProxyDiscoveryQuery(query) {
  if (!allowed(query, ['kind', 'filter', 'lookup', 'limit', 'cursor', 'since']) || !kinds.has(query.kind)) fail('Invalid proxy discovery query.');
  const result = { kind: query.kind, filter: query.filter ?? {}, lookup: query.lookup ?? null, limit: query.limit ?? 40,
    cursor: query.cursor ?? null, since: query.since ?? null };
  if (!Number.isSafeInteger(result.limit) || result.limit < 1 || result.limit > 100 ||
      !tokenShape(result.cursor) || !tokenShape(result.since) || (result.cursor && result.since) ||
      !allowed(result.filter, ['market_id', 'provider_pubkey', 'family_id', 'endpoint_family'])) fail('Invalid proxy discovery page or filter.');
  for (const [name, value] of Object.entries(result.filter)) {
    if (['market_id', 'provider_pubkey'].includes(name)) {
      if (!['memberships', 'offers'].includes(result.kind) || !hex(value)) fail('Invalid proxy market/provider filter.');
    } else if (result.kind !== 'markets' || (name === 'family_id' ? !identifier(value) : !['llm', 'decisions'].includes(value))) {
      fail('Invalid proxy family filter.');
    }
  }
  if (result.lookup !== null) {
    if (typeof result.lookup !== 'string' || Object.keys(result.filter).length || result.cursor || result.since || result.kind === 'catalog') fail('Invalid proxy direct lookup.');
    const parts = result.lookup.split('/');
    const valid = result.kind === 'families' ? identifier(result.lookup)
      : result.kind === 'network' ? result.lookup === 'current'
      : parts.length === (result.kind === 'memberships' ? 2 : result.kind === 'offers' ? 3 : 1) && parts.every(hex);
    if (!valid) fail('Invalid proxy lookup identity.');
  }
  return clone(result);
}

export function validateProxyDiscoveryRequest(request) {
  if (!object(request) || stable(Object.keys(request).sort()) !== stable(['query', 'request_nonce', 'requester']) ||
      !hex(request.requester) || !hex(request.request_nonce) || bytes(request) > 10_240) fail('Invalid proxy discovery request.');
  normalizeProxyDiscoveryQuery(request.query);
}

function rangeFor(query) {
  const { kind, filter: f, lookup } = query;
  if (lookup !== null) return { exact: `${CATALOG}${kind}/${lookup}` };
  if (kind === 'markets' && f.endpoint_family && f.family_id) return { prefix: `${INDEX}type-family/${f.endpoint_family}/${f.family_id}/`, indirect: true };
  if (kind === 'markets' && f.endpoint_family) return { prefix: `${INDEX}type/${f.endpoint_family}/`, indirect: true };
  if (kind === 'markets' && f.family_id) return { prefix: `${INDEX}family/${f.family_id}/`, indirect: true };
  if (['memberships', 'offers'].includes(kind)) {
    if (f.market_id && f.provider_pubkey && kind === 'memberships') return { exact: `${CATALOG}${kind}/${f.market_id}/${f.provider_pubkey}` };
    if (f.market_id) return { prefix: `${CATALOG}${kind}/${f.market_id}/${f.provider_pubkey ? `${f.provider_pubkey}/` : ''}` };
    if (f.provider_pubkey) return { prefix: `${INDEX}${kind}-provider/${f.provider_pubkey}/`, indirect: true };
  }
  return { prefix: `${CATALOG}${kind === 'catalog' ? '' : `${kind}/`}` };
}

const queryBinding = query => ({ kind: query.kind, filter: query.filter, lookup: query.lookup, limit: query.limit });
const network = context => { const { epoch, ...rest } = context; return rest; };
const signingBytes = value => b4a.from(`mayhem/proxy/discovery-cursor/v1\0${stable(value)}`);
const encode = value => b4a.toString(b4a.from(stable(value)), 'base64').replace(/=/g, '').replace(/\+/g, '-').replace(/\//g, '_');

export function createProxyDiscovery(peer, contractVersion, {
  now = Date.now, pageMaxAgeMs = 15 * 60_000, checkpointMaxAgeMs = 7 * 24 * 60 * 60_000,
  maxInFlight = 8,
} = {}) {
  if (![pageMaxAgeMs, checkpointMaxAgeMs, maxInFlight].every(value => Number.isSafeInteger(value) && value > 0) || maxInFlight > 64) {
    fail('Invalid proxy discovery configuration.');
  }
  const reader = createProxyCanonicalReader(peer, contractVersion);
  let active = 0;
  const signToken = value => `pdc1.${encode(value)}.${keyHex(peer.wallet.sign(signingBytes(value)))}`;
  const verifyToken = (token, kind, query, context, at) => {
    if (!tokenShape(token) || token === null) fail('Invalid proxy discovery cursor.');
    const [, body, signature] = token.split('.');
    let value;
    try { value = JSON.parse(b4a.toString(b4a.from(body.replace(/-/g, '+').replace(/_/g, '/'), 'base64'))); }
    catch { fail('Invalid proxy discovery cursor.'); }
    let verified = false;
    try { verified = PeerWallet.verify(b4a.from(signature, 'hex'), signingBytes(value), b4a.from(keyHex(peer.wallet.publicKey), 'hex')) === true; }
    catch { /* Rejected below. No request-supplied signer. */ }
    if (!verified || !object(value) || value.version !== 1 || value.kind !== kind ||
        stable(value.query) !== stable(queryBinding(query)) || stable(value.network) !== stable(network(context))) fail('Proxy discovery cursor does not match this query/network.');
    if (!Number.isSafeInteger(value.issued_at) || !Number.isSafeInteger(value.expires_at) || value.issued_at > at ||
        value.expires_at <= at || value.expires_at < value.issued_at) fail('Proxy discovery cursor expired; restart this traversal.', 'proxy_cursor_expired');
    validateProxySnapshotProof(value.proof);
    if (value.since !== null) validateProxySnapshotProof(value.since);
    const range = rangeFor(query);
    if (value.after !== null && (typeof value.after !== 'string' || !range.prefix || !value.after.startsWith(range.prefix) || value.after.length > 256)) fail('Invalid proxy discovery cursor position.');
    return value;
  };

  return async request => {
    validateProxyDiscoveryRequest(request);
    const query = normalizeProxyDiscoveryQuery(request.query);
    if (active >= maxInFlight) fail('Proxy discovery is busy; retry this read.', 'proxy_discovery_busy');
    active++;
    let current, previous, stream, timer;
    try {
      const at = now();
      current = await reader.pin();
      let page = null;
      let since = null;
      if (query.cursor) {
        page = verifyToken(query.cursor, 'page', query, current.context, at);
        const pinned = await reader.pin(page.proof);
        await current.view.close(); current = pinned;
        since = page.since;
      } else if (query.since) {
        since = verifyToken(query.since, 'checkpoint', query, current.context, at).proof;
      }
      if (since) {
        if (since.signed_length > current.proof.signed_length) fail('Proxy discovery checkpoint is newer than the selected snapshot.');
        previous = await reader.pin(since);
      }
      const range = rangeFor(query);
      const entries = [];
      let size = 0;
      let after = page?.after ?? null;
      let truncated = false;
      const add = entry => {
        const cost = bytes(entry);
        if (cost > PROXY_DISCOVERY_MAX_PAGE_BYTES) fail('Proxy discovery record exceeds the page bound.');
        if (entries.length >= query.limit || size + cost > PROXY_DISCOVERY_MAX_PAGE_BYTES) return false;
        entries.push(clone(entry)); size += cost; return true;
      };
      if (range.exact) {
        const value = await current.read(range.exact);
        if (value !== null || previous) add({ key: range.exact, value });
      } else {
        const options = { ...(after === null ? { gte: range.prefix } : { gt: after }),
          lt: `${range.prefix}\xff`, limit: query.limit + 1, timeout: READ_TIMEOUT_MS };
        stream = previous ? current.view.createDiffStream(previous.view, options) : current.view.createReadStream(options);
        timer = setTimeout(() => stream.destroy(Object.assign(new Error('Proxy discovery page read timed out.'), { code: 'proxy_discovery_timeout' })), READ_TIMEOUT_MS);
        timer.unref?.();
        for await (const raw of stream) {
          const node = previous ? raw.left ?? raw.right : raw;
          const value = previous ? raw.left?.value ?? null : raw.value;
          let entry = { key: node.key, value };
          if (range.indirect) {
            const target = (previous ? raw.left?.value ?? raw.right?.value : value)?.key;
            if (typeof target !== 'string' || !target.startsWith(`${CATALOG}${query.kind}/`) || target.length > 256) fail('Invalid proxy discovery index reference.');
            entry = { key: target, value: value === null ? null : await current.read(target) };
            if (value !== null && entry.value === null) fail('Proxy discovery index refers to a missing record.');
          }
          if (!add(entry)) { truncated = true; break; }
          after = node.key;
        }
      }
      current.assertCanonical(); previous?.assertCanonical();
      const base = { version: 1, network: network(current.context), query: queryBinding(query), proof: current.proof };
      // Partial pages never advance a hydration checkpoint. Apply all pages before
      // replacing a previous checkpoint, and revalidate eligibility before dispatch.
      const nextCursor = truncated ? signToken({ ...base, kind: 'page', since, after,
        issued_at: page?.issued_at ?? at, expires_at: page?.expires_at ?? at + pageMaxAgeMs }) : null;
      const checkpoint = truncated ? null : signToken({ ...base, kind: 'checkpoint', since: null, after: null,
        issued_at: at, expires_at: at + checkpointMaxAgeMs });
      return { ok: true, lane: 'proxy', schema_version: 1, request_nonce: request.request_nonce,
        query: queryBinding(query), context: current.context, proof: current.proof, base_proof: since,
        mode: since ? 'changes' : 'snapshot', entries, truncated, next_cursor: nextCursor, checkpoint };
    } finally {
      clearTimeout(timer);
      stream?.destroy();
      try { await previous?.view.close(); } finally { try { await current?.view.close(); } finally { active--; } }
    }
  };
}

import http from 'http';
import { applyCors } from '../trac/trac-peer/rpc/cors.js';
import { DEFAULT_MAX_BODY_BYTES } from '../trac/trac-peer/rpc/constants.js';
import { routes } from '../trac/trac-peer/rpc/routes/index.js';
import { contractFeature } from '../trac/trac-peer/rpc/services.js';
import { readJsonBody } from '../trac/trac-peer/rpc/utils/body.js';

const isObject = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);
const normalizeKey = (value) => String(value ?? '').trim().toLowerCase();

const safeBooleanCall = (target, method) => {
  try {
    return typeof target?.[method] === 'function' ? target[method]() === true : null;
  } catch {
    return null;
  }
};

export const adminWriterDiagnostics = (peer) => {
  const base = peer?.base;
  const writer = base?.localWriter;
  const writerCore = writer?.core;
  const applyState = base?._applyState;
  const system = applyState?.system;
  const applyStage = peer?.contract?.instance?._mayhemApplyStage;
  const viewCore = base?.view?.core;
  return {
    contract: {
      apply_stage: typeof applyStage === 'string' ? applyStage : null,
    },
    base: {
      writable: base?.writable === true,
      is_indexer: base?.isIndexer === true,
      opened: base?.opened === true,
      closing: base?.closing === true,
      paused: base?.paused === true,
      caught_up: base?._caughtup === true,
      draining: base?._draining === true,
      advancing: base?._advancing != null,
      appending_count: Array.isArray(base?._appending) ? base._appending.length : 0,
      appended_count: Number(base?._appended ?? 0),
      signed_length: Number(base?.signedLength ?? 0),
      length: Number(base?.length ?? 0),
      local_length: Number(base?.local?.length ?? 0),
      needs_wakeup: base?._needsWakeup === true,
      wakeup_hint_count: Number(base?._wakeupHints?.size ?? 0),
    },
    view: viewCore ? {
      writable: viewCore.writable === true,
      opened: viewCore.opened === true,
      closing: viewCore.closing === true,
      length: Number(viewCore.length ?? 0),
      fork: Number(viewCore.fork ?? 0),
      signed_length: Number(viewCore.signedLength ?? 0),
      upgrading: viewCore?.core?.upgrading === true,
    } : null,
    local_writer: writer ? {
      removed: writer.isRemoved === true,
      active_indexer: writer.isActiveIndexer === true,
      idle: safeBooleanCall(writer, 'idle'),
      flushed: safeBooleanCall(writer, 'flushed'),
      length: Number(writer.length ?? 0),
      available: Number(writer.available ?? 0),
      seen_length: Number(writer.seenLength ?? 0),
      core_length: Number(writerCore?.length ?? 0),
      core_writable: writerCore?.writable === true,
      core_opened: writerCore?.opened === true,
      core_upgrading: writerCore?.core?.upgrading === true,
    } : null,
    apply_state: applyState ? {
      opened: applyState.opened === true,
      applying: applyState.applying === true,
      local_indexer: safeBooleanCall(applyState, 'isLocalIndexer'),
      local_pending_indexer: safeBooleanCall(applyState, 'isLocalPendingIndexer'),
      indexed_length: Number(applyState.indexedLength ?? 0),
      indexer_count: Array.isArray(system?.indexers) ? system.indexers.length : 0,
      pending_indexer_count: Array.isArray(system?.pendingIndexers)
        ? system.pendingIndexers.length
        : 0,
      indexer_lengths: Array.isArray(system?.indexers)
        ? system.indexers.map((entry) => Number(entry?.length ?? 0))
        : [],
      indexers_updated: system?.indexerUpdate === true,
    } : null,
  };
};

export async function submitMayhemFeature(peer, body) {
  if (!isObject(body)) throw new Error('Missing JSON body.');
  const feature = String(body.feature ?? 'mayhem').trim();
  const key = String(body.key ?? '').trim();
  if (!feature) throw new Error('Missing feature.');
  if (!key) throw new Error('Missing key.');
  if (key.length > 256) throw new Error('Invalid key. Expected at most 256 characters.');
  if (!isObject(body.value)) throw new Error('Invalid value. Expected an object.');

  const registered = peer.protocol?.instance?.features?.[feature];
  if (!registered || typeof registered.append !== 'function') {
    throw new Error(`Invalid feature ${feature}.`);
  }

  const admin = await peer.base?.view?.get('admin');
  const adminKey = normalizeKey(admin?.value);
  const selfKey = normalizeKey(peer.wallet?.publicKey);
  const isAdminWriter = peer.base?.writable === true && (!adminKey || adminKey === selfKey);
  if (isAdminWriter) {
    const watchdog = setTimeout(() => {
      console.error(
        'Mayhem admin feature append stalled:',
        JSON.stringify(adminWriterDiagnostics(peer))
      );
    }, 5_000);
    try {
      return await contractFeature(peer, { feature, key, value: body.value });
    } finally {
      clearTimeout(watchdog);
    }
  }
  if (feature !== 'mayhem' || typeof registered.relay !== 'function') {
    throw new Error('Peer subnet is not writable.');
  }
  return await registered.relay(key, body.value);
}

export async function requestStripeCheckout(peer, body) {
  if (!isObject(body)) throw new Error('Missing JSON body.');
  if (!isObject(body.payload) || typeof body.payload.who !== 'string' || !body.payload.who.trim()) {
    throw new Error('Missing who.');
  }
  const registered = peer.protocol?.instance?.features?.mayhem;
  if (!registered || typeof registered.requestService !== 'function') {
    throw new Error('Mayhem service relay is not ready.');
  }
  return await registered.requestService('stripe_checkout', body);
}

export async function requestStripeConnect(peer, service, body) {
  if (!isObject(body)) throw new Error('Missing JSON body.');
  if (!isObject(body.payload) ||
      typeof body.payload.provider !== 'string' || !body.payload.provider.trim()) {
    throw new Error('Missing provider.');
  }
  if (!['stripe_connect_onboard', 'stripe_connect_status'].includes(service)) {
    throw new Error('Invalid Stripe Connect service.');
  }
  const registered = peer.protocol?.instance?.features?.mayhem;
  if (!registered || typeof registered.requestService !== 'function') {
    throw new Error('Mayhem service relay is not ready.');
  }
  return await registered.requestService(service, body);
}

const errorResponse = (error) => {
  if (error?.code === 'BODY_TOO_LARGE') return [413, error.message];
  if (error?.code === 'BAD_JSON') return [400, error.message];
  const message = String(error?.message || '');
  if (/^(Missing|Invalid|Empty|Peer subnet is not writable)/.test(message)) return [400, message];
  if (message.startsWith('Mayhem feature relay') || message.startsWith('Mayhem service relay')) {
    return [503, message];
  }
  return [500, 'An internal error occurred.'];
};

export const createServer = (
  peer,
  { maxBodyBytes = DEFAULT_MAX_BODY_BYTES, allowOrigin = '*' } = {}
) => {
  const sortedRoutes = [...routes].sort((a, b) => b.path.length - a.path.length);
  return http.createServer({}, async (req, res) => {
    const respond = (code, payload) => {
      if (res.headersSent) return;
      res.writeHead(code, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify(payload ?? {}));
    };

    req.on('error', (error) => {
      console.error('RPC request stream error:', error);
      respond(500, { error: 'Request stream error.' });
    });
    if (applyCors(req, res, { allowOrigin })) return;

    const requestPath = (req.url || '/').split('?')[0];
    try {
      if (req.method === 'POST' && requestPath === '/v1/contract/feature') {
        const body = await readJsonBody(req, { maxBytes: maxBodyBytes });
        return respond(200, await submitMayhemFeature(peer, body));
      }
      if (req.method === 'POST' && requestPath === '/v1/payment/stripe/checkout') {
        const body = await readJsonBody(req, { maxBytes: maxBodyBytes });
        return respond(200, await requestStripeCheckout(peer, body));
      }
      if (req.method === 'POST' && requestPath === '/v1/payment/stripe/connect/onboard') {
        const body = await readJsonBody(req, { maxBytes: maxBodyBytes });
        return respond(200, await requestStripeConnect(peer, 'stripe_connect_onboard', body));
      }
      if (req.method === 'POST' && requestPath === '/v1/payment/stripe/connect/status') {
        const body = await readJsonBody(req, { maxBytes: maxBodyBytes });
        return respond(200, await requestStripeConnect(peer, 'stripe_connect_status', body));
      }
      for (const route of sortedRoutes) {
        if (req.method !== route.method || requestPath !== route.path) continue;
        await route.handler({ req, res, respond, peer, maxBodyBytes });
        return;
      }
      respond(404, { error: 'Not Found' });
    } catch (error) {
      const [code, message] = errorResponse(error);
      if (code === 500) console.error('RPC handler error:', error);
      respond(code, { error: message });
    }
  });
};

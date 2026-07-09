import http from 'http';
import { applyCors } from '../trac/trac-peer/rpc/cors.js';
import { DEFAULT_MAX_BODY_BYTES } from '../trac/trac-peer/rpc/constants.js';
import { routes } from '../trac/trac-peer/rpc/routes/index.js';
import { contractFeature } from '../trac/trac-peer/rpc/services.js';
import { readJsonBody } from '../trac/trac-peer/rpc/utils/body.js';

const isObject = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);
const normalizeKey = (value) => String(value ?? '').trim().toLowerCase();

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
    return await contractFeature(peer, { feature, key, value: body.value });
  }
  if (feature !== 'mayhem' || typeof registered.relay !== 'function') {
    throw new Error('Peer subnet is not writable.');
  }
  return await registered.relay(key, body.value);
}

const errorResponse = (error) => {
  if (error?.code === 'BODY_TOO_LARGE') return [413, error.message];
  if (error?.code === 'BAD_JSON') return [400, error.message];
  const message = String(error?.message || '');
  if (/^(Missing|Invalid|Empty|Peer subnet is not writable)/.test(message)) return [400, message];
  if (message.startsWith('Mayhem feature relay')) return [503, message];
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

import http from 'http';
import crypto from 'crypto';
import b4a from 'b4a';
import fs from 'fs';
import { readJsonBody } from '../trac/trac-peer/rpc/utils/body.js';
import { createProxyFamilyAdmin } from '../features/mayhem/proxy-family-admin.js';

export function readFamilyAdminToken(file) {
  if (typeof file !== 'string' || !file) throw new Error('Family admin token file is required.');
  const info = fs.lstatSync(file);
  if (!info.isFile() || info.isSymbolicLink() || info.size > 257 || (info.mode & 0o077) !== 0) throw new Error('Family admin token file must be private.');
  const token = fs.readFileSync(file, 'utf8').trim(); validateToken(token); return token;
}
function validateToken(token) {
  if (typeof token !== 'string' || !/^[\x21-\x7e]{32,256}$/.test(token)) throw new Error('Invalid family admin credential configuration.');
}
export function familyAdminListenOptions(env) {
  if (env.MAYHEM_PROXY_FAMILY_ADMIN !== '1') return null;
  const host = env.MAYHEM_PROXY_FAMILY_ADMIN_HOST || '127.0.0.1';
  if (!['127.0.0.1', '::1'].includes(host) && env.MAYHEM_PROXY_FAMILY_ADMIN_ALLOW_REMOTE !== '1') throw new Error('Family admin remote bind requires explicit configuration.');
  const port = Number(env.MAYHEM_PROXY_FAMILY_ADMIN_PORT || '5003');
  if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error('Invalid family admin port.');
  return { host, port };
}
/** Separate from the general RPC server. No generic routes, CORS, redirect or
 * anonymous loopback exception. Operator terminates TLS before a remote bind. */
export function createFamilyAdminServer(feature, { token, contractVersion }) {
  validateToken(token);
  const expected = b4a.from(`Bearer ${token}`), adapter = createProxyFamilyAdmin(feature, contractVersion);
  let active = 0;
  const server = http.createServer(async (req, res) => {
    const respond = (status, value) => {
      if (res.headersSent || res.destroyed) return;
      res.writeHead(status, { 'Content-Type': 'application/json', 'Cache-Control': 'private, no-store', 'X-Content-Type-Options': 'nosniff' });
      res.end(JSON.stringify(value));
    };
    const supplied = b4a.from(typeof req.headers.authorization === 'string' ? req.headers.authorization : '');
    if (supplied.length !== expected.length || !crypto.timingSafeEqual(supplied, expected)) return respond(401, { code: 'proxy_family_admin_unauthorized' });
    if (req.headers.origin || req.method !== 'POST' || !['/v1/admin/proxy/families/preview', '/v1/admin/proxy/families/register'].includes(req.url)) return respond(404, { code: 'proxy_family_admin_not_found' });
    if (active >= 2) return respond(503, { code: 'proxy_family_admin_busy' });
    active++;
    const deadline = setTimeout(() => { respond(503, { code: 'proxy_family_admin_timeout' }); if (!req.complete) req.destroy(); }, 5_000);
    try {
      if (!/^application\/json(?:;|$)/i.test(req.headers['content-type'] || '')) return respond(400, { code: 'proxy_family_admin_invalid' });
      const body = await readJsonBody(req, { maxBytes: 4096 });
      const result = await (req.url.endsWith('/preview') ? adapter.preview(body) : adapter.submit(body));
      respond(result.status === 'pending' ? 202 : 200, result);
    } catch (error) {
      const status = error.status ?? (['BAD_JSON', 'BODY_TOO_LARGE'].includes(error.code) ? 400 : 503);
      respond(status, { code: /^proxy_family_admin_[a-z_]+$/.test(error.code ?? '') ? error.code : 'proxy_family_admin_unavailable' });
    } finally { clearTimeout(deadline); active--; }
  });
  server.requestTimeout = 5_000; server.headersTimeout = 5_000; server.keepAliveTimeout = 1_000;
  return server;
}

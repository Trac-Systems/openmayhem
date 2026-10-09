// Opt-in SITE integration fixture: real local Autobase/Hyperbee + canonical
// contract and private admin HTTP. The read-only Gateway projection is a fixture
// adapter over that same signed view; this is not a running Rust gateway.
import { createServer } from 'node:http';
import { familyAdminFixture } from './proxy-family-admin-fixture.mjs';
import { createFamilyAdminServer } from '../../src/proxy-family-admin-http.js';
import { createProxyCanonicalReader } from '../../features/mayhem/proxy-canonical-view.js';
import { CONTRACT_VERSION } from '../../contract/contract.js';
const cleanup = [], f = await familyAdminFixture({ after: action => cleanup.push(action) });
const token = 'public-fixture-family-admin-token-00000000';
const admin = createFamilyAdminServer(f.feature, { token, contractVersion: CONTRACT_VERSION });
const reader = createProxyCanonicalReader(f.peer, CONTRACT_VERSION);
const gateway = createServer(async (request, response) => {
  const reply = (status, value) => { response.writeHead(status, { 'content-type': 'application/json' }); response.end(JSON.stringify(value)); };
  if (request.headers.authorization !== 'Bearer public-canonical-family-read-fixture') return reply(401, {});
  if (request.method !== 'POST' || request.url !== '/v1/proxy/families/lookup') return reply(404, {});
  let snapshot;
  try {
    const chunks = []; for await (const chunk of request) chunks.push(chunk);
    const body = JSON.parse(Buffer.concat(chunks));
    if (body.schema_version !== 1 || !Array.isArray(body.family_ids) || body.family_ids.length > 96) return reply(400, {});
    snapshot = await reader.pin(); const { epoch, ...network } = snapshot.context;
    const families = [];
    for (const family_id of body.family_ids) {
      const value = await snapshot.read(`proxy/v1/family/${family_id}`);
      if (value === null) return reply(404, { error: { code: 'proxy_family_not_found' } });
      families.push({ family_id, ...value });
    }
    snapshot.assertCanonical(); const now = Date.now();
    reply(200, { schema_version: 1, network, snapshot: `local-canonical-fixture:${snapshot.proof.signed_length}`, proof: snapshot.proof,
      observed_at_ms: now, expires_at_ms: now + 60_000, families });
  } catch { reply(503, {}); } finally { await snapshot?.view.close(); }
});
await new Promise(resolve => admin.listen(0, '127.0.0.1', resolve));
await new Promise(resolve => gateway.listen(0, '127.0.0.1', resolve));
console.log(JSON.stringify({ ready: true, admin_url: `http://127.0.0.1:${admin.address().port}`, gateway_url: `http://127.0.0.1:${gateway.address().port}`,
  network: f.network, admin_public_key: f.admin.publicKey }));
let closing = false;
async function close() {
  if (closing) return; closing = true;
  for (const server of [admin, gateway]) await new Promise(resolve => { server.closeAllConnections(); server.close(resolve); });
  for (const action of cleanup.reverse()) await action();
  process.exit(0);
}
process.stdin.resume(); process.stdin.on('data', () => void close()); process.stdin.on('end', () => void close());
process.on('SIGTERM', () => void close());

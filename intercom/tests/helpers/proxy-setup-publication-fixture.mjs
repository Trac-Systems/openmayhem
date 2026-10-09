// Local acceptance only: real signed Autobase view, canonical contract/gate and
// durable publication journal. The owned provider-read facade replaces remote
// service transport. Issuer signatures attest synthetic test allocations only.
import { createServer } from 'node:http';
import crypto from 'node:crypto';
import b4a from 'b4a';
import { familyAdminFixture } from './proxy-family-admin-fixture.mjs';
import { createProxyCanonicalSnapshot } from '../../features/mayhem/proxy-canonical-view.js';
import { readProxyProviderState } from '../../features/mayhem/proxy-provider-state.js';
import { proxyAdmissionSigningBytes, proxyOperationDigest } from '../../contract/proxy-protocol.js';
import { submitMayhemFeature } from '../../src/rpc.js';
import { CONTRACT_VERSION } from '../../contract/contract.js';

const cleanup = [], f = await familyAdminFixture({ after: fn => cleanup.push(fn) });
const snapshot = createProxyCanonicalSnapshot(f.peer, CONTRACT_VERSION);
let configured = false, submissions = 0, permits = 0, hideAfterSubmit = false, hidden = false, corrupt = false;
const server = createServer(async (request, response) => {
  const reply = (status, body) => { response.writeHead(status, { 'content-type': 'application/json', 'cache-control': 'no-store' }); response.end(JSON.stringify(body)); };
  try {
    let bytes = Buffer.alloc(0);
    for await (const part of request) { bytes = Buffer.concat([bytes, part]); if (bytes.length > 131072) throw new Error('fixture body bound'); }
    const body = bytes.length ? JSON.parse(bytes) : {};
    if (request.url === '/fixture/configure') {
      if (configured) throw new Error('fixture already configured'); configured = true;
      const market = body.market;
      for (const action of [
        { kind: 'set_family', family_id: market.model.family_id, label: 'Synthetic setup family', enabled: true },
        { kind: 'set_metering', policy_hash: market.metering.policy_hash, policy: { enabled: true, units: market.metering.units } },
        ...market.endpoints.map(endpoint => ({ kind: 'set_endpoint', contract_hash: endpoint.contract_hash, policy: {
          enabled: true, endpoint: endpoint.endpoint, family: market.family, max_context: 262144,
          ctx_brackets: [...new Set(body.offers.map(offer => offer.ctx_bracket))].sort(),
          outcome_classes: [...new Set(body.offers.map(offer => offer.outcome_class))].sort(),
        } })),
      ]) { const result = await f.policy(action); if (result.ok !== true) throw new Error('fixture policy rejected'); }
      await f.base.append({ type: 'seed', entries: [...f.storage.values] }); await f.base.update();
      return reply(200, { configured: true });
    }
    if (request.url === '/fixture/permit') {
      permits++;
      const intent = body.intent, provider = intent.provider_pubkey;
      const hash = name => crypto.createHash('sha256').update(`${provider}:${name}`).digest('hex');
      const permit = { ...f.permit, ...f.network, provider_pubkey: provider, issuer_pubkey: f.issuer.publicKey,
        entitlement_id: hash('entitlement'), invoice_commitment: hash('invoice'), evidence_commitment: hash('evidence'), nonce: hash('nonce'),
        rail: body.rail, initial_operation_digest: await proxyOperationDigest(intent) };
      return reply(200, { permit, issuer_signature: b4a.toString(f.issuer.wallet.sign(proxyAdmissionSigningBytes(permit)), 'hex') });
    }
    if (request.url === '/fixture/mode') {
      hideAfterSubmit = body.hide_after_submit === true; hidden = body.hidden === true; corrupt = body.corrupt === true;
      return reply(200, {});
    }
    if (request.url === '/fixture/status') {
      const view = f.base.view.checkout(f.base.view.core.signedLength); await view.ready();
      try {
        const provider = body.provider ? (await view.get(`proxy/v1/provider/${body.provider}`))?.value ?? null : null;
        return reply(200, { submissions, permits, appends: f.calls, pending: f.journal.list().length, provider,
          native_balance: (await view.get('bal/existing-customer'))?.value,
          native_payout: (await view.get('payout/epoch/542'))?.value,
          model_calls: 0, financial_collection: false });
      } finally { await view.close(); }
    }
    if (request.url === '/v1/proxy/provider-state') {
      if (hidden) return reply(503, {});
      const value = await readProxyProviderState({ request: { ...body, requester: body.provider_pubkey }, withCanonicalSnapshot: snapshot });
      if (corrupt) value.initial_operation_digest = 'f'.repeat(64);
      return reply(200, value);
    }
    if (request.url === '/v1/contract/feature') {
      submissions++;
      const result = await submitMayhemFeature(f.peer, body);
      if (hideAfterSubmit) { hidden = true; hideAfterSubmit = false; response.destroy(); return; }
      return reply(200, result);
    }
    reply(404, {});
  } catch (error) { reply(409, { fixture_error: String(error.message) }); }
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
console.log(JSON.stringify({ ready: true, url: `http://127.0.0.1:${server.address().port}`, network: f.network }));
let closing = false;
async function close() {
  if (closing) return; closing = true;
  await new Promise(resolve => { server.closeAllConnections(); server.close(resolve); });
  for (const fn of cleanup.reverse()) await fn();
  process.exit(0);
}
process.stdin.resume(); process.stdin.on('data', () => void close()); process.stdin.on('end', () => void close());
process.on('SIGTERM', () => void close());

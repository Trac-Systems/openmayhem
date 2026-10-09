import assert from 'node:assert/strict';
import test from 'node:test';
import { randomUUID } from 'node:crypto';
import { familyAdminFixture } from './helpers/proxy-family-admin-fixture.mjs';
import { createProxyFamilyAdmin, familyIntentNonce } from '../features/mayhem/proxy-family-admin.js';
import { createFamilyAdminServer, familyAdminListenOptions } from '../src/proxy-family-admin-http.js';
import { CONTRACT_VERSION } from '../contract/contract.js';
const token = 'public-fixture-family-admin-token-00000000';
async function server(t) {
  const f = await familyAdminFixture(t), http = createFamilyAdminServer(f.feature, { token, contractVersion: CONTRACT_VERSION });
  await new Promise(resolve => http.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise(resolve => { http.closeAllConnections(); http.close(resolve); }));
  const url = `http://127.0.0.1:${http.address().port}/v1/admin/proxy/families`;
  return { ...f, f, send: async (path, body, credential = token, headers = {}) => {
    const response = await fetch(`${url}/${path}`, { method: 'POST', headers: { authorization: `Bearer ${credential}`, 'content-type': 'application/json', ...headers }, body: JSON.stringify(body) });
    return { status: response.status, body: await response.json(), cache: response.headers.get('cache-control') };
  } };
}
function operation(intent) { const operation_id = randomUUID(); return { schema_version: 1, intent, operation_id, nonce: familyIntentNonce(intent, operation_id) }; }
const preview = id => ({ schema_version: 1, family: { family_id: id, label: 'New test family' } });

test('private family service is disabled by default, explicitly bound and authenticated', async t => {
  assert.equal(familyAdminListenOptions({}), null);
  assert.deepEqual(familyAdminListenOptions({ MAYHEM_PROXY_FAMILY_ADMIN: '1' }), { host: '127.0.0.1', port: 5003 });
  assert.throws(() => familyAdminListenOptions({ MAYHEM_PROXY_FAMILY_ADMIN: '1', MAYHEM_PROXY_FAMILY_ADMIN_HOST: '0.0.0.0' }));
  const f = await server(t), before = f.base.local.length;
  assert.equal((await f.send('preview', preview('new_family'), '')).status, 401);
  assert.equal((await f.send('preview', preview('new_family'), token, { origin: 'https://foreign.invalid' })).status, 404);
  assert.equal((await f.send('other', preview('new_family'))).status, 404);
  assert.equal((await f.send('preview', { ...preview('new_family'), enabled: true })).status, 400);
  assert.equal((await f.send('preview', preview('new_family'))).cache, 'private, no-store');
  assert.equal(f.base.local.length, before);
});

test('actual canonical application and lost HTTP acknowledgement recover the original after head advance/restart', async t => {
  const f = await server(t), before = f.base.local.length;
  const p = await f.send('preview', preview('fictional_family'));
  assert.equal(p.status, 200, JSON.stringify(p.body));
  const body = operation(p.body.intent), applied = await f.send('register', body);
  assert.equal(applied.status, 200, JSON.stringify(applied.body)); assert.equal(applied.body.status, 'applied');
  assert.match(applied.body.result_key, /^fr\/[0-9a-f]{128}$/);
  assert.deepEqual((await f.base.view.get('proxy/v1/family/fictional_family')).value, { enabled: true, label: 'New test family' });
  assert.equal(f.base.local.length, before + 1);
  const next = await f.send('preview', preview('later_family'));
  assert.equal((await f.send('register', operation(next.body.intent))).body.status, 'applied');
  const originalKey = applied.body.result_key, finalLength = f.base.local.length;
  assert.equal((await f.send('register', body)).body.result_key, originalKey);
  await f.f.reopen();
  const recovered = await createProxyFamilyAdmin(f.f.feature, CONTRACT_VERSION).submit(body);
  assert.equal(recovered.result_key, originalKey); assert.equal(recovered.status, 'applied');
  assert.equal(f.f.base.local.length, finalLength);
  assert.deepEqual((await f.f.base.view.get('payout/epoch/542')).value, { status: 'prepared', native: true });
});

test('altered nonce/intent, wrong network/admin, stale revision and existing disabled family never append', async t => {
  const f = await server(t), p = await f.send('preview', preview('refused_family')), body = operation(p.body.intent);
  const before = f.base.local.length;
  for (const change of [b => { b.intent.family.label = 'Changed'; }, b => { b.intent.admin = 'f'.repeat(64); },
    b => { b.intent.network.network_id = 'foreign'; }, b => { b.nonce = 'f'.repeat(64); }]) {
    const altered = structuredClone(body); change(altered); assert.equal((await f.send('register', altered)).status, 400);
  }
  const foreign = structuredClone(body); foreign.intent.network.network_id = 'foreign'; foreign.nonce = familyIntentNonce(foreign.intent, foreign.operation_id);
  assert.equal((await f.send('register', foreign)).status, 409);
  const stale = structuredClone(body); stale.intent.expected_policy_revision++; stale.nonce = familyIntentNonce(stale.intent, stale.operation_id);
  assert.equal((await f.send('register', stale)).status, 409);
  assert.equal(f.base.local.length, before);
  await f.base.append({ type: 'seed', entries: [['proxy/v1/family/refused_family', { enabled: false, label: 'Retained' }]] });
  assert.equal((await f.send('register', body)).status, 409);
  assert.equal((await f.send('preview', preview('refused_family'))).status, 409);
});

test('uncertain append retains exact journal nonce and retries without second append', async t => {
  const f = await server(t), p = await f.send('preview', preview('uncertain_family')), body = operation(p.body.intent);
  f.f.dropFeatures();
  assert.equal((await f.send('register', body)).body.status, 'pending');
  const retained = f.f.journal.list()[0]; assert.equal(retained.nonce, body.nonce);
  const before = f.base.local.length;
  assert.equal((await f.send('register', body)).body.status, 'pending');
  assert.equal(f.base.local.length, before);
  const other = operation(body.intent); assert.equal((await f.send('register', other)).status, 409);
  assert.equal(f.f.journal.list()[0].nonce, body.nonce);
});

test('revoked canonical writer cannot preview, append or recover through this private adapter', async t => {
  const f = await server(t), p = await f.send('preview', preview('revoked_family')), body = operation(p.body.intent);
  await f.base.append({ type: 'seed', entries: [['admin', 'f'.repeat(64)]] });
  const before = f.base.local.length;
  assert.equal((await f.send('register', body)).status, 503); assert.equal((await f.send('preview', preview('other_family'))).status, 503);
  assert.equal(f.base.local.length, before);
});

test('signed result must bind the original policy intent, not merely its nonce or family existence', async t => {
  const f = await server(t), p = await f.send('preview', preview('binding_family')), body = operation(p.body.intent);
  const applied = await f.send('register', body), resultKey = applied.body.result_key;
  const result = (await f.base.view.get(resultKey)).value;
  await f.base.append({ type: 'seed', entries: [[resultKey, { ...result, result: { ...result.result, operation_key: `proxy/policy/${'f'.repeat(64)}` } }]] });
  const before = f.base.local.length;
  const recovery = await f.send('register', body);
  assert.equal(recovery.status, 503); assert.equal(recovery.body.code, 'proxy_family_admin_result_invalid');
  assert.equal(f.base.local.length, before);
});

test('current canonical administrator is rechecked after durable intent persistence, before append', async t => {
  const f = await server(t), p = await f.send('preview', preview('demoted_before_append')), body = operation(p.body.intent);
  const put = f.f.journal.put.bind(f.f.journal);
  f.f.journal.put = async entry => { await put(entry); await f.base.append({ type: 'seed', entries: [['admin', 'f'.repeat(64)]] }); };
  assert.equal((await f.send('register', body)).status, 503);
  assert.equal((await f.base.view.get('proxy/v1/family/demoted_before_append')), null);
  assert.equal(f.f.calls, 0);
});

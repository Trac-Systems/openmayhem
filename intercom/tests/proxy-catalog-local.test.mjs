import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { openLocalCatalog } from './helpers/proxy-catalog-local.mjs';

function root(t) {
  const directory = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-local-catalog-')));
  fs.chmodSync(directory, 0o700);
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  return directory;
}
async function catalog(local) {
  const response = await fetch(`${local.metadata.rpc_url}/proxy/discovery`, { method: 'POST', headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ query: { kind: 'catalog', filter: {}, lookup: null, limit: 100, cursor: null, since: null } }) });
  assert.equal(response.status, 200); return response.json();
}
test('local test catalog applies real signed publications and preserves identity/append count across restart', async t => {
  const directory = root(t);
  let local = await openLocalCatalog(directory);
  try {
    const page = await catalog(local);
    assert.equal(page.ok, true); assert.match(page.checkpoint, /^pdc1\./);
    const offers = page.entries.filter(row => row.key.startsWith('proxy/v1/catalog/offers/'));
    assert.equal(offers.length, 1); assert.equal(offers[0].value.active, true);
    assert.equal(page.entries.find(row => row.key.startsWith('proxy/v1/catalog/markets/')).value.model.model_id, 'LOCAL TEST ONLY / catalog acceptance');
    assert.equal((await local.base.view.get('local-test/catalog')).value.test_only, true);
    assert.equal(await local.base.view.get('proxy/v1/finance-policy'), null);
    const network = local.metadata.network; const length = local.base.local.length;
    const plan = structuredClone(local.plan);
    await assert.rejects(openLocalCatalog(directory));
    for (const url of ['/v1/contract/feature', '/v1/proxy/financial-state', '/v1/status']) {
      assert.equal((await fetch(new URL(url, local.metadata.rpc_url), { method: 'POST', body: '{}' })).status, 404);
    }
    assert.equal(local.base.local.length, length, 'discovery/blocked endpoints cannot append');
    await local.close(); local = await openLocalCatalog(directory);
    assert.deepEqual(local.metadata.network, network); assert.deepEqual(local.plan, plan);
    assert.equal(local.base.local.length, length, 'restart does not republish or replace admission');
    assert.deepEqual((await catalog(local)).entries, page.entries);
    for (const file of ['identities.json', 'network.json', 'plan.json']) assert.equal(fs.statSync(path.join(directory, file)).mode & 0o777, 0o600);
  } finally { await local.close(); }
});
test('local test catalog rejects unsafe or substituted state without replacing it', async t => {
  const directory = root(t);
  const local = await openLocalCatalog(directory); await local.close();
  const file = path.join(directory, 'identities.json'); const original = fs.readFileSync(file);
  fs.chmodSync(file, 0o644); await assert.rejects(openLocalCatalog(directory));
  assert.ok(fs.readFileSync(file).equals(original), "unsafe state must remain unchanged"); fs.chmodSync(file, 0o600);
  const network = path.join(directory, 'network.json');
  const value = JSON.parse(fs.readFileSync(network)); value.network.subnet_bootstrap = 'f'.repeat(64);
  fs.writeFileSync(network, JSON.stringify(value));
  await assert.rejects(openLocalCatalog(directory));
  assert.deepEqual(JSON.parse(fs.readFileSync(network)), value);
});
test('local test catalog rejects symlink directories and partial identity loss', async t => {
  const directory = root(t); const linked = `${directory}-link`;
  fs.symlinkSync(directory, linked); t.after(() => fs.unlinkSync(linked));
  await assert.rejects(openLocalCatalog(linked));
  const local = await openLocalCatalog(directory); await local.close();
  fs.unlinkSync(path.join(directory, 'identities.json'));
  await assert.rejects(openLocalCatalog(directory));
  assert.equal(fs.existsSync(path.join(directory, 'identities.json')), false);
});
test('a lost original plan is not replaced by another synthetic admission', async t => {
  const directory = root(t);
  const local = await openLocalCatalog(directory); await local.close();
  const plan = path.join(directory, 'plan.json'); fs.unlinkSync(plan);
  await assert.rejects(openLocalCatalog(directory));
  assert.equal(fs.existsSync(plan), false);
});

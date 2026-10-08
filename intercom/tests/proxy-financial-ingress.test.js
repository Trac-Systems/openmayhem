import { PROXY_FINANCIAL_STATE_SERVICE } from '../features/mayhem/proxy-financial-state.js';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import b4a from 'b4a';
import Autobase from 'autobase';
import Corestore from 'corestore';
import Hyperbee from 'hyperbee';
import PeerWallet from 'trac-wallet';
import { Protocol } from 'trac-peer';
import MayhemProtocol, { mayhemFeatureParticipant } from '../contract/protocol.js';
import { CONTRACT_VERSION } from '../contract/contract.js';
import MayhemFeature, { participantFor } from '../features/mayhem/index.js';
import { createProxyCanonicalSnapshot } from '../features/mayhem/proxy-canonical-view.js';
import { createProxyPublicationTransport } from '../features/mayhem/proxy-publication-transport.js';
import { ProxyPublicationController, ProxyPublicationJournal } from '../features/mayhem/proxy-publication-journal.js';
import { proxyPublicationFeatureKey } from '../contract/proxy-publication.js';
import { proxySettlementPolicyDigest, proxySpendTermsDigest, proxyBuyerReceiptSigningBytes, proxyProviderReceiptSigningBytes } from '../contract/proxy-finance.js';
import { proxyOfferCost } from '../contract/proxy-protocol.js';
import { submitMayhemFeature } from '../src/rpc.js';
import { proxyReservationFixture } from './helpers/proxy-finance.js';

import { closure, expiry, nextAttempt } from './helpers/proxy-closure.js';
import { proxyReservationKeys } from '../contract/proxy-reservations.js';

const hex = value => b4a.toString(value, 'hex');
const copy = structuredClone;
const financialResult = response => response.result?.result ?? response.result;
const sign = (wallet, bytes) => hex(wallet.sign(bytes));

async function fixture(t, rail = 'tnk', family = 'llm') {
  const f = await proxyReservationFixture(rail, family);
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-proxy-finance-'));
  let store, base, feature, controller, journal, transport, appends = 0;
  const openBase = async () => {
    store = new Corestore(path.join(directory, 'store'));
    base = new Autobase(store, null, { ackInterval: 0, valueEncoding: 'json',
      open: views => new Hyperbee(views.get('view'), { extension: false, keyEncoding: 'utf-8', valueEncoding: 'json' }),
      apply: async (nodes, view) => {
        const batch = view.batch();
        try {
          for (const node of nodes) {
            if (node.value?.type === 'seed') {
              for (const [key, value] of node.value.entries) {
                if (value === null) await batch.del(key); else await batch.put(key, value);
              }
            } else await f.contract.execute(node.value, batch);
          }
          await batch.flush();
        } finally { await batch.close(); }
      } });
    await base.ready();
    f.peer.base = base;
  };
  await openBase();
  const bootstrap = hex(base.key);
  for (const value of [f.network, f.context, f.config, f.terms]) value.subnet_bootstrap = bootstrap;
  f.peer.config.bootstrap = bootstrap;
  await f.storage.put('proxy/v1/config', f.config);
  // Real current state omits the obsolete `epoch` property.
  await f.storage.put('epoch/apply/state', { updated_epoch: 100, pending_epoch: null });
  await base.append({ type: 'seed', entries: [...f.storage.values.entries()] });
  f.peer.wallet = { publicKey: f.admin.publicKey,
    sign: bytes => sign(f.admin.wallet, b4a.isBuffer(bytes) ? bytes : b4a.from(String(bytes))),
    verify: (signature, bytes, publicKey) => PeerWallet.verify(
      b4a.isBuffer(signature) ? signature : b4a.from(signature, 'hex'),
      b4a.isBuffer(bytes) ? bytes : b4a.from(String(bytes)),
      b4a.isBuffer(publicKey) ? publicKey : b4a.from(publicKey, 'hex')) };
  f.peer.protocol = { instance: { features: {}, featMaxBytes: () => 64000 } };
  f.peer.contract = { instance: f.contract };
  const openController = async () => {
    transport = createProxyPublicationTransport(f.peer, CONTRACT_VERSION);
    journal = await ProxyPublicationJournal.open({ directory: path.join(directory, 'journal'), identity: transport.identity() });
    feature = new MayhemFeature(f.peer, { resultTimeoutMs: 25, resultPollMs: 1,
      withProxyCanonicalSnapshot: createProxyCanonicalSnapshot(f.peer, CONTRACT_VERSION) });
    feature.key = 'mayhem'; f.peer.protocol.instance.features.mayhem = feature;
    controller = new ProxyPublicationController({ journal, ...transport,
      admit: (key, value, forward) => feature._admitProxyPublication(key, value, forward),
      append: async entry => { appends++; return feature._submitFeature(entry.key, entry.envelope, { nonce: entry.nonce }); },
      result: (entry, result) => feature._featureResponse(entry.key, entry.hash, entry.result_key, result) });
    feature.proxyPublicationController = controller;
  };
  await openController();
  const stop = async () => { await controller.close(); await feature.stop(); await base.close(); await store.close(); };
  t.after(async () => { await stop(); fs.rmSync(directory, { recursive: true, force: true }); });
  const submit = async envelope => submitMayhemFeature(f.peer, { key: await proxyPublicationFeatureKey(envelope), value: envelope });
  const read = async key => (await base.view.get(key))?.value ?? null;
  const seed = async entries => { await base.append({ type: 'seed', entries }); await base.update(); };
  const receipt = async ({ seq = 1, final = true, quantity = 4 } = {}) => {
    const usage = Object.fromEntries(f.terms.offer.rates.map(rate => [rate.unit, quantity]));
    const au = proxyOfferCost(f.terms.offer, usage);
    const body = { schema_version: 1, lane: 'proxy', accepted_terms: await proxySpendTermsDigest(f.terms), seq,
      final, outcome: final ? 'complete' : 'running', result_hash: 'a'.repeat(64), observation_hash: 'b'.repeat(64),
      usage, au_owed_cum: au, billing_au_owed_cum: au, at_ms: 2000 + seq };
    return { op: 'proxy_record_usage', provider: f.provider.publicKey, receipt: { body,
      buyer_sig: sign(f.buyer.wallet, proxyBuyerReceiptSigningBytes(body)),
      provider_sig: sign(f.provider.wallet, proxyProviderReceiptSigningBytes(body)) } };
  };
  return { ...f, read, seed, submit, receipt, get base() { return base; }, get feature() { return feature; },
    get controller() { return controller; }, get journal() { return journal; }, get appends() { return appends; },
    reopen: async () => { await stop(); await openBase(); await base.update(); await openController(); } };
}

for (const family of ['llm', 'decisions']) for (const rail of ['fiat', 'tnk', 'tap']) {
  test(`${family}/${rail}: actual canonical ingress reserves and finalizes once across restart`, async t => {
    const f = await fixture(t, rail, family);
    const reservation = f.authorize(f.terms);
    assert.equal(participantFor(reservation), f.provider.publicKey);
    assert.equal(mayhemFeatureParticipant(reservation), f.provider.publicKey);
    const results = await Promise.all(Array.from({ length: 5 }, () => f.submit(reservation)));
    assert.ok(results.every(result => result.ok), JSON.stringify(results));
    assert.equal(f.appends, 1);
    const stateQuery={accepted_terms:await proxySpendTermsDigest(f.terms),request_nonce:'c'.repeat(64),requester:f.provider.publicKey};
    const state=await f.feature._handleService(PROXY_FINANCIAL_STATE_SERVICE,stateQuery,{});
    assert.equal(state.session.settlement_ready,false);
    assert.equal(state.receipt_head,null);
    assert.deepEqual(state.accepted.authorization,reservation.authorization);
    await assert.rejects(f.feature._handleService(PROXY_FINANCIAL_STATE_SERVICE,{...stateQuery,requester:f.admin.publicKey},{}),/not a party/);
    assert.equal(f.appends,1,'financial reads never append');
    assert.equal((await f.read('proxy/v1/reservation-budget')).count, 1);
    assert.equal((await f.read(f.summaryKey)).reserved_au, String(50n + BigInt(f.terms.max_spend_au)));
    const receipt = await f.receipt();
    assert.equal(participantFor(receipt), f.provider.publicKey);
    assert.equal(mayhemFeatureParticipant(receipt), f.provider.publicKey);
    const original = f.controller.append;
    f.controller.append = async entry => { await original(entry); throw new Error('test lost ACK'); };
    assert.equal((await f.submit(receipt)).status, 'pending');
    const length = f.base.local.length;
    await f.reopen();
    assert.equal((await f.submit(receipt)).ok, true);
    assert.equal(f.base.local.length, length);
    assert.equal(f.journal.list().length, 0);
    assert.equal((await f.submit(reservation)).duplicate, true);
    assert.equal((await f.submit(receipt)).duplicate, true);
    const recovered=await f.feature._handleService(PROXY_FINANCIAL_STATE_SERVICE,stateQuery,{});
    assert.deepEqual(recovered.receipt_head.receipt,receipt.receipt);
    assert.equal(recovered.session.settlement_ready,true);
    assert.equal(f.appends, 2);
    assert.equal((await f.read(f.balanceKey)).au, f.balance.au, 'settlement owns actual debit');
    assert.equal((await f.read(f.summaryKey)).reserved_au, String(50n + BigInt(receipt.receipt.body.au_owed_cum)));
    assert.deepEqual(await f.read('payout/epoch/542'), { status: 'prepared', native: true });
  });
}

test('financial invalid/unpaid/misbound/over-budget operations append nothing through every public entry', async t => {
  const f = await fixture(t);
  const valid = f.authorize(f.terms);
  for (const method of ['submit', 'record', 'append', 'rpc', 'relayed']) {
    const forged = copy(valid); forged.authorization.buyer_sig = '0'.repeat(128);
    const key = await proxyPublicationFeatureKey(forged);
    const call = method === 'rpc' ? () => submitMayhemFeature(f.peer, { key, value: forged })
      : method === 'relayed' ? () => f.feature._applyRelayed(key, forged, 'test-transport')
        : () => f.feature[method](key, forged);
    const before = f.base.local.length;
    await assert.rejects(call, /signature/);
    assert.equal(f.base.local.length, before);
  }
  const providerKey = `proxy/v1/provider/${f.provider.publicKey}`;
  const provider = await f.read(providerKey);
  await f.seed([[providerKey, null]]);
  const before = f.base.local.length;
  await assert.rejects(() => f.submit(valid), /admitted/);
  assert.equal(f.base.local.length, before);
  await f.seed([[providerKey, provider], ['proxy/v1/reservation-budget', { epoch: 101, count: 1000 }]]);
  const full = f.base.local.length;
  await assert.rejects(() => f.submit(valid), /quota/);
  assert.equal(f.base.local.length, full);
  assert.equal(f.appends, 0);
});

test('accepted final receipt survives disabled admissions, revocation and exhausted checkpoint budget', async t => {
  const f = await fixture(t);
  const policy = await f.read('proxy/v1/finance-policy');
  await f.seed([['proxy/v1/finance-policy', { ...policy, max_checkpoints_per_reservation: 1 }]]);
  assert.equal((await f.submit(f.authorize(f.terms))).ok, true);
  assert.equal((await f.submit(await f.receipt({ final: false }))).ok, true);
  await f.seed([['proxy/v1/finance-policy', { ...policy, enabled: false }],
    [`proxy/v1/provider-revoked/${f.provider.publicKey}`, { reason_hash: 'c'.repeat(64) }]]);
  const before = f.base.local.length;
  const checkpoint = await f.receipt({ final: false, seq: 2, quantity: 5 });
  await assert.rejects(() => f.submit(checkpoint), /checkpoint publication budget/);
  assert.equal(f.base.local.length, before);
  const final = await f.receipt({ seq: 3, quantity: 5 });
  const forged = { ...final, provider: 'f'.repeat(64) };
  await assert.rejects(() => f.submit(forged), /provider does not match/);
  assert.equal(f.base.local.length, before);
  assert.equal((await f.submit(final)).ok, true);
  assert.equal((await f.read(f.contract.receiptHeadKey(f.terms.billing_id, 1))).checkpoint_count, 1);
});

test('concurrent distinct spends cannot both preflight against the same pending balance/quota', async t => {
  const f = await fixture(t);
  let release;
  const barrier = new Promise(resolve => { release = resolve; });
  let arrived;
  const ready = new Promise(resolve => { arrived = resolve; });
  const original = f.controller.append;
  f.controller.append = async entry => { arrived(); await barrier; return original(entry); };
  const first = f.submit(f.authorize(f.terms));
  await ready;
  const otherTerms = { ...f.terms, billing_id: 'd'.repeat(64), reservation_id: 'e'.repeat(64), session_id: 'f'.repeat(64) };
  try {
    await assert.rejects(() => f.submit(f.authorize(otherTerms)), /accounting dependency/);
    assert.equal(f.appends, 0, 'second request never reaches append');
    assert.equal(f.journal.list().length, 1);
  } finally { release(); }
  assert.equal((await first).ok, true);
  const before = f.base.local.length;
  await assert.rejects(() => f.submit(f.authorize(otherTerms)), /unreserved credit/);
  assert.equal(f.base.local.length, before);
  assert.equal(f.appends, 1);
});

test('proxy financial operations cannot escape through paid/prepared/aliased MSB dispatch', async t => {
  let sent = 0;
  t.mock.method(Protocol.prototype, 'broadcastTransaction', async () => { sent++; });
  const protocol = new MayhemProtocol({}, {}, {});
  for (const [op, alias] of [['proxy_spend_reserve', 'proxySpendReserve'], ['proxy_record_usage', 'proxyRecordUsage']]) {
    for (const command of [{ op }, { type: alias, value: {} }, { type: op, value: {} },
      { op: 'admin_contract_tx', prepared_command: { type: 'mayhem_feature', value: { op } } }]) {
      assert.throws(() => protocol.mapTxCommand(JSON.stringify(command)), /admitted feature/);
      await assert.rejects(protocol.preparePaidTransaction(command), /admitted feature/);
      await assert.rejects(protocol.broadcastTransaction(command), /admitted feature/);
      await assert.rejects(protocol.broadcastPreparedTransaction({ dispatch: command, surrogate: {} }), /admitted feature/);
    }
  }
  assert.equal(sent, 0);
});

test('zero-cost completion uses atomic deletion and remains recoverable after restart', async t => {
  const f = await fixture(t);
  assert.equal((await f.submit(f.authorize(f.terms))).ok, true);
  const receipt = await f.receipt({ quantity: 0 });
  assert.equal((await f.submit(receipt)).ok, true);
  assert.equal(await f.read(f.contract.targetedSpendSessionKey(f.buyer.publicKey, 'tnk', f.terms.reservation_id)), null);
  assert.equal(await f.read(f.contract.targetedSpendSessionIndexKey(f.buyer.publicKey, 'tnk', f.terms.session_id)), null);
  assert.equal(await f.read(f.contract.receiptEpochIndexKey(101)), null);
  assert.equal((await f.read(f.summaryKey)).reserved_au, '50');
  await f.reopen();
  assert.equal((await f.submit(receipt)).duplicate, true);
});

test('participant obtains authenticated financial preflight from the writer without any ledger write', async t => {
  const f = await fixture(t);
  const requester = f.provider.publicKey;
  const peer = { ...f.peer, wallet: { ...f.peer.wallet, publicKey: requester,
    sign: bytes => sign(f.provider.wallet, b4a.isBuffer(bytes) ? bytes : b4a.from(String(bytes))) },
    base: { writable: false, view: f.base.view } };
  const participant = new MayhemFeature(peer, {});
  t.after(() => participant.stop());
  participant.requestService = async (service, request) => {
    const authorization = f.feature._verifyServiceRequest(service, request,
      { admin: f.admin.publicKey, transport: requester });
    assert.ok(authorization);
    return f.feature._handleService(service, authorization.payload, authorization);
  };
  const reservation = f.authorize(f.terms);
  const before = f.base.local.length;
  const preflight = await participant._requestProxyPreflight(await proxyPublicationFeatureKey(reservation), reservation);
  assert.equal(preflight.status, 'admissible');
  assert.equal(preflight.context.epoch, 100);
  assert.equal(f.base.local.length, before);
  assert.equal((await f.submit(reservation)).ok, true);
  const receipt = await f.receipt();
  const receiptPreflight = await participant._requestProxyPreflight(await proxyPublicationFeatureKey(receipt), receipt);
  assert.equal(receiptPreflight.status, 'admissible');
  assert.equal(f.appends, 1);
  assert.equal((await f.submit(receipt)).ok, true);
  const replay = await participant._requestProxyPreflight(await proxyPublicationFeatureKey(receipt), receipt);
  assert.equal(replay.status, 'applied');
  assert.equal(replay.result.au, receipt.receipt.body.au_owed_cum);
  assert.equal(f.appends, 2);
});


for(const family of ['llm','decisions'])for(const rail of ['fiat','tnk','tap']) {
  test(`${family}/${rail}: real closure lost ACK/restart releases once and admits next attempt`,async t=>{
    const f=await fixture(t,rail,family);
    assert.equal((await f.submit(f.authorize(f.terms))).ok,true);
    const e=await closure(f);
    assert.equal(participantFor(e),f.provider.publicKey);
    assert.equal(mayhemFeatureParticipant(e),f.provider.publicKey);
    const append=f.controller.append;
    f.controller.append=async entry=>{await append(entry);throw new Error('injected lost closure ACK');};
    assert.equal((await f.submit(e)).status,'pending');
    assert.equal((await f.read(f.summaryKey)).reserved_au,'50');
    await f.reopen(); const count=f.appends;
    const recovered=await f.submit(e);
    assert.equal(recovered.ok,true); assert.equal(financialResult(recovered).retry_safe,true);
    assert.equal(f.appends,count,'restart recovery must not append closure again');
    Object.assign(f.terms,nextAttempt(f.terms));
    assert.equal((await f.submit(f.authorize(f.terms))).ok,true);
    assert.equal((await f.read(f.ledger.receiptBillingKey(f.terms.billing_id))).latest_attempt,2);
    assert.equal((await f.submit(await f.receipt())).ok,true);
    assert.equal((await f.read(f.ledger.receiptEpochIndexKey(101))).count,1);
    assert.deepEqual(await f.read(f.balanceKey),f.balance);
    assert.deepEqual(await f.read('payout/epoch/542'),{status:'prepared',native:true});
  });
}

test('canonical completed epoch controls expiry, unknown retry is fenced through restart, later proof resolves',async t=>{
  const f=await fixture(t);
  f.settlementPolicy.hold_expiry='release_unfinalized_and_block_retry';
  f.terms.settlement_policy_hash=await proxySettlementPolicyDigest(f.settlementPolicy);
  await f.seed([[proxyReservationKeys.settlementPolicy(f.terms.settlement_policy_hash),{enabled:true,policy:f.settlementPolicy}]]);
  assert.equal((await f.submit(f.authorize(f.terms))).ok,true);
  const e=await expiry(f), done=e.expiry.body.observed_epoch;
  assert.equal(participantFor(e),f.buyer.publicKey);
  assert.equal(mayhemFeatureParticipant(e),f.buyer.publicKey);
  // Pending epoch is NOT proof that the receipt grace is over.
  await f.seed([['epoch/apply/state',{updated_epoch:done-1,pending_epoch:done}]]);
  const before=f.appends;
  await assert.rejects(f.submit(e),/grace/); assert.equal(f.appends,before);
  await f.seed([['epoch/apply/state',{updated_epoch:done,pending_epoch:null}]]);
  assert.equal(financialResult(await f.submit(e)).retry_safe,false);
  assert.equal((await f.read(f.summaryKey)).reserved_au,'50');
  await f.reopen(); assert.equal(financialResult(await f.submit(e)).execution,'unknown');
  const next=nextAttempt(f.terms,{billing_epoch:done+1,acceptance_expires_after_epoch:done+1,reservation_expires_after_epoch:done+20});
  await assert.rejects(f.submit(f.authorize(next)),/recover/);
  assert.equal(financialResult(await f.submit(await closure(f,'cancelled'))).retry_safe,true);
  assert.equal((await f.read(f.summaryKey)).reserved_au,'50');
  assert.equal((await f.submit(f.authorize(next))).ok,true);
});

test('forged closure/expiry and paid aliases fail before canonical append',async t=>{
  const f=await fixture(t);assert.equal((await f.submit(f.authorize(f.terms))).ok,true);
  const e=await closure(f), x=await expiry(f), count=f.appends;
  await assert.rejects(f.submit({...e,closure:{...e.closure,provider_sig:'0'.repeat(128)}}),/signature/);
  await assert.rejects(f.submit(x),/not enabled/);
  const {assertProxyPublicationNotPaid}=await import('../contract/proxy-protocol.js');
  for(const op of [e,x]) {
    for(const value of [op,{type:op.op},{dispatch:op},{prepared_command:{value:op}}]) {
      assert.throws(()=>assertProxyPublicationNotPaid(value),/admitted feature/);
    }
  }
  assert.equal(f.appends,count);
  assert.equal((await f.submit(e)).ok,true);
});

test('repeated local financial queries use fresh signed challenges and see closure instead of cached admission',async t=>{
  const f=await fixture(t);await f.submit(f.authorize(f.terms));
  const requester=f.provider.publicKey;
  const peer={...f.peer,wallet:{...f.peer.wallet,publicKey:requester,
    sign:bytes=>sign(f.provider.wallet,b4a.isBuffer(bytes)?bytes:b4a.from(String(bytes)))},base:{writable:false,view:f.base.view}};
  const participant=new MayhemFeature(peer,{});t.after(()=>participant.stop());
  const seen=[],cache=new Map();
  participant.requestService=async(service,request)=>{
    const authorization=f.feature._verifyServiceRequest(service,request,{admin:f.admin.publicKey,transport:requester});
    assert.ok(authorization);
    const key=JSON.stringify(authorization.payload);seen.push(authorization.payload.request_nonce);
    if(!cache.has(key))cache.set(key,await f.feature._handleService(service,authorization.payload,authorization));
    return structuredClone(cache.get(key));
  };
  const query={accepted_terms:await proxySpendTermsDigest(f.terms),request_nonce:'d'.repeat(64)};
  const first=await participant.proxyFinancialState(query);assert.equal(first.receipt_head,null);
  await f.submit(await f.receipt());
  const second=await participant.proxyFinancialState(query);assert.equal(second.receipt_head.settlement_ready,true);
  assert.equal(first.request_nonce,query.request_nonce);assert.equal(second.request_nonce,query.request_nonce);
  assert.equal(new Set(seen).size,2);assert.ok(seen.every(nonce=>nonce!==query.request_nonce));
  assert.equal(f.appends,2,'queries never append');
});

import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import sodium from 'sodium-native';
import { AdmissionWorker } from '../scripts/proxy-admission-worker.mjs';
import { PURPOSE, EVIDENCE_PAGE_SIZE, EVIDENCE_PROGRESS_DOMAIN, digest, evidenceSeed, evidenceAppend, validateWork } from '../scripts/proxy-admission-wire.mjs';
import { proxyCanonicalSigningBytes, proxyAdmissionSigningBytes } from '../contract/proxy-protocol.js';

// Synthetic existing invoice fixtures only. The deterministic seed is public
// test material and never loaded from an operator wallet or environment.
const fixtures = JSON.parse(fs.readFileSync(new URL('./fixtures/proxy-admission-worker-v1.json', import.meta.url)));
const pk = Buffer.alloc(32), sk = Buffer.alloc(64);
sodium.crypto_sign_seed_keypair(pk, sk, Buffer.alloc(32, 83));
const h = n => n.toString(16).padStart(64, '0'), clone = structuredClone;
const fixedNow = 1700000100000;
function sign(bytes, key = sk) {
  const signature = Buffer.alloc(64); sodium.crypto_sign_detached(signature, bytes, key); return signature.toString('hex');
}
async function commitManifest(work) {
  work.evidence.evidence_commitment = await digest('mayhem/proxy/admission-evidence-pages/v1', {
    invoice_commitment: work.invoice.invoice_commitment, root: work.evidence.root,
    receipt_count: work.evidence.receipt_count, total_amount: work.evidence.total_amount,
  });
  work.permit.evidence_commitment = work.evidence.evidence_commitment;
}
async function fixture(rail = 'tap', count = 37) {
  const source = fixtures.cases.find(c => c.rail === rail), work = clone(source.issue_work);
  const template = source.issue_work.evidence.receipts[0], required = BigInt(work.invoice.amount_base_units);
  const originals = Array.from({ length: count }, (_, i) => {
    const member = clone(template), ref = member.payment_reference, receipt = member.receipt;
    if (rail === 'tap') {
      ref.log_index = i + 1; receipt.log_index = ref.log_index;
      receipt.physical_key = `tap/${ref.chain_id}/${ref.token_contract}/${ref.transaction_hash}/${ref.log_index}`;
    } else if (rail === 'tnk') {
      ref.transaction_hash = h(i + 1); receipt.transaction_hash = ref.transaction_hash;
      receipt.physical_key = `tnk/${ref.network}/${ref.transaction_hash}`;
    } else {
      ref.payment_intent_id = `pi_${i % 2 ? 'A' : 'a'}${String(i).padStart(3, '0')}`;
      receipt.payment_intent_id = ref.payment_intent_id;
      receipt.physical_key = `fiat/${ref.stripe_account}/${ref.livemode ? 'live' : 'test'}/${ref.payment_intent_id}`;
    }
    receipt.amount_base_units = String(required / BigInt(count) + (i === count - 1 ? required % BigInt(count) : 0n));
    member.reference_assigned_at_ms = work.invoice.created_at_ms + 20;
    if (receipt.paid_at_ms !== undefined) receipt.paid_at_ms = work.invoice.created_at_ms + 10;
    return member;
  }).reverse();
  // Model arbitrary verification order, with pages ordered by canonical ASCII
  // bytes instead. Mixed-case Stripe IDs exercise the C-collation requirement.
  const members = [...originals].sort((a, b) => Buffer.compare(Buffer.from(a.receipt.physical_key), Buffer.from(b.receipt.physical_key)));
  let root = await evidenceSeed(work), total = 0n;
  for (let i = 0; i < members.length; i++) { root = await evidenceAppend(work, root, i + 1, members[i]); total += BigInt(members[i].receipt.amount_base_units); }
  work.evidence = { format: 'paged-v1', canonical_epoch: work.permit.valid_from_epoch,
    evidence_commitment: h(0), receipt_count: members.length, total_amount: String(total), root };
  work.evidence_progress = null; await commitManifest(work); await validateWork(work, 'issue');
  return { work, members, originals };
}
function page(f, work = f.work) {
  const from = work.evidence_progress?.checkpoint.count ?? 0;
  return { schema_version: 1, purpose: PURPOSE, phase: 'issue', evidence_commitment: work.evidence.evidence_commitment,
    from_count: from, members: f.members.slice(from, from + EVIDENCE_PAGE_SIZE).map((member, i) => ({ sequence: from + i + 1, member: clone(member) })) };
}
function canonical(work, query) {
  const policy = { ok: true, schema_version: 1, lane: 'proxy', requester: h(1), request_nonce: query.request_nonce,
    context: { ...work.invoice.network, epoch: work.permit.valid_from_epoch },
    proof: { view_key: h(2), tree_hash: h(3), signed_length: 1, fork: 0 }, registry_enabled: true,
    fee_policy_hash: work.invoice.fee_policy_hash, active_issuers: [work.invoice.issuer_pubkey], max_permit_epochs: 20 };
  if (query.recovery) Object.assign(policy, { provider_pubkey: query.provider_pubkey, recovery: query.recovery,
    enrollment: { provider_pubkey: query.provider_pubkey, entitlement_id: null, provider_revoked: false, admission_revoked: false },
    recovery_state: { entitlement_used: null, invoice_used: null, evidence_used: null, admission_revoked: false, generation: null } });
  return policy;
}
function issuer(f, overrides = {}) {
  const state = { reads: [], pages: [], signatures: [], now: fixedNow };
  const options = { phase: 'issue', api: { phase: 'issue', post: async (action, body) => {
    assert.equal(action, 'evidence-page'); assert.equal(body.invoice_id, f.work.invoice_id);
    const result = page(f); state.pages.push(clone(result)); return result;
  } }, network: f.work.invoice.network, feePolicyHash: f.work.invoice.fee_policy_hash,
  issuerPubkey: f.work.invoice.issuer_pubkey, rails: [f.work.invoice.rail], coreOrigin: 'http://127.0.0.1:9', allowLoopbackHttp: true,
  now: () => state.now, signPermit: bytes => { state.signatures.push(Buffer.from(bytes)); return sign(bytes); },
  fetcher: async (url, init) => { assert.equal(url, 'http://127.0.0.1:9/v1/proxy/admission-policy');
    const query = JSON.parse(init.body); state.reads.push(query); return Response.json(canonical(f.work, query)); }, ...overrides };
  return { worker: new AdmissionWorker(options), state };
}
const signal = () => new AbortController().signal;
async function advance(f, w) {
  const result = await w.complete(f.work, signal()); assert.equal(result.action, 'evidence-progress');
  const progress = result.body.progress;
  assert.ok(sodium.crypto_sign_verify_detached(Buffer.from(progress.signature, 'hex'), proxyCanonicalSigningBytes(EVIDENCE_PROGRESS_DOMAIN, progress.checkpoint), pk));
  f.work.evidence_progress = clone(progress); return progress;
}

test('all rails traverse more than 32 out-of-order receipts with real signatures and only final fresh policy reads', async () => {
  for (const rail of ['tap', 'tnk', 'fiat']) {
    const f = await fixture(rail), { worker, state } = issuer(f);
    assert.notDeepEqual(f.originals, f.members);
    const immutable = clone(f.work.evidence);
    while ((f.work.evidence_progress?.checkpoint.count ?? 0) < f.members.length) {
      const progress = await advance(f, worker);
      assert.equal(state.reads.length, 0); assert.ok(state.pages.at(-1).members.length <= 4);
      assert.equal(progress.checkpoint.last_key, f.members[progress.checkpoint.count - 1].receipt.physical_key);
      assert.deepEqual(f.work.evidence, immutable);
    }
    assert.equal(state.pages.length, 10); assert.equal(state.signatures.length, 10);
    const original = await worker.complete(f.work, signal()); assert.equal(original.action, 'permit');
    assert.equal(state.reads.length, 1); assert.equal(state.pages.length, 10);
    assert.ok(sodium.crypto_sign_verify_detached(Buffer.from(original.body.issuer_signature, 'hex'), proxyAdmissionSigningBytes(f.work.permit), pk));
    assert.deepEqual(original.body.permit, f.work.permit);
    assert.deepEqual((await worker.complete(f.work, signal())).body, original.body, 'lost permit ACK preserves the exact body/signature');
    assert.equal(state.reads.length, 2); assert.notEqual(state.reads[0].request_nonce, state.reads[1].request_nonce);
  }
});

test('repeated, duplicate, out-of-order, missing, oversized and foreign pages cannot advance or sign', async () => {
  const original = await fixture('tap', 9); await advance(original, issuer(original).worker);
  const mutations = [
    p => { p.from_count = 0; }, p => { p.evidence_commitment = h(91); },
    p => { p.members.pop(); }, p => { p.members.push(clone(p.members[0])); },
    p => { p.members[0].sequence++; },
    p => { p.members.reverse(); p.members.forEach((v, i) => { v.sequence = 5 + i; }); },
    p => { p.members[1].member = clone(p.members[0].member); },
    p => { p.members[0].member = clone(original.members[3]); },
    p => { p.members[0].member.receipt.to_address = `0x${'f'.repeat(40)}`; },
    p => { p.members[0].member.receipt.amount_base_units = '0'; },
    p => { p.members[0].member.payment_reference.log_index++; },
    p => { p.extra = 'untrusted'; },
  ];
  for (const mutate of mutations) {
    const f = clone(original), bad = page(f); mutate(bad);
    const { worker, state } = issuer(f, { api: { phase: 'issue', post: async () => bad } });
    await assert.rejects(worker.complete(f.work, signal())); assert.equal(state.signatures.length, 0); assert.equal(state.reads.length, 0);
    assert.deepEqual(f.work.evidence_progress, original.work.evidence_progress);
  }
});

test('checkpoint signature, domain, identity, count, cursor and manifest changes cannot skip validation', async () => {
  const original = await fixture(); await advance(original, issuer(original).worker);
  const mutations = [
    w => { w.evidence_progress.signature = '0'.repeat(128); },
    w => { w.evidence_progress.checkpoint.root = h(88); },
    w => { w.evidence_progress.checkpoint.total = '0'; },
    w => { w.evidence_progress.checkpoint.count = w.evidence.receipt_count; },
    w => { w.evidence_progress.checkpoint.count = 0; },
    w => { w.evidence_progress.checkpoint.last_key = 'z'; },
    w => { w.evidence_progress.checkpoint.last_key = 'z'.repeat(901); },
    w => { w.evidence_progress.checkpoint.invoice_id = 'another_invoice'; },
    w => { w.evidence_progress.checkpoint.invoice_commitment = h(89); },
    w => { w.evidence_progress.checkpoint.evidence_commitment = h(90); },
    w => { w.evidence_progress.signature = sign(proxyCanonicalSigningBytes('mayhem/proxy/admission/v1', w.evidence_progress.checkpoint)); },
    async w => { w.evidence.root = h(92); await commitManifest(w); },
    async w => { w.evidence.receipt_count++; await commitManifest(w); },
  ];
  for (const mutate of mutations) {
    const f = clone(original); await mutate(f.work); const { worker, state } = issuer(f);
    await assert.rejects(worker.complete(f.work, signal()));
    assert.equal(state.pages.length, 0); assert.equal(state.signatures.length, 0); assert.equal(state.reads.length, 0);
  }
  const foreignPk = Buffer.alloc(32), foreignSk = Buffer.alloc(64);
  sodium.crypto_sign_seed_keypair(foreignPk, foreignSk, Buffer.alloc(32, 17));
  const f = clone(original); f.work.evidence_progress.signature = sign(proxyCanonicalSigningBytes(EVIDENCE_PROGRESS_DOMAIN, f.work.evidence_progress.checkpoint), foreignSk);
  await assert.rejects(issuer(f).worker.complete(f.work, signal()), /unsigned evidence progress/);
});

test('a self-consistent but false manifest never produces a completed checkpoint or permit', async () => {
  for (const mutate of [w => { w.evidence.root = h(45); }, w => { w.evidence.total_amount = String(BigInt(w.evidence.total_amount) + 1n); }, w => { w.evidence.receipt_count--; }]) {
    const f = await fixture('tap', 9); mutate(f.work); await commitManifest(f.work);
    const { worker, state } = issuer(f); await advance(f, worker);
    if (f.work.evidence.receipt_count === 9) await advance(f, worker);
    const before = state.signatures.length;
    await assert.rejects(worker.complete(f.work, signal()), /paged evidence differs/);
    assert.equal(state.signatures.length, before); assert.equal(state.reads.length, 0);
  }
});

test('page sums, assignment/payment time, lease and abort bounds are checked before signing', async () => {
  const changes = [
    p => { p.members[0].member.receipt.amount_base_units = String((1n << 128n) - 1n); },
    (p, f) => { p.members[0].member.reference_assigned_at_ms = f.work.invoice.created_at_ms - 1; },
    p => { p.members[0].member.reference_assigned_at_ms = fixedNow + 1; },
    (p, f) => { p.members[0].member.receipt.paid_at_ms = f.work.invoice.created_at_ms - 1; },
    (p, f) => { p.members[0].member.receipt.paid_at_ms = f.work.invoice.quote_expires_at_ms + 1; },
    p => { p.members[0].member.receipt.paid_at_ms = fixedNow + 1; },
  ];
  for (const mutate of changes) {
    const f = await fixture('tap', 9), bad = page(f); mutate(bad, f);
    const { worker, state } = issuer(f, { api: { phase: 'issue', post: async () => bad } });
    await assert.rejects(worker.complete(f.work, signal())); assert.equal(state.signatures.length, 0); assert.equal(state.reads.length, 0);
  }
  for (const mode of ['expired', 'aborted']) {
    const f = await fixture(), abort = new AbortController();
    let state;
    const w = issuer(f, { api: { phase: 'issue', post: async () => {
      if (mode === 'expired') state.now = f.work.lease_expires_at_ms; else abort.abort(new Error('test abort'));
      return page(f);
    } } }); state = w.state;
    await assert.rejects(w.worker.complete(f.work, abort.signal)); assert.equal(state.signatures.length, 0); assert.equal(state.reads.length, 0);
  }
});

test('partial signed evidence survives renewal and restart; final signing rechecks current canonical authority', async () => {
  const f = await fixture('tap', 9); const initial = issuer(f); await advance(f, initial.worker);
  const retained = clone(f.work.evidence_progress), previous = clone(f.work.permit);
  f.work.previous_permit = previous;
  const epoch = previous.expires_after_epoch + 1;
  f.work.evidence.canonical_epoch = epoch;
  Object.assign(f.work.permit, { issuance_revision: 2, nonce: h(100), valid_from_epoch: epoch, expires_after_epoch: epoch + 9 });
  const restarted = issuer(f);
  assert.deepEqual(await restarted.worker.checkedEvidenceProgress(f.work), retained.checkpoint);
  await advance(f, restarted.worker); assert.equal(f.work.evidence_progress.checkpoint.count, 8);
  await advance(f, restarted.worker); assert.equal(restarted.state.reads.length, 0);
  const result = await restarted.worker.complete(f.work, signal()); assert.equal(result.action, 'permit');
  assert.equal(result.body.permit.issuance_revision, 2); assert.equal(restarted.state.reads.length, 1);
  assert.deepEqual(restarted.state.reads[0].recovery, { entitlement_id: previous.entitlement_id,
    invoice_commitment: previous.invoice_commitment, evidence_commitment: previous.evidence_commitment });
  for (const mutate of [p => { p.registry_enabled = false; }, p => { p.active_issuers = [h(77)]; },
    p => { p.recovery_state.evidence_used = { provider_pubkey: h(9), entitlement_id: h(10) }; },
    p => { p.recovery_state.admission_revoked = true; }, p => { p.context.epoch = epoch + 10; }, p => { p.request_nonce = h(80); }]) {
    const w = issuer(f, { fetcher: async (_url, init) => { const p = canonical(f.work, JSON.parse(init.body)); mutate(p); return Response.json(p); } });
    await assert.rejects(w.worker.complete(f.work, signal())); assert.equal(w.state.signatures.length, 0); assert.equal(w.state.pages.length, 0);
  }
});

test('lost progress ACK resumes the persisted checkpoint on the next lease instead of restarting or issuing early', async () => {
  const f = await fixture('tap', 9); let lose = true, completions = 0, persisted = null; const calls = [];
  const api = { phase: 'issue', post: async (action, body) => {
    calls.push(action);
    const ack = { schema_version: 1, purpose: PURPOSE, phase: 'issue', accepted: true };
    if (action === 'pull') return { schema_version: 1, purpose: PURPOSE, phase: 'issue', work: clone(f.work) };
    if (action === 'evidence-page') return page(f);
    if (action === 'evidence-progress') {
      persisted = clone(body.progress); f.work.evidence_progress = persisted; completions++;
      if (lose) { lose = false; throw new Error('lost progress ACK'); } return ack;
    }
    assert.equal(action, 'retry'); return ack;
  } };
  const first = issuer(f, { api }); assert.equal((await first.worker.runOnce()).status, 'retry');
  assert.equal(persisted.checkpoint.count, 4); assert.equal(first.state.reads.length, 0);
  f.work.lease_token = h(110);
  const restarted = issuer(f, { api }); assert.equal((await restarted.worker.runOnce()).status, 'accepted');
  assert.equal(persisted.checkpoint.count, 8); assert.equal(completions, 2); assert.equal(restarted.state.reads.length, 0);
  assert.deepEqual(calls, ['pull', 'evidence-page', 'evidence-progress', 'retry', 'pull', 'evidence-page', 'evidence-progress']);
});


test('paged issuance accepts timely TAP/FIAT found late but never invents a TNK payment timestamp',async()=>{
 for(const rail of ['tap','fiat','tnk']) {
  const f=await fixture(rail,9),time=f.work.invoice.quote_expires_at_ms+1000;
  f.work.lease_expires_at_ms=time+60000;
  for(const m of f.members)m.reference_assigned_at_ms=time-100;
  let root=await evidenceSeed(f.work);
  for(let n=0;n<f.members.length;n++)root=await evidenceAppend(f.work,root,n+1,f.members[n]);
  f.work.evidence.root=root;await commitManifest(f.work);
  const w=issuer(f,{now:()=>time});
  if(rail==='tnk') { await assert.rejects(w.worker.complete(f.work,signal()),/payment_time_unproven/);assert.equal(w.state.signatures.length,0); }
  else {
   while((f.work.evidence_progress?.checkpoint.count??0)<f.members.length)await advance(f,w.worker);
   assert.equal((await w.worker.complete(f.work,signal())).action,'permit');
  }
 }
});

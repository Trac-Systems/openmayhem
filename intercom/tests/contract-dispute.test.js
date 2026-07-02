import assert from 'node:assert/strict';
import test from 'node:test';
import MayhemContract from '../contract/contract.js';
import {
  MemoryStorage,
  execute,
  makeIdentity,
  makeTxKey,
  makeVerifier,
  signConsent,
} from './helpers/contract.js';

const rulesHash = '7'.repeat(64);
const enclaveId = '6'.repeat(64);

async function setupDisputeContract() {
  const admin = await makeIdentity();
  const user = await makeIdentity();
  const provider = await makeIdentity();
  const outsider = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const contract = new MayhemContract({ peer: { wallet: makeVerifier(user.wallet) } }, {});

  for (const op of [
    {
      type: 'setRules',
      value: { op: 'set_rules', ver: 1, hash: rulesHash },
      sender: admin.publicKey,
      txNo: 1,
    },
    {
      type: 'consent',
      value: {
        op: 'consent',
        ver: 1,
        hash: rulesHash,
        sig: signConsent(user.wallet, 1, rulesHash),
      },
      sender: user.publicKey,
      txNo: 2,
    },
    {
      type: 'consent',
      value: {
        op: 'consent',
        ver: 1,
        hash: rulesHash,
        sig: signConsent(provider.wallet, 1, rulesHash),
      },
      sender: provider.publicKey,
      txNo: 3,
    },
    {
      type: 'registerProvider',
      value: { op: 'register_provider' },
      sender: provider.publicKey,
      txNo: 4,
    },
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }

  await storage.put(`bal/${user.publicKey}`, {
    user: user.publicKey,
    denom: 'mu_usd',
    mu: 10_000,
    updated_epoch: 0,
    updated_at: null,
  });
  return { admin, user, provider, outsider, storage, contract };
}

function openDispute(user, provider, overrides = {}) {
  return {
    op: 'dispute',
    session_id: 'session-dispute-1',
    reason: 'service_failure',
    provider: provider.publicKey,
    counterparty: provider.publicKey,
    enclave_id: enclaveId,
    epoch: 2,
    at: 7_200,
    evidence_hash: 'a'.repeat(64),
    evidence: {
      receipts: ['receipt-root'],
      attestation: 'att-head',
      probe: 'probe-head',
    },
    ...overrides,
  };
}

function seedProviderHoldback(storage, provider, overrides = {}) {
  return storage.put(`earn/${provider.publicKey}`, {
    provider: provider.publicKey,
    denom: 'mu_usd',
    total_mu: 20_000,
    held_mu: 10_000,
    paid_cum_mu: 0,
    holdbacks: [
      { epoch: 1, mu: 4_000 },
      { epoch: 2, mu: 6_000 },
    ],
    updated_epoch: 2,
    updated_at: null,
    ...overrides,
  });
}

test('MayhemContract dispute lifecycle refunds deposit and applies provider_fault slash', async () => {
  const { admin, user, provider, outsider, storage, contract } = await setupDisputeContract();
  await seedProviderHoldback(storage, provider);

  const opened = await execute(
    contract,
    storage,
    'dispute',
    openDispute(user, provider),
    user.publicKey,
    5
  );
  assert.equal(opened.ok, true, opened.message);
  assert.equal(opened.dispute_id, 1);
  assert.equal(opened.deposit_mu, 5_000);
  assert.equal((await storage.get(`bal/${user.publicKey}`)).value.mu, 5_000);

  const nonAdmin = await execute(
    contract,
    storage,
    'disputeResolve',
    {
      op: 'dispute_resolve',
      dispute_id: 1,
      outcome: 'provider_fault',
      deposit_action: 'refund',
      rationale_hash: 'b'.repeat(64),
      slash: true,
      at: 10_800,
    },
    outsider.publicKey,
    6
  );
  assert.match(nonAdmin.message, /admin required/i);

  const resolved = await execute(
    contract,
    storage,
    'disputeResolve',
    {
      op: 'dispute_resolve',
      dispute_id: 1,
      outcome: 'provider_fault',
      deposit_action: 'refund',
      rationale_hash: 'b'.repeat(64),
      slash: true,
      at: 10_800,
    },
    admin.publicKey,
    7
  );
  assert.equal(resolved.ok, true, resolved.message);
  assert.equal(resolved.deposit_refunded_mu, 5_000);
  assert.equal(resolved.deposit_forfeited_mu, 0);
  assert.equal(resolved.slash.forfeited_mu, 2_000);
  assert.equal(resolved.slash.beneficiary_mu, 1_000);
  assert.equal(resolved.slash.treasury_mu, 1_000);

  assert.equal((await storage.get(`bal/${user.publicKey}`)).value.mu, 11_000);
  assert.deepEqual((await storage.get(`earn/${provider.publicKey}`)).value, {
    provider: provider.publicKey,
    denom: 'mu_usd',
    total_mu: 18_000,
    held_mu: 8_000,
    paid_cum_mu: 0,
    holdbacks: [
      { epoch: 1, mu: 4_000 },
      { epoch: 2, mu: 4_000 },
    ],
    updated_epoch: 2,
    updated_at: makeTxKey(7),
    slashed_cum_mu: 2_000,
    last_slash_at: makeTxKey(7),
  });
  assert.equal((await storage.get('fee/cum')).value.cum_mu, 1_000);

  const dispute = (await storage.get('disp/1')).value;
  assert.equal(dispute.status, 'resolved');
  assert.equal(dispute.deposit_action, 'refund');
  assert.equal(dispute.deposit_refunded_mu, 5_000);
  assert.equal(dispute.reputation_event.kind, 'dispute_lost');
  assert.equal(dispute.slash.reason, 'dispute_lost');
  assert.equal(dispute.resolution_hash.length, 64);

  const repEvent = await storage.get(`ev/rep/${provider.publicKey}/dispute-1-lost`);
  assert.equal(repEvent.value.kind, 'dispute_lost');
  const slash = await storage.get(`ev/slash/${provider.publicKey}/${makeTxKey(7)}`);
  assert.equal(slash.value.source, 'dispute');

  const duplicate = await execute(
    contract,
    storage,
    'disputeResolve',
    {
      op: 'dispute_resolve',
      dispute_id: 1,
      outcome: 'provider_fault',
      deposit_action: 'refund',
      rationale_hash: 'b'.repeat(64),
      slash: true,
      at: 10_900,
    },
    admin.publicKey,
    8
  );
  assert.match(duplicate.message, /not open/i);
});

test('MayhemContract dispute resolution validates slash target before mutating funds', async () => {
  const { admin, user, provider, storage, contract } = await setupDisputeContract();
  await seedProviderHoldback(storage, provider);

  const opened = await execute(
    contract,
    storage,
    'dispute',
    openDispute(user, provider, {
      session_id: 'session-invalid-beneficiary',
      evidence_hash: 'e'.repeat(64),
    }),
    user.publicKey,
    5
  );
  assert.equal(opened.ok, true, opened.message);
  assert.equal((await storage.get(`bal/${user.publicKey}`)).value.mu, 5_000);

  const invalid = await execute(
    contract,
    storage,
    'disputeResolve',
    {
      op: 'dispute_resolve',
      dispute_id: 1,
      outcome: 'provider_fault',
      deposit_action: 'refund',
      rationale_hash: 'f'.repeat(64),
      slash: true,
      beneficiary: 'bad/beneficiary',
      at: 10_800,
    },
    admin.publicKey,
    6
  );
  assert.match(invalid.message, /invalid slash beneficiary/i);

  assert.equal((await storage.get(`bal/${user.publicKey}`)).value.mu, 5_000);
  assert.equal((await storage.get('disp/1')).value.status, 'open');
  assert.equal(await storage.get(`ev/rep/${provider.publicKey}/dispute-1-lost`), null);
  assert.equal(await storage.get(`ev/slash/${provider.publicKey}/${makeTxKey(6)}`), null);
  assert.equal(await storage.get('fee/cum'), null);
});

test('MayhemContract dispute lifecycle forfeits opener deposit to treasury', async () => {
  const { admin, user, provider, storage, contract } = await setupDisputeContract();

  const opened = await execute(
    contract,
    storage,
    'dispute',
    openDispute(user, provider, {
      session_id: 'session-bad-opener',
      reason: 'invalid_claim',
      evidence_hash: 'c'.repeat(64),
      evidence: { receipts: [] },
    }),
    user.publicKey,
    5
  );
  assert.equal(opened.ok, true, opened.message);

  const resolved = await execute(
    contract,
    storage,
    'disputeResolve',
    {
      op: 'dispute_resolve',
      dispute_id: 1,
      outcome: 'opener_fault',
      deposit_action: 'forfeit',
      rationale_hash: 'd'.repeat(64),
      at: 10_800,
    },
    admin.publicKey,
    6
  );
  assert.equal(resolved.ok, true, resolved.message);
  assert.equal(resolved.deposit_refunded_mu, 0);
  assert.equal(resolved.deposit_forfeited_mu, 5_000);
  assert.equal(resolved.slash, null);

  assert.equal((await storage.get(`bal/${user.publicKey}`)).value.mu, 5_000);
  assert.equal((await storage.get('fee/cum')).value.cum_mu, 5_000);
  const dispute = (await storage.get('disp/1')).value;
  assert.equal(dispute.status, 'resolved');
  assert.equal(dispute.outcome, 'opener_fault');
  assert.equal(dispute.deposit_action, 'forfeit');
  assert.equal(dispute.slash, null);
});

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
const DAY_SECONDS = 24 * 60 * 60;
const DISPUTE_DEPOSIT_AU = '1000000000000000000';
const DOUBLE_DISPUTE_DEPOSIT_AU = '2000000000000000000';

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

  await storage.put(`bal/${user.publicKey}/fiat`, {
    user: user.publicKey,
    rail: 'fiat',
    denom: 'au_usd',
    au: DOUBLE_DISPUTE_DEPOSIT_AU,
    updated_epoch: 0,
    updated_at: null,
  });
  return { admin, user, provider, outsider, storage, contract };
}

function openDispute(user, provider, overrides = {}) {
  return {
    op: 'dispute',
    session_id: 'session-dispute-1',
    rail: 'fiat',
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
  return storage.put(`earn/fiat/${provider.publicKey}`, {
    provider: provider.publicKey,
    rail: 'fiat',
    denom: 'au_usd',
    total_au: '20000',
    held_au: '10000',
    paid_cum_au: '0',
    holdbacks: [
      { epoch: 1, au: '4000' },
      { epoch: 2, au: '6000' },
    ],
    updated_epoch: 2,
    updated_at: null,
    ...overrides,
  });
}

test('MayhemContract dispute default bond blocks half-cent spam-scale opens', async () => {
  const { user, provider, storage, contract } = await setupDisputeContract();
  await storage.put(`bal/${user.publicKey}/fiat`, {
    user: user.publicKey,
    rail: 'fiat',
    denom: 'au_usd',
    au: '10000',
    updated_epoch: 0,
    updated_at: null,
  });
  const before = storage.snapshotBytes();
  const opened = await execute(
    contract,
    storage,
    'dispute',
    openDispute(user, provider),
    user.publicKey,
    5
  );
  assert.match(opened.message, /insufficient balance for dispute deposit/i);
  assert.equal(storage.snapshotBytes(), before);
});

test('MayhemContract dispute timeout permissionlessly refunds unresolved opener bond', async () => {
  const { admin, user, provider, outsider, storage, contract } = await setupDisputeContract();

  const opened = await execute(
    contract,
    storage,
    'dispute',
    openDispute(user, provider),
    user.publicKey,
    5
  );
  assert.equal(opened.ok, true, opened.message);
  assert.equal(opened.expires_after_epoch, 170);
  assert.equal((await storage.get(`bal/${user.publicKey}/fiat`)).value.au, DISPUTE_DEPOSIT_AU);

  await storage.put('epoch/apply/state', {
    updated_epoch: 169,
    updated_at: makeTxKey(6),
    last_apply_hash: 'e'.repeat(64),
  });
  const early = await execute(
    contract,
    storage,
    'disputeExpire',
    {
      op: 'dispute_expire',
      dispute_id: 1,
      at: 604_800,
    },
    outsider.publicKey,
    6
  );
  assert.match(early.message, /timeout epoch not reached/i);
  assert.equal((await storage.get(`bal/${user.publicKey}/fiat`)).value.au, DISPUTE_DEPOSIT_AU);
  assert.equal((await storage.get('disp/1')).value.status, 'open');

  await storage.put('epoch/apply/state', {
    updated_epoch: 170,
    updated_at: makeTxKey(7),
    last_apply_hash: 'f'.repeat(64),
  });
  const expired = await execute(
    contract,
    storage,
    'disputeExpire',
    {
      op: 'dispute_expire',
      dispute_id: 1,
      at: 608_400,
    },
    outsider.publicKey,
    7
  );
  assert.equal(expired.ok, true, expired.message);
  assert.equal(expired.deposit_refunded_au, DISPUTE_DEPOSIT_AU);
  assert.equal(expired.expired_at_epoch, 170);
  assert.equal(expired.expiry_hash.length, 64);
  assert.deepEqual((await storage.get(`bal/${user.publicKey}/fiat`)).value, {
    user: user.publicKey,
    rail: 'fiat',
    denom: 'au_usd',
    au: DOUBLE_DISPUTE_DEPOSIT_AU,
    updated_epoch: 170,
    updated_at: makeTxKey(7),
  });

  const dispute = (await storage.get('disp/1')).value;
  assert.equal(dispute.status, 'expired');
  assert.equal(dispute.outcome, 'timeout_refund');
  assert.equal(dispute.deposit_action, 'refund');
  assert.equal(dispute.deposit_holder, null);
  assert.equal(dispute.expired_by, outsider.publicKey);
  assert.equal(dispute.deposit_refunded_au, DISPUTE_DEPOSIT_AU);
  assert.equal(dispute.deposit_forfeited_au, '0');
  assert.equal(dispute.slash, null);

  const lateAdmin = await execute(
    contract,
    storage,
    'disputeResolve',
    {
      op: 'dispute_resolve',
      dispute_id: 1,
      outcome: 'opener_fault',
      deposit_action: 'forfeit',
      rationale_hash: 'b'.repeat(64),
      at: 612_000,
    },
    admin.publicKey,
    8
  );
  assert.match(lateAdmin.message, /not open/i);
});

test('MayhemContract dispute timeout epochs are admin-governed', async () => {
  const { admin, user, provider, storage, contract } = await setupDisputeContract();

  const scheduled = await execute(
    contract,
    storage,
    'setParams',
    {
      op: 'set_params',
      submitted_at: 0,
      effective_at: DAY_SECONDS,
      values: { dispute_timeout_epochs: 3 },
    },
    admin.publicKey,
    5
  );
  assert.equal(scheduled.ok, true, scheduled.message);

  const opened = await execute(
    contract,
    storage,
    'dispute',
    openDispute(user, provider, { at: DAY_SECONDS }),
    user.publicKey,
    6
  );
  assert.equal(opened.ok, true, opened.message);
  assert.equal(opened.expires_after_epoch, 5);
  const dispute = (await storage.get('disp/1')).value;
  assert.equal(dispute.timeout_epochs, 3);
  assert.equal(dispute.expires_after_epoch, 5);
});

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
  assert.equal(opened.deposit_au, DISPUTE_DEPOSIT_AU);
  assert.equal((await storage.get(`bal/${user.publicKey}/fiat`)).value.au, DISPUTE_DEPOSIT_AU);

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
  assert.equal(resolved.deposit_refunded_au, DISPUTE_DEPOSIT_AU);
  assert.equal(resolved.deposit_forfeited_au, '0');
  assert.equal(resolved.slash.forfeited_au, '2000');
  assert.equal(resolved.slash.beneficiary_au, '1000');
  assert.equal(resolved.slash.treasury_au, '1000');

  assert.equal((await storage.get(`bal/${user.publicKey}/fiat`)).value.au, '2000000000000001000');
  assert.deepEqual((await storage.get(`earn/fiat/${provider.publicKey}`)).value, {
    provider: provider.publicKey,
    rail: 'fiat',
    denom: 'au_usd',
    total_au: '18000',
    held_au: '8000',
    paid_cum_au: '0',
    holdbacks: [
      { epoch: 1, au: '4000' },
      { epoch: 2, au: '4000' },
    ],
    updated_epoch: 2,
    updated_at: makeTxKey(7),
    slashed_cum_au: '2000',
    last_slash_at: makeTxKey(7),
  });
  assert.equal((await storage.get('fee/fiat/cum')).value.cum_au, '1000');

  const dispute = (await storage.get('disp/1')).value;
  assert.equal(dispute.status, 'resolved');
  assert.equal(dispute.deposit_action, 'refund');
  assert.equal(dispute.deposit_refunded_au, DISPUTE_DEPOSIT_AU);
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

test('MayhemContract dispute resolution uses admin-governed dispute_lost_slash_bps', async () => {
  const { admin, user, provider, storage, contract } = await setupDisputeContract();
  await seedProviderHoldback(storage, provider);

  const scheduled = await execute(
    contract,
    storage,
    'setParams',
    {
      op: 'set_params',
      submitted_at: 0,
      effective_at: DAY_SECONDS,
      values: { dispute_lost_slash_bps: 1_000 },
    },
    admin.publicKey,
    5
  );
  assert.equal(scheduled.ok, true, scheduled.message);

  const opened = await execute(
    contract,
    storage,
    'dispute',
    openDispute(user, provider),
    user.publicKey,
    6
  );
  assert.equal(opened.ok, true, opened.message);

  const resolved = await execute(
    contract,
    storage,
    'disputeResolve',
    {
      op: 'dispute_resolve',
      dispute_id: 1,
      outcome: 'provider_fault',
      deposit_action: 'refund',
      rationale_hash: 'c'.repeat(64),
      slash: true,
      at: DAY_SECONDS,
    },
    admin.publicKey,
    7
  );
  assert.equal(resolved.ok, true, resolved.message);
  assert.equal(resolved.slash.slash_bps, 1_000);
  assert.equal(resolved.slash.forfeited_au, '1000');
  assert.equal(resolved.slash.beneficiary_au, '500');
  assert.equal(resolved.slash.treasury_au, '500');
  assert.equal((await storage.get(`bal/${user.publicKey}/fiat`)).value.au, '2000000000000000500');
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
  assert.equal((await storage.get(`bal/${user.publicKey}/fiat`)).value.au, DISPUTE_DEPOSIT_AU);

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

  assert.equal((await storage.get(`bal/${user.publicKey}/fiat`)).value.au, DISPUTE_DEPOSIT_AU);
  assert.equal((await storage.get('disp/1')).value.status, 'open');
  assert.equal(await storage.get(`ev/rep/${provider.publicKey}/dispute-1-lost`), null);
  assert.equal(await storage.get(`ev/slash/${provider.publicKey}/${makeTxKey(6)}`), null);
  assert.equal(await storage.get('fee/fiat/cum'), null);
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
  assert.equal(resolved.deposit_refunded_au, '0');
  assert.equal(resolved.deposit_forfeited_au, DISPUTE_DEPOSIT_AU);
  assert.equal(resolved.slash, null);

  assert.equal((await storage.get(`bal/${user.publicKey}/fiat`)).value.au, DISPUTE_DEPOSIT_AU);
  assert.equal((await storage.get('fee/fiat/cum')).value.cum_au, DISPUTE_DEPOSIT_AU);
  const dispute = (await storage.get('disp/1')).value;
  assert.equal(dispute.status, 'resolved');
  assert.equal(dispute.outcome, 'opener_fault');
  assert.equal(dispute.deposit_action, 'forfeit');
  assert.equal(dispute.slash, null);
});

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

const rulesHash = 'a9'.repeat(32);
const enclaveId = 'b9'.repeat(32);

const providerRegistration = {
  op: 'register_provider',
  payout_addr: 'trac1providerpayouttarget',
  payout_method: 'tnk',
};

async function setupAuditorContract() {
  const admin = await makeIdentity();
  const provider = await makeIdentity();
  const auditor = await makeIdentity();
  const storage = new MemoryStorage({ admin: admin.publicKey });
  const protocol = { peer: { wallet: makeVerifier(provider.wallet) } };
  const contract = new MayhemContract(protocol, {});

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
        sig: signConsent(provider.wallet, 1, rulesHash),
      },
      sender: provider.publicKey,
      txNo: 2,
    },
    {
      type: 'consent',
      value: {
        op: 'consent',
        ver: 1,
        hash: rulesHash,
        sig: signConsent(auditor.wallet, 1, rulesHash),
      },
      sender: auditor.publicKey,
      txNo: 3,
    },
    {
      type: 'registerProvider',
      value: providerRegistration,
      sender: provider.publicKey,
      txNo: 4,
    },
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }

  return { admin, provider, auditor, storage, contract };
}

const canaryProbe = (provider, overrides = {}) => ({
  op: 'probe_result',
  probe_id: 'canary-good',
  probe_kind: 'canary',
  provider: provider.publicKey,
  enclave_id: enclaveId,
  epoch: 1,
  at: 10_000,
  canary_set: 'canary-dev-v1',
  match_bps: 9_700,
  pass: true,
  session_receipt_hash: 'c'.repeat(64),
  evidence_hash: 'd'.repeat(64),
  ...overrides,
});

test('MayhemContract auditor probes write evidence, uptime ticks, and canary violation bans', async () => {
  const { admin, provider, auditor, storage, contract } = await setupAuditorContract();

  const beforeRegister = await execute(
    contract,
    storage,
    'probeResult',
    canaryProbe(provider),
    auditor.publicKey,
    5
  );
  assert.match(beforeRegister.message, /auditor registration required/i);

  const registered = await execute(
    contract,
    storage,
    'auditorRegister',
    {
      op: 'auditor_register',
      auditor: auditor.publicKey,
      registered_at_seconds: 0,
    },
    admin.publicKey,
    6
  );
  assert.deepEqual(registered, {
    ok: true,
    op: 'auditorRegister',
    auditor: auditor.publicKey,
  });

  const auditorEntry = await storage.get(`auditor/${auditor.publicKey}`);
  assert.deepEqual(auditorEntry.value, {
    auditor: auditor.publicKey,
    status: 'active',
    registered_at: makeTxKey(6),
    registered_at_seconds: 0,
    accredited_by: admin.publicKey,
    successful_probes: 0,
    submitted_probes: 0,
    false_reports: 0,
    updated_at: makeTxKey(6),
  });

  const canaryOk = await execute(
    contract,
    storage,
    'probeResult',
    canaryProbe(provider),
    auditor.publicKey,
    7
  );
  assert.deepEqual(canaryOk, {
    ok: true,
    op: 'probeResult',
    probe_id: 'canary-good',
    provider: provider.publicKey,
    pass: true,
    provenance_violation: false,
  });

  const uptime = await execute(
    contract,
    storage,
    'probeResult',
    {
      op: 'probe_result',
      probe_id: 'uptime-1',
      probe_kind: 'uptime_tick',
      provider: provider.publicKey,
      epoch: 1,
      at: 21_600,
      evidence_hash: 'e'.repeat(64),
    },
    auditor.publicKey,
    8
  );
  assert.equal(uptime.ok, true, uptime.message);
  assert.equal(uptime.pass, true);

  const mismatch = await execute(
    contract,
    storage,
    'probeResult',
    canaryProbe(provider, {
      probe_id: 'canary-bad',
      match_bps: 1_000,
      pass: false,
      evidence_hash: 'f'.repeat(64),
    }),
    auditor.publicKey,
    9
  );
  assert.deepEqual(mismatch, {
    ok: true,
    op: 'probeResult',
    probe_id: 'canary-bad',
    provider: provider.publicKey,
    pass: false,
    provenance_violation: true,
  });

  const head = await storage.get(`ev/rep/head/${provider.publicKey}`);
  assert.equal(head.value.count, 4);

  const goodProbe = await storage.get('ev/probe/canary-good');
  assert.equal(goodProbe.value.pass, true);
  assert.equal(goodProbe.value.probe_reward_mu, 5_000);

  const badProbe = await storage.get('ev/probe/canary-bad');
  assert.equal(badProbe.value.pass, false);
  assert.equal(badProbe.value.provenance_violation, true);
  assert.equal(badProbe.value.slash.reason, 'canary_mismatch');

  const slash = await storage.get(`ev/slash/${provider.publicKey}/${makeTxKey(9)}`);
  assert.equal(slash.value.reason, 'canary_mismatch');
  assert.equal(slash.value.beneficiary, auditor.publicKey);
  assert.equal(slash.value.slashed_by, auditor.publicKey);

  const providerEntry = await storage.get(`prov/${provider.publicKey}`);
  assert.equal(providerEntry.value.status, 'banned');
  assert.equal(providerEntry.value.banned_by, auditor.publicKey);
  assert.equal(providerEntry.value.ban_reason_hash, 'f'.repeat(64));

  const updatedAuditor = await storage.get(`auditor/${auditor.publicKey}`);
  assert.equal(updatedAuditor.value.submitted_probes, 3);
  assert.equal(updatedAuditor.value.successful_probes, 2);
});

test('MayhemContract auditor probe log replays deterministically', async () => {
  const first = await setupAuditorContract();
  const second = {
    admin: first.admin,
    provider: first.provider,
    auditor: first.auditor,
    storage: new MemoryStorage({ admin: first.admin.publicKey }),
    contract: new MayhemContract({ peer: { wallet: makeVerifier(first.provider.wallet) } }, {}),
  };

  async function applyScenario(ctx) {
    for (const op of [
      {
        type: 'setRules',
        value: { op: 'set_rules', ver: 1, hash: rulesHash },
        sender: ctx.admin.publicKey,
        txNo: 1,
      },
      {
        type: 'consent',
        value: {
          op: 'consent',
          ver: 1,
          hash: rulesHash,
          sig: signConsent(ctx.provider.wallet, 1, rulesHash),
        },
        sender: ctx.provider.publicKey,
        txNo: 2,
      },
      {
        type: 'consent',
        value: {
          op: 'consent',
          ver: 1,
          hash: rulesHash,
          sig: signConsent(ctx.auditor.wallet, 1, rulesHash),
        },
        sender: ctx.auditor.publicKey,
        txNo: 3,
      },
      {
        type: 'registerProvider',
        value: providerRegistration,
        sender: ctx.provider.publicKey,
        txNo: 4,
      },
      {
        type: 'auditorRegister',
        value: {
          op: 'auditor_register',
          auditor: ctx.auditor.publicKey,
          registered_at_seconds: 0,
        },
        sender: ctx.admin.publicKey,
        txNo: 5,
      },
      {
        type: 'probeResult',
        value: canaryProbe(ctx.provider, { probe_id: 'canary-bad', match_bps: 1_000, pass: false }),
        sender: ctx.auditor.publicKey,
        txNo: 6,
      },
    ]) {
      const result = await execute(ctx.contract, ctx.storage, op.type, op.value, op.sender, op.txNo);
      assert.equal(result.ok, true, result.message);
    }
  }

  first.storage = new MemoryStorage({ admin: first.admin.publicKey });
  first.contract = new MayhemContract({ peer: { wallet: makeVerifier(first.provider.wallet) } }, {});
  await applyScenario(first);
  await applyScenario(second);
  assert.equal(first.storage.snapshotBytes(), second.storage.snapshotBytes());
});

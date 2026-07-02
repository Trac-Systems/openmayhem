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

const rulesHash = '9'.repeat(64);
const DAY_SECONDS = 24 * 60 * 60;
const PROBATION_SECONDS = 7 * DAY_SECONDS;

const providerRegistration = {
  op: 'register_provider',
  payout_addr: 'trac1providerpayouttarget',
  payout_method: 'tnk',
  registered_at_seconds: 0,
};

async function setupReputationContract() {
  const admin = await makeIdentity();
  const provider = await makeIdentity();
  const outsider = await makeIdentity();
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
      type: 'registerProvider',
      value: providerRegistration,
      sender: provider.publicKey,
      txNo: 3,
    },
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }

  return { admin, provider, outsider, storage, contract };
}

async function recordEvent(contract, storage, admin, provider, value, txNo) {
  return await execute(
    contract,
    storage,
    'recordReputationEvent',
    {
      provider: provider.publicKey,
      epoch: 1,
      at: DAY_SECONDS,
      evidence_hash: 'a'.repeat(64),
      ...value,
    },
    admin.publicKey,
    txNo
  );
}

test('MayhemContract records reputation events and anchors rep snapshots with probation caps', async () => {
  const { admin, provider, outsider, storage, contract } = await setupReputationContract();

  const nonAdminEvent = await execute(
    contract,
    storage,
    'recordReputationEvent',
    {
      op: 'record_rep_event',
      provider: provider.publicKey,
      event_id: 'session-ok-1',
      kind: 'session_ok',
      paid_mu: 1_000,
      epoch: 1,
      at: DAY_SECONDS,
    },
    outsider.publicKey,
    4
  );
  assert.match(nonAdminEvent.message, /admin required/i);

  const first = await recordEvent(
    contract,
    storage,
    admin,
    provider,
    {
      op: 'record_rep_event',
      event_id: 'session-ok-1',
      kind: 'session_ok',
      paid_mu: 1_000,
    },
    5
  );
  assert.equal(first.ok, true, first.message);
  assert.equal(first.head.length, 64);

  const second = await recordEvent(
    contract,
    storage,
    admin,
    provider,
    {
      op: 'record_rep_event',
      event_id: 'session-partial-1',
      kind: 'session_partial',
      paid_mu: 250,
    },
    6
  );
  assert.equal(second.ok, true, second.message);
  assert.notEqual(second.head, first.head);

  const duplicate = await recordEvent(
    contract,
    storage,
    admin,
    provider,
    {
      op: 'record_rep_event',
      event_id: 'session-ok-1',
      kind: 'session_ok',
      paid_mu: 1_000,
    },
    7
  );
  assert.match(duplicate.message, /already recorded/i);

  const head = await storage.get(`ev/rep/head/${provider.publicKey}`);
  assert.deepEqual(head.value, {
    provider: provider.publicKey,
    head: second.head,
    count: 2,
    updated_at: makeTxKey(6),
  });

  const wrongHead = await execute(
    contract,
    storage,
    'anchorReputation',
    {
      op: 'anchor_reputation',
      provider: provider.publicKey,
      epoch: 1,
      folded_at: DAY_SECONDS,
      events_head: 'b'.repeat(64),
      r_bps: 8_700,
      raw_milli: 12_345,
      successful_sessions: 10,
    },
    admin.publicKey,
    8
  );
  assert.match(wrongHead.message, /head mismatch/i);

  const anchored = await execute(
    contract,
    storage,
    'anchorReputation',
    {
      op: 'anchor_reputation',
      provider: provider.publicKey,
      epoch: 1,
      folded_at: DAY_SECONDS,
      events_head: second.head,
      r_bps: 8_700,
      raw_milli: 12_345,
      successful_sessions: 10,
    },
    admin.publicKey,
    9
  );
  assert.deepEqual(anchored, {
    ok: true,
    op: 'anchorReputation',
    provider: provider.publicKey,
    epoch: 1,
    events_head: second.head,
  });

  const activeRep = await storage.get(`rep/${provider.publicKey}`);
  assert.deepEqual(activeRep.value, {
    provider: provider.publicKey,
    r: 0.87,
    r_bps: 8_700,
    raw: 12.345,
    raw_milli: 12_345,
    events_head: second.head,
    epoch: 1,
    folded_at: DAY_SECONDS,
    updated_at: makeTxKey(9),
    probation: {
      active: true,
      since: makeTxKey(3),
      since_seconds: 0,
      successful_sessions: 10,
      required_successful_sessions: 50,
      required_seconds: PROBATION_SECONDS,
      caps: {
        max_concurrent_sessions_per_user: 2,
        price_max_bps: 10_000,
        weight_bps: 5_000,
      },
    },
    provenance_violation: false,
  });

  const cleared = await execute(
    contract,
    storage,
    'anchorReputation',
    {
      op: 'anchor_reputation',
      provider: provider.publicKey,
      epoch: 2,
      folded_at: PROBATION_SECONDS,
      events_head: second.head,
      r_bps: 9_000,
      raw_milli: 20_000,
      successful_sessions: 50,
    },
    admin.publicKey,
    10
  );
  assert.equal(cleared.ok, true, cleared.message);

  const clearedRep = await storage.get(`rep/${provider.publicKey}`);
  assert.equal(clearedRep.value.probation.active, false);
  assert.equal(clearedRep.value.probation.successful_sessions, 50);

  const providerEntry = await storage.get(`prov/${provider.publicKey}`);
  assert.equal(providerEntry.value.probation.successful_sessions, 50);
  assert.equal(providerEntry.value.probation.since_seconds, 0);
});

test('MayhemContract reputation event log replays deterministically', async () => {
  const first = await setupReputationContract();
  const second = {
    admin: first.admin,
    provider: first.provider,
    outsider: first.outsider,
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
        type: 'registerProvider',
        value: providerRegistration,
        sender: ctx.provider.publicKey,
        txNo: 3,
      },
    ]) {
      const result = await execute(ctx.contract, ctx.storage, op.type, op.value, op.sender, op.txNo);
      assert.equal(result.ok, true, result.message);
    }

    const ok = await recordEvent(
      ctx.contract,
      ctx.storage,
      ctx.admin,
      ctx.provider,
      {
        op: 'record_rep_event',
        event_id: 'session-ok-1',
        kind: 'session_ok',
        paid_mu: 1_000,
      },
      4
    );
    assert.equal(ok.ok, true, ok.message);
    const fail = await recordEvent(
      ctx.contract,
      ctx.storage,
      ctx.admin,
      ctx.provider,
      {
        op: 'record_rep_event',
        event_id: 'session-fail-1',
        kind: 'session_fail',
        max_spend_mu: 5_000,
      },
      5
    );
    assert.equal(fail.ok, true, fail.message);

    const head = await ctx.storage.get(`ev/rep/head/${ctx.provider.publicKey}`);
    const anchored = await execute(
      ctx.contract,
      ctx.storage,
      'anchorReputation',
      {
        op: 'anchor_reputation',
        provider: ctx.provider.publicKey,
        epoch: 1,
        folded_at: DAY_SECONDS,
        events_head: head.value.head,
        r_bps: 4_200,
        raw_milli: -3_000,
        successful_sessions: 1,
      },
      ctx.admin.publicKey,
      6
    );
    assert.equal(anchored.ok, true, anchored.message);
  }

  first.storage = new MemoryStorage({ admin: first.admin.publicKey });
  first.contract = new MayhemContract({ peer: { wallet: makeVerifier(first.provider.wallet) } }, {});
  await applyScenario(first);
  await applyScenario(second);

  assert.equal(first.storage.snapshotBytes(), second.storage.snapshotBytes());
});

import assert from 'node:assert/strict';
import crypto from 'node:crypto';
import test from 'node:test';

import MayhemContract from '../contract/contract.js';
import {
  opaqueHash,
  recomputeEpoch,
  stableJson,
} from '../scripts/recompute-epoch-roots.mjs';

const hex = (value) => value.toString(16).padStart(64, '0').slice(-64);
const signature = (value) => value.repeat(128).slice(0, 128);

async function canonicalBundle(count, {
  order = Array.from({ length: count }, (_, index) => index + 1),
  settlementEpoch = 1,
  billingEpoch = settlementEpoch,
  mutateBody,
  mutateHead,
  mutateSnapshot,
} = {}) {
  const payoutRevision = 'a'.repeat(64);
  const heads = [];
  const identities = [];
  for (const ordinal of order) {
    const billingId = hex(ordinal);
    const body = {
      schema_version: 11,
      session_id: hex(10_000 + ordinal),
      billing_id: billingId,
      billing_attempt: 0,
      billing_epoch: billingEpoch,
      reservation_id: hex(20_000 + ordinal),
      payout_revision: payoutRevision,
      billing_prior_usage: {},
      billing_prior_au_owed_cum: '0',
      seq: 1,
      final: true,
      rail: ordinal % 2 === 0 ? 'tap' : 'fiat',
      user: hex(30_000 + ordinal),
      provider: ordinal % 2 === 0 ? 'b'.repeat(64) : 'c'.repeat(64),
      enclave_id: 'd'.repeat(64),
      model_id: 'fixture/model',
      price_ver: 1,
      locked_rate_map: [{ unit: 'output_token', per_unit_au: '1', granularity: 1 }],
      locked_per_req_au: '0',
      locked_min_session_au: '0',
      served_ctx: 1024,
      ctx_bracket: 'le32k',
      ctx_bracket_table_ver: 1,
      rules_ver: 1,
      usage: { output_token: 1 },
      au_owed_cum: '1',
      prompt_hash: 'e'.repeat(64),
      ts: 3_600,
    };
    mutateBody?.(body, ordinal);
    const receipt = {
      body,
      enclave_sig: signature('1'),
      enclave_pubkey: '2'.repeat(64),
      user_sig: signature('3'),
    };
    const receiptHash = await opaqueHash('mayhem-canonical-receipt-v1', receipt);
    const head = {
      epoch: settlementEpoch,
      billing_epoch: body.billing_epoch,
      billing_id: billingId,
      billing_attempt: 0,
      reservation_id: body.reservation_id,
      payout_revision: body.payout_revision,
      receipt_hash: receiptHash,
      incremental_au: '1',
      receipt,
    };
    mutateHead?.(head, ordinal);
    identities.push({ billing_id: billingId, billing_attempt: 0 });
    heads.push(head);
  }
  const metadata = {
    type: 'canonical_receipt_epoch_index',
    epoch: settlementEpoch,
    count,
    page_size: 128,
    page_count: Math.ceil(count / 128),
    revision: count,
    updated_at: 'f'.repeat(64),
  };
  const snapshot = {
    schema_version: 1,
    type: 'canonical_epoch_receipt_snapshot',
    settlement_epoch: settlementEpoch,
    metadata,
    identities,
    heads,
  };
  mutateSnapshot?.(snapshot);
  snapshot.snapshot_sha256 = crypto
    .createHash('sha256')
    .update(stableJson(snapshot))
    .digest('hex');
  return {
    epoch: settlementEpoch,
    params: {
      fee_bps: 1_500,
      max_apply_batch: 2_000,
      max_market_usage_entries: 5_000,
    },
    deposits: [],
    receipts: snapshot.heads,
    receipt_snapshot: snapshot,
    payouts: [],
    price_derivations: [],
    prior_earnings: {
      [`fiat/${'c'.repeat(64)}`]: '0',
      [`tap/${'b'.repeat(64)}`]: '0',
    },
    prior_fee_cum_au: '0',
    prior_burn_cum_au: '0',
  };
}

test('v17 recompute preserves insertion-order snapshots and emits targeted fields', async () => {
  const bundle = await canonicalBundle(2, { order: [2, 1] });
  assert.deepEqual(
    bundle.receipt_snapshot.identities.map((entry) => entry.billing_id),
    [hex(2), hex(1)],
  );
  const result = await recomputeEpoch(bundle);
  assert.equal(result.totals.use_count, 2);
  assert.equal(result.allocations.length, 2);
  assert.equal(result.apply_pages.length, 1);
  assert.equal(result.apply_pages[0].last_page, true);
  assert.deepEqual(result.apply_pages[0].receipt_index, bundle.receipt_snapshot.metadata);
  for (const allocation of result.allocations) {
    assert.deepEqual(Object.keys(allocation).sort(), [
      'au',
      'billing_attempt',
      'billing_epoch',
      'billing_id',
      'payout_revision',
      'provider',
      'rail',
      'receipt_hash',
      'receipt_seq',
      'session_id',
      'user',
    ]);
  }
});

test('v17 recompute pages more than 1000 exact receipt heads without peers', async () => {
  const receiptCount = Number(process.env.MAYHEM_RECOMPUTE_SCALE_RECEIPTS ?? 1_001);
  const bundle = await canonicalBundle(receiptCount);
  const result = await recomputeEpoch(bundle);
  assert.equal(result.allocations.length, receiptCount);
  assert.ok(result.apply_pages.length > 8);
  assert.equal(
    result.apply_pages.reduce((sum, page) => sum + page.allocations.length, 0),
    receiptCount,
  );
  for (const [index, page] of result.apply_pages.entries()) {
    assert.ok(page.allocations.length <= 128);
    assert.ok(page.max_feature_operation_json_bytes <= 60_000);
    assert.equal(page.page, index);
    assert.equal(page.last_page, index === result.apply_pages.length - 1);
    const allocations = page.allocations.reduce((sum, entry) => sum + BigInt(entry.au), 0n);
    const debits = page.debits.reduce((sum, entry) => sum + BigInt(entry.au), 0n);
    const earnings = page.earnings.reduce((sum, entry) => sum + BigInt(entry.gross_au), 0n);
    assert.equal(allocations, debits);
    assert.equal(allocations, earnings);
    if (page.last_page) {
      assert.equal(Object.hasOwn(page, 'market_usage'), true);
      assert.equal(Object.hasOwn(page, 'earning_finals'), true);
      assert.equal(
        page.market_usage.reduce((sum, entry) => sum + BigInt(entry.demand_au), 0n),
        BigInt(receiptCount),
      );
      assert.equal(page.earning_finals.length, result.totals.provider_count);
    } else {
      assert.equal(Object.hasOwn(page, 'market_usage'), false);
      assert.equal(Object.hasOwn(page, 'earning_finals'), false);
    }
  }
});

test('v17 recompute reserves feature bytes for final earning and market evidence', async () => {
  const result = await recomputeEpoch(await canonicalBundle(240, {
    mutateBody: (body, ordinal) => {
      const bucket = (ordinal % 60) + 1;
      body.provider = hex(40_000 + bucket);
      body.payout_revision = hex(50_000 + bucket);
      body.enclave_id = hex(60_000 + bucket);
      body.ctx_bracket = `le${bucket}k`;
    },
  }));

  assert.ok(result.apply_pages.length > 1);
  assert.equal(result.apply_pages.at(-1).earning_finals.length, 60);
  assert.equal(result.apply_pages.at(-1).market_usage.length, 60);
  assert.ok(
    result.apply_pages.at(-1).allocations.length <
      result.apply_pages.at(-2).allocations.length,
  );
  assert.equal(
    result.apply_pages.reduce((sum, page) => sum + page.allocations.length, 0),
    240,
  );
  for (const [index, page] of result.apply_pages.entries()) {
    assert.ok(page.max_feature_operation_json_bytes <= 60_000);
    assert.equal(page.last_page, index === result.apply_pages.length - 1);
  }
});

test('v17 recompute accepts late receipts in their current settlement epoch', async (t) => {
  for (const scenario of ['after-empty-seal', 'during-prior-pending-apply']) {
    await t.test(scenario, async () => {
      const result = await recomputeEpoch(await canonicalBundle(1, {
        settlementEpoch: 2,
        billingEpoch: 1,
      }));
      assert.equal(result.epoch, 2);
      assert.equal(result.receipt_index.epoch, 2);
      assert.equal(result.allocations[0].billing_epoch, 1);
      assert.equal(result.apply_pages[0].allocations[0].billing_epoch, 1);
    });
  }
});

test('v17 recompute and contract share canonical ordering across billing epochs', async () => {
  const result = await recomputeEpoch(await canonicalBundle(2, {
    settlementEpoch: 2,
    mutateBody: (body, ordinal) => {
      body.billing_epoch = ordinal === 1 ? 2 : 1;
      body.user = '9'.repeat(64);
      body.rail = 'fiat';
      body.provider = 'c'.repeat(64);
    },
  }));
  assert.deepEqual(
    result.allocations.map((allocation) => allocation.billing_id),
    [hex(2), hex(1)],
  );

  const {
    page_sha256: _pageSha256,
    max_feature_operation_json_bytes: _maxFeatureOperationJsonBytes,
    ...page
  } = result.apply_pages[0];
  const normalized = await new MayhemContract({}, {}).normalizeTargetedEpochFeatureValue({
    op: 'apply_targeted_epoch',
    epoch: 2,
    at: 7_200,
    epoch_commit_hash: '0'.repeat(64),
    ...page,
  });
  assert.equal(normalized instanceof Error, false, normalized.message);
});

test('v17 recompute accepts historical schema 10 and rejects v9, epoch, reservation, and payout drift', async () => {
  const mixed = await canonicalBundle(2, {
    mutateBody: (body, ordinal) => { body.schema_version = ordinal === 1 ? 10 : 11; },
  });
  const mixedResult = await recomputeEpoch(mixed);
  assert.equal(mixedResult.totals.use_count, 2);

  await assert.rejects(
    recomputeEpoch(await canonicalBundle(1, {
      mutateBody: (body) => { body.schema_version = 9; },
    })),
    /schema_version must be one of 10, 11/,
  );
  await assert.rejects(
    recomputeEpoch(await canonicalBundle(1, {
      mutateHead: (head) => { head.billing_epoch = 2; },
    })),
    /head billing_epoch does not match signed receipt body/,
  );
  await assert.rejects(
    recomputeEpoch(await canonicalBundle(1, {
      settlementEpoch: 1,
      billingEpoch: 2,
    })),
    /billing_epoch cannot be after its settlement epoch/,
  );
  await assert.rejects(
    recomputeEpoch(await canonicalBundle(1, {
      mutateSnapshot: (snapshot) => {
        snapshot.epoch = snapshot.settlement_epoch;
        delete snapshot.settlement_epoch;
      },
    })),
    /receipt_snapshot identity is invalid/,
  );
  await assert.rejects(
    recomputeEpoch(await canonicalBundle(1, {
      mutateHead: (head) => { head.reservation_id = '4'.repeat(64); },
    })),
    /reservation_id does not match/,
  );
  await assert.rejects(
    recomputeEpoch(await canonicalBundle(1, {
      mutateHead: (head) => { head.payout_revision = '5'.repeat(64); },
    })),
    /payout_revision does not match/,
  );
});

test('v17 recompute rejects replayed identities and conflicting canonical heads', async () => {
  const replay = await canonicalBundle(2);
  replay.receipt_snapshot.identities[1] = replay.receipt_snapshot.identities[0];
  replay.receipt_snapshot.heads[1] = replay.receipt_snapshot.heads[0];
  replay.receipts = replay.receipt_snapshot.heads;
  replay.receipt_snapshot.snapshot_sha256 = crypto
    .createHash('sha256')
    .update(stableJson({
      schema_version: replay.receipt_snapshot.schema_version,
      type: replay.receipt_snapshot.type,
      settlement_epoch: replay.receipt_snapshot.settlement_epoch,
      metadata: replay.receipt_snapshot.metadata,
      identities: replay.receipt_snapshot.identities,
      heads: replay.receipt_snapshot.heads,
    }))
    .digest('hex');
  await assert.rejects(recomputeEpoch(replay), /contain a replay/);

  await assert.rejects(
    recomputeEpoch(await canonicalBundle(1, {
      mutateHead: (head) => { head.receipt_hash = '6'.repeat(64); },
    })),
    /hash does not match signed receipt envelope/,
  );
});

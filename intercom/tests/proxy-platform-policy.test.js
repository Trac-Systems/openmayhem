import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import { proxyReceiptFixture } from './helpers/proxy-finance.js';
import { expiry, nextAttempt } from './helpers/proxy-closure.js';
import { prepareProxyExpiry } from '../contract/proxy-closure.js';
import { proxySettlementPolicyDigest } from '../contract/proxy-finance.js';

const settings = JSON.parse(fs.readFileSync(new URL('../../config/proxy/platform-commercial-policy-v1.json', import.meta.url)));

async function accepted(rail, family) {
  // Configure exact approved terms before either signature or reservation.
  // Defaults used by every other fixture are unchanged.
  const f = await proxyReceiptFixture(rail, family, null, true);
  const policy = structuredClone(settings.settlement_policy);
  const policyHash = await proxySettlementPolicyDigest(policy);
  assert.equal(policyHash, settings.settlement_policy_hash);
  assert.equal((await f.policy({ kind: 'set_settlement', policy_hash: policyHash, enabled: true, policy })).ok, true);
  Object.assign(f.terms, {
    settlement_policy_hash: policyHash,
    acceptance_expires_after_epoch: f.terms.billing_epoch + settings.buyer_lifetimes.acceptance_epochs,
    reservation_expires_after_epoch: f.terms.billing_epoch + settings.buyer_lifetimes.reservation_epochs,
    reservation_receipt_grace_epochs: settings.buyer_lifetimes.receipt_grace_epochs,
  });
  await f.apply(await f.prepare(f.authorize(f.terms)));
  return f;
}

for (const family of ['llm', 'decisions']) for (const rail of ['fiat', 'tnk', 'tap']) {
  test(`approved policy ${family}/${rail}: payable outcomes preserve native holds and original economics`, async () => {
    for (const outcome of settings.settlement_policy.payable_outcomes) {
      const f = await accepted(rail, family);
      const before = f.storage.snapshotBytes();
      // Running checkpoints and unsigned/invalid outcomes cannot acquire a charge.
      await assert.rejects(async () => f.finalize(await f.receipt({ final: false })), /payable|checkpoint/i);
      await assert.rejects(async () => f.finalize(await f.receipt({ outcome: 'upstream_error' })));
      const forged = await f.receipt({ outcome });
      forged.receipt.provider_sig = '0'.repeat(128);
      await assert.rejects(f.finalize(forged), /signature/i);
      assert.equal(f.storage.snapshotBytes(), before);

      const receipt = await f.receipt({ quantity: 2, outcome });
      f.reads.length = 0;
      await f.apply(await f.finalize(receipt));
      assert.ok(f.reads.length < 30, 'receipt work remains bounded');
      const paid = receipt.receipt.body.au_owed_cum;
      assert.equal((await f.read(f.summaryKey)).reserved_au, String(50n + BigInt(paid)));
      assert.deepEqual(await f.read(f.balanceKey), f.balance, 'balances debit only through canonical epoch settlement');
      assert.deepEqual(await f.read('payout/epoch/542'), { status: 'prepared', native: true });
      assert.deepEqual(await f.read('bal/existing-customer'), { fiat: '10', tnk: '20', tap: '30' });
      assert.equal((await f.read(f.ledger.receiptHeadKey(f.terms.billing_id, 1))).receipt.body.outcome, outcome);
      const after = f.storage.snapshotBytes();
      assert.equal((await f.finalize(receipt)).duplicate, true);
      assert.equal(f.storage.snapshotBytes(), after, 'replay cannot charge twice');

      const expiration = await expiry(f);
      await assert.rejects(prepareProxyExpiry(f.ledger, expiration,
        { ...f.context, epoch: expiration.expiry.body.observed_epoch }, f.peer.wallet.verify));
      assert.equal(f.storage.snapshotBytes(), after, 'expiry cannot erase a finalized payable receipt');
    }
  });

  test(`approved policy ${family}/${rail}: expiry requires canonical grace and blocks unresolved replay`, async () => {
    const f = await accepted(rail, family);
    const deadline = f.terms.billing_epoch + 30;
    assert.equal(f.terms.reservation_expires_after_epoch, f.terms.billing_epoch + 24);
    const e = await expiry(f);
    assert.equal(e.expiry.body.observed_epoch, deadline + 1);
    const before = f.storage.snapshotBytes();
    await assert.rejects(prepareProxyExpiry(f.ledger, e, { ...f.context, epoch: deadline }, f.peer.wallet.verify), /grace/i);
    assert.equal(f.storage.snapshotBytes(), before);
    const context = { ...f.context, epoch: deadline + 1 };
    const plan = await prepareProxyExpiry(f.ledger, e, context, f.peer.wallet.verify);
    assert.equal(plan.result.retry_safe, false);
    assert.equal(plan.result.released_au, f.terms.max_spend_au);
    await f.apply(plan);
    assert.equal((await f.read(f.summaryKey)).reserved_au, '50');
    assert.deepEqual(await f.read(f.balanceKey), f.balance);
    assert.deepEqual(await f.read('payout/epoch/542'), { status: 'prepared', native: true });
    const expired = f.storage.snapshotBytes();
    assert.equal((await prepareProxyExpiry(f.ledger, e, context, f.peer.wallet.verify)).duplicate, true);
    await assert.rejects(f.prepare(f.authorize(nextAttempt(f.terms))), /recover/i);
    await assert.rejects(f.finalize(await f.receipt()), /anchor|billing|session|reservation/i);
    assert.equal(f.storage.snapshotBytes(), expired, 'late receipt and retry cannot resurrect released exposure');
  });
}

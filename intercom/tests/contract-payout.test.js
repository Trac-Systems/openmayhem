import assert from 'node:assert/strict';
import test from 'node:test';
import MayhemContract from '../contract/contract.js';
import {
  MemoryStorage,
  execute,
  executeEpochApplyFeature,
  epochApplyFeatureKey,
  makeIdentity,
  makeTxKey,
  makeVerifier,
  signConsent,
  signProbeResult,
} from './helpers/contract.js';

const rulesHash = '9'.repeat(64);
const oneUsdAtTwoUsdPerTnk = '500000000000000000';
const DAY_SECONDS = 24 * 60 * 60;
const probeEnclaveId = '7'.repeat(64);
const probeBinaryHash = '8'.repeat(64);

async function setupPayoutContract() {
  const admin = await makeIdentity();
  const provider = await makeIdentity();
  const user = await makeIdentity();
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
      value: { op: 'register_provider' },
      sender: provider.publicKey,
      txNo: 3,
    },
    {
      type: 'setProviderPayout',
      value: {
        op: 'set_provider_payout',
        provider: provider.publicKey,
        payout_addr: 'trac1providerpayouttarget',
        payout_method: 'tnk',
      },
      sender: admin.publicKey,
      txNo: 4,
    },
    {
      type: 'rateOracle',
      value: {
        op: 'rate_oracle',
        tnk_usd_e6: 2_000_000,
        source: 'gate-spot',
        ts: 1_000,
      },
      sender: admin.publicKey,
      txNo: 5,
    },
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }

  await storage.put(`bal/${user.publicKey}`, {
    user: user.publicKey,
    denom: 'mu_usd',
    mu: 5_000_000,
    updated_epoch: 0,
    updated_at: null,
  });
  return { admin, provider, user, storage, contract };
}

const epochApply = (epoch, user, provider, grossMu, overrides = {}) => ({
  op: 'epoch_apply',
  epoch,
  at: epoch * 3_600,
  debits: [{ user, mu: grossMu }],
  earnings: [{ provider, gross_mu: grossMu }],
  ...overrides,
});

const payoutConfirm = (provider, overrides = {}) => ({
  op: 'payout_confirm',
  epoch: 169,
  who: provider,
  mu: 1_000_000,
  tnk_e18: oneUsdAtTwoUsdPerTnk,
  msb_tx_hash: 'a'.repeat(64),
  at: 1_900,
  ...overrides,
});

const fiatPayoutConfirm = (provider, rail, externalRef, overrides = {}) => ({
  op: 'payout_confirm',
  epoch: 169,
  who: provider,
  rail,
  mu: 1_000_000,
  external_ref: externalRef,
  fiat_currency: 'usd',
  fiat_amount_minor: 100,
  at: 1_900,
  ...overrides,
});

const signedCanaryProbe = (provider, auditor, overrides = {}) => {
  const value = {
    op: 'probe_result',
    probe_id: 'probe-pass-1',
    probe_kind: 'canary',
    provider: provider.publicKey,
    enclave_id: probeEnclaveId,
    binary_hash: probeBinaryHash,
    epoch: 1,
    at: DAY_SECONDS + 7_200,
    canary_set: 'canary-dev-v1',
    match_bps: 10_000,
    pass: true,
    session_receipt_hash: 'a'.repeat(64),
    evidence_hash: 'b'.repeat(64),
    ...overrides,
  };
  value.auditor_sig = signProbeResult(auditor.wallet, value, auditor.publicKey);
  return value;
};

test('MayhemContract setProviderPayout stamps admin authority evidence', async () => {
  const { admin, provider, storage, contract } = await setupPayoutContract();
  const registered = (await storage.get(`prov/${provider.publicKey}`)).value;
  assert.deepEqual(registered.payout, {
    addr: 'trac1providerpayouttarget',
    method: 'tnk',
    set_by: admin.publicKey,
    set_by_role: 'admin',
    set_at: makeTxKey(4),
  });

  const retargeted = await execute(
    contract,
    storage,
    'setProviderPayout',
    {
      op: 'set_provider_payout',
      provider: provider.publicKey,
      payout_addr: 'acct_test_provider',
      payout_method: 'stripe',
      payout_currency: 'eur',
    },
    admin.publicKey,
    6
  );
  assert.equal(retargeted.ok, true, retargeted.message);
  const updated = (await storage.get(`prov/${provider.publicKey}`)).value;
  assert.deepEqual(updated.payout, {
    addr: 'acct_test_provider',
    method: 'stripe',
    currency: 'eur',
    set_by: admin.publicKey,
    set_by_role: 'admin',
    set_at: makeTxKey(6),
  });
});

test('MayhemContract payoutConfirm releases earnings only after challenge plus holdback lock', async () => {
  const { admin, provider, user, storage, contract } = await setupPayoutContract();
  const applyValue = epochApply(1, user.publicKey, provider.publicKey, 2_000_000);
  const applyKey = await epochApplyFeatureKey(contract, applyValue);

  const settled = await executeEpochApplyFeature(
    contract,
    storage,
    applyValue,
    admin.publicKey
  );
  assert.equal(settled.ok, true, settled.message);
  assert.deepEqual((await storage.get(`earn/${provider.publicKey}`)).value, {
    provider: provider.publicKey,
    denom: 'mu_usd',
    total_mu: 1_700_000,
    held_mu: 1_700_000,
    paid_cum_mu: 0,
    holdbacks: [{ epoch: 1, mu: 1_700_000 }],
    updated_epoch: 1,
    updated_at: applyKey,
    last_holdback_release_epoch: 1,
  });

  const beforeEarly = storage.snapshotBytes();
  const early = await execute(
    contract,
    storage,
    'payoutConfirm',
    payoutConfirm(provider.publicKey, { epoch: 100 }),
    admin.publicKey,
    7
  );
  assert.match(early.message, /below payout_min/i);
  assert.equal(storage.snapshotBytes(), beforeEarly);

  const paid = await execute(
    contract,
    storage,
    'payoutConfirm',
    payoutConfirm(provider.publicKey),
    admin.publicKey,
    8
  );
  assert.equal(paid.ok, true, paid.message);
  assert.equal(paid.kind, 'provider');
  assert.equal(paid.epoch, 169);
  assert.equal(paid.payout_root.length, 64);

  assert.deepEqual((await storage.get(`earn/${provider.publicKey}`)).value, {
    provider: provider.publicKey,
    denom: 'mu_usd',
    total_mu: 1_700_000,
    held_mu: 0,
    paid_cum_mu: 1_000_000,
    holdbacks: [],
    updated_epoch: 1,
    updated_at: makeTxKey(8),
    last_holdback_release_epoch: 169,
    last_payout_rail: 'tnk',
    last_payout_rate_ts: 1_000,
    last_payout_msb_tx_hash: 'a'.repeat(64),
  });
  const payRoot = (await storage.get('ev/pay/169')).value;
  assert.equal(payRoot.type, 'payout_root');
  assert.equal(payRoot.epoch, 169);
  assert.equal(payRoot.count, 1);
  assert.equal(payRoot.mu_total, 1_000_000);
  assert.equal(payRoot.merkle_root, paid.payout_root);
});

test('MayhemContract payoutConfirm keeps configured probe-gated earnings held until canary passes', async () => {
  const { admin, provider, user, storage, contract } = await setupPayoutContract();
  const auditor = await makeIdentity();

  const params = await execute(
    contract,
    storage,
    'setParams',
    {
      op: 'set_params',
      submitted_at: 0,
      effective_at: DAY_SECONDS,
      values: {
        holdback_epochs: 1,
        challenge_epochs: 1,
        canary_probe_holdback_bps: 5_000,
        canary_probe_release_min_passes: 1,
        payout_min_mu: 0,
        rate_staleness_seconds: 86_400,
      },
    },
    admin.publicKey,
    6
  );
  assert.equal(params.ok, true, params.message);

  const freshRate = await execute(
    contract,
    storage,
    'rateOracle',
    {
      op: 'rate_oracle',
      tnk_usd_e6: 2_000_000,
      source: 'gate-spot',
      ts: DAY_SECONDS,
    },
    admin.publicKey,
    7
  );
  assert.equal(freshRate.ok, true, freshRate.message);

  await storage.put(`auditor/${auditor.publicKey}`, {
    auditor: auditor.publicKey,
    status: 'active',
    registered_at: makeTxKey(8),
    registered_at_seconds: 0,
    accredited_by: admin.publicKey,
    successful_probes: 0,
    submitted_probes: 0,
    false_reports: 0,
    updated_at: makeTxKey(8),
  });
  await storage.put(`enclave/${probeEnclaveId}`, {
    enclave_id: probeEnclaveId,
    binary_hash: probeBinaryHash,
    status: 'active',
  });
  await storage.put('catalog/current', {
    status: 'active',
    canaries: [{ set_id: 'canary-dev-v1' }],
  });

  const applied = await executeEpochApplyFeature(
    contract,
    storage,
    epochApply(1, user.publicKey, provider.publicKey, 2_000_000, { at: DAY_SECONDS }),
    admin.publicKey
  );
  assert.equal(applied.ok, true, applied.message);

  const overGated = await execute(
    contract,
    storage,
    'payoutConfirm',
    payoutConfirm(provider.publicKey, {
      epoch: 2,
      at: DAY_SECONDS + 3_600,
      mu: 1_000_000,
      msb_tx_hash: 'b'.repeat(64),
    }),
    admin.publicKey,
    10
  );
  assert.match(overGated.message, /insufficient released earnings/i);

  const ungated = await execute(
    contract,
    storage,
    'payoutConfirm',
    payoutConfirm(provider.publicKey, {
      epoch: 2,
      at: DAY_SECONDS + 3_600,
      mu: 850_000,
      msb_tx_hash: 'c'.repeat(64),
    }),
    admin.publicKey,
    11
  );
  assert.equal(ungated.ok, true, ungated.message);
  assert.deepEqual((await storage.get(`earn/${provider.publicKey}`)).value.holdbacks, [
    { epoch: 1, mu: 850_000, probe_gate: true },
  ]);

  const probe = await execute(
    contract,
    storage,
    'probeResult',
    signedCanaryProbe(provider, auditor),
    auditor.publicKey,
    12
  );
  assert.equal(probe.ok, true, probe.message);
  assert.equal(probe.probe_pass_record.pass_count, 1);

  const gated = await execute(
    contract,
    storage,
    'payoutConfirm',
    payoutConfirm(provider.publicKey, {
      epoch: 3,
      at: DAY_SECONDS + 7_200,
      mu: 850_000,
      msb_tx_hash: 'd'.repeat(64),
    }),
    admin.publicKey,
    13
  );
  assert.equal(gated.ok, true, gated.message);
  assert.deepEqual((await storage.get(`earn/${provider.publicKey}`)).value, {
    provider: provider.publicKey,
    denom: 'mu_usd',
    total_mu: 1_700_000,
    held_mu: 0,
    paid_cum_mu: 1_700_000,
    holdbacks: [],
    updated_epoch: 1,
    updated_at: makeTxKey(13),
    last_holdback_release_epoch: 3,
    last_payout_rail: 'tnk',
    last_payout_rate_ts: DAY_SECONDS,
    last_payout_msb_tx_hash: 'd'.repeat(64),
  });
});

test('MayhemContract payoutConfirm rejects non-admin payout target provenance', async () => {
  const { admin, provider, user, storage, contract } = await setupPayoutContract();
  const settled = await executeEpochApplyFeature(
    contract,
    storage,
    epochApply(1, user.publicKey, provider.publicKey, 2_000_000),
    admin.publicKey
  );
  assert.equal(settled.ok, true, settled.message);

  const record = (await storage.get(`prov/${provider.publicKey}`)).value;
  await storage.put(`prov/${provider.publicKey}`, {
    ...record,
    payout: {
      ...record.payout,
      set_by: provider.publicKey,
      set_by_role: 'provider',
    },
  });

  const before = storage.snapshotBytes();
  const rejected = await execute(
    contract,
    storage,
    'payoutConfirm',
    payoutConfirm(provider.publicKey),
    admin.publicKey,
    7
  );
  assert.match(
    rejected.message,
    /payout target was not set by the current admin|payout target must be admin-set/i
  );
  assert.equal(storage.snapshotBytes(), before);
});

test('MayhemContract payoutConfirm requires admin role marker on payout targets', async () => {
  const { admin, provider, user, storage, contract } = await setupPayoutContract();
  const record = (await storage.get(`prov/${provider.publicKey}`)).value;
  const missingRolePayout = { ...record.payout };
  delete missingRolePayout.set_by_role;
  await storage.put(`prov/${provider.publicKey}`, {
    ...record,
    payout: missingRolePayout,
  });

  const settled = await executeEpochApplyFeature(
    contract,
    storage,
    epochApply(1, user.publicKey, provider.publicKey, 2_000_000),
    admin.publicKey
  );
  assert.equal(settled.ok, true, settled.message);

  const rejected = await execute(
    contract,
    storage,
    'payoutConfirm',
    payoutConfirm(provider.publicKey),
    admin.publicKey,
    7
  );
  assert.match(rejected.message, /payout target must be admin-set/i);
});

test('MayhemContract payoutConfirm fails closed without current admin key', async () => {
  const admin = await makeIdentity();
  const provider = await makeIdentity();
  const user = await makeIdentity();
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
      value: { op: 'register_provider' },
      sender: provider.publicKey,
      txNo: 3,
    },
    {
      type: 'setProviderPayout',
      value: {
        op: 'set_provider_payout',
        provider: provider.publicKey,
        payout_addr: 'trac1providerpayouttarget',
        payout_method: 'tnk',
      },
      sender: admin.publicKey,
      txNo: 4,
    },
    {
      type: 'rateOracle',
      value: {
        op: 'rate_oracle',
        tnk_usd_e6: 2_000_000,
        source: 'gate-spot',
        ts: 1_000,
      },
      sender: admin.publicKey,
      txNo: 5,
    },
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }

  await storage.put(`bal/${user.publicKey}`, {
    user: user.publicKey,
    denom: 'mu_usd',
    mu: 5_000_000,
    updated_epoch: 0,
    updated_at: null,
  });
  const settled = await executeEpochApplyFeature(
    contract,
    storage,
    epochApply(1, user.publicKey, provider.publicKey, 2_000_000),
    admin.publicKey
  );
  assert.equal(settled.ok, true, settled.message);
  await storage.del('admin');

  const before = storage.snapshotBytes();
  const rejected = await execute(
    contract,
    storage,
    'payoutConfirm',
    payoutConfirm(provider.publicKey),
    admin.publicKey,
    7
  );
  assert.match(rejected.message, /admin required/i);
  assert.equal(storage.snapshotBytes(), before);
  assert.equal(await storage.get('ev/pay/169'), null);
});

test('MayhemContract payoutConfirm accepts admin-set fiat payout rails', async () => {
  for (const [rail, target, externalRef] of [
    ['stripe', 'acct_test_provider', 'tr_test_provider_payout'],
    ['coinbase', 'paymentMethod_test_provider', 'transfer_test_provider_payout'],
  ]) {
    const { admin, provider, user, storage, contract } = await setupPayoutContract();
    const retargeted = await execute(
      contract,
      storage,
      'setProviderPayout',
      {
        op: 'set_provider_payout',
        provider: provider.publicKey,
        payout_addr: target,
        payout_method: rail,
        payout_currency: rail === 'stripe' ? 'eur' : 'usd',
      },
      admin.publicKey,
      6
    );
    assert.equal(retargeted.ok, true, retargeted.message);

    const settled = await executeEpochApplyFeature(
      contract,
      storage,
      epochApply(1, user.publicKey, provider.publicKey, 2_000_000),
      admin.publicKey
    );
    assert.equal(settled.ok, true, settled.message);

    const wrongRail = await execute(
      contract,
      storage,
      'payoutConfirm',
      fiatPayoutConfirm(
        provider.publicKey,
        rail === 'stripe' ? 'coinbase' : 'stripe',
        'wrong_rail_transfer',
        { fiat_currency: rail === 'stripe' ? 'usd' : 'eur' }
      ),
      admin.publicKey,
      8
    );
    assert.match(wrongRail.message, /payout target for rail/i);

    const confirmed = await execute(
      contract,
      storage,
      'payoutConfirm',
      fiatPayoutConfirm(provider.publicKey, rail, externalRef, {
        fiat_currency: rail === 'stripe' ? 'eur' : 'usd',
      }),
      admin.publicKey,
      9
    );
    assert.equal(confirmed.ok, true, confirmed.message);
    assert.equal(confirmed.op, 'payoutConfirm');
    assert.equal(confirmed.kind, 'provider');
    assert.equal(confirmed.rail, rail);
    assert.equal(confirmed.who, provider.publicKey);
    assert.equal(confirmed.mu, 1_000_000);
    assert.equal(confirmed.epoch, 169);
    assert.equal(confirmed.fiat_currency, rail === 'stripe' ? 'eur' : 'usd');
    assert.equal(confirmed.fiat_amount_minor, 100);
    assert.equal(confirmed.payout_root.length, 64);
    assert.equal(confirmed.external_ref_hash.length, 64);

    const earning = (await storage.get(`earn/${provider.publicKey}`)).value;
    assert.equal(earning.paid_cum_mu, 1_000_000);
    assert.equal(earning.held_mu, 0);
    assert.equal(earning.last_payout_rail, rail);
    assert.equal(earning.last_payout_fiat_currency, rail === 'stripe' ? 'eur' : 'usd');
    assert.equal(earning.last_payout_external_ref_hash, confirmed.external_ref_hash);
    assert.equal(earning.last_payout_rate_ts, undefined);
    assert.equal(earning.last_payout_msb_tx_hash, undefined);

    const payRoot = (await storage.get('ev/pay/169')).value;
    assert.equal(payRoot.type, 'payout_root');
    assert.equal(payRoot.count, 1);
    assert.equal(payRoot.mu_total, 1_000_000);
    assert.equal(payRoot.merkle_root, confirmed.payout_root);
  }
});

test('MayhemContract payoutConfirm clears previous rail metadata on rail switch', async () => {
  const { admin, provider, user, storage, contract } = await setupPayoutContract();
  const settled = await executeEpochApplyFeature(
    contract,
    storage,
    epochApply(1, user.publicKey, provider.publicKey, 3_000_000),
    admin.publicKey
  );
  assert.equal(settled.ok, true, settled.message);

  const tnk = await execute(
    contract,
    storage,
    'payoutConfirm',
    payoutConfirm(provider.publicKey),
    admin.publicKey,
    7
  );
  assert.equal(tnk.ok, true, tnk.message);
  let earning = (await storage.get(`earn/${provider.publicKey}`)).value;
  assert.equal(earning.last_payout_rail, 'tnk');
  assert.equal(earning.last_payout_msb_tx_hash, 'a'.repeat(64));
  assert.equal(earning.last_payout_external_ref_hash, undefined);

  const retargeted = await execute(
    contract,
    storage,
    'setProviderPayout',
    {
      op: 'set_provider_payout',
      provider: provider.publicKey,
      payout_addr: 'acct_test_provider',
      payout_method: 'stripe',
      payout_currency: 'usd',
    },
    admin.publicKey,
    8
  );
  assert.equal(retargeted.ok, true, retargeted.message);

  const stripe = await execute(
    contract,
    storage,
    'payoutConfirm',
    fiatPayoutConfirm(provider.publicKey, 'stripe', 'tr_second_payout'),
    admin.publicKey,
    9
  );
  assert.equal(stripe.ok, true, stripe.message);
  earning = (await storage.get(`earn/${provider.publicKey}`)).value;
  assert.equal(earning.paid_cum_mu, 2_000_000);
  assert.equal(earning.last_payout_rail, 'stripe');
  assert.equal(earning.last_payout_external_ref_hash, stripe.external_ref_hash);
  assert.equal(earning.last_payout_rate_ts, undefined);
  assert.equal(earning.last_payout_msb_tx_hash, undefined);
});

test('MayhemContract payoutConfirm sweeps router fees into fee evidence', async () => {
  const { admin, storage, contract } = await setupPayoutContract();
  await storage.put('fee/cum', {
    denom: 'mu_usd',
    cum_mu: 2_000_000,
    swept_cum_mu: 0,
    settled_cum_mu: 2_000_000,
    updated_epoch: 1,
    updated_at: null,
    last_apply_hash: null,
    last_fee_bps: 1_500,
  });
  await storage.put('ev/fee/169', {
    type: 'fee_root',
    epoch: 169,
    merkle_root: 'f'.repeat(64),
    mu_fee_epoch: 0,
    mu_fee_cum: 2_000_000,
    sweep_msb_tx_hash: null,
    ts: 1_900,
    updated_at: null,
  });

  const swept = await execute(
    contract,
    storage,
    'payoutConfirm',
    payoutConfirm('treasury', {
      kind: 'fee_sweep',
      msb_tx_hash: 'b'.repeat(64),
    }),
    admin.publicKey,
    6
  );
  assert.deepEqual(swept, {
    ok: true,
    op: 'payoutConfirm',
    kind: 'fee_sweep',
    who: 'treasury',
    mu: 1_000_000,
    epoch: 169,
    rate_ts: 1_000,
  });
  assert.deepEqual((await storage.get('fee/cum')).value, {
    denom: 'mu_usd',
    cum_mu: 2_000_000,
    swept_cum_mu: 1_000_000,
    settled_cum_mu: 2_000_000,
    updated_epoch: 1,
    updated_at: makeTxKey(6),
    last_apply_hash: null,
    last_fee_bps: 1_500,
    last_sweep_rate_ts: 1_000,
    last_sweep_msb_tx_hash: 'b'.repeat(64),
  });
  assert.deepEqual((await storage.get('ev/fee/169')).value, {
    type: 'fee_root',
    epoch: 169,
    merkle_root: 'f'.repeat(64),
    mu_fee_epoch: 0,
    mu_fee_cum: 2_000_000,
    sweep_msb_tx_hash: 'b'.repeat(64),
    ts: 1_900,
    updated_at: makeTxKey(6),
    sweep_mu: 1_000_000,
    sweep_tnk_e18: oneUsdAtTwoUsdPerTnk,
    sweep_rate_ts: 1_000,
    swept_cum_mu: 1_000_000,
  });
});

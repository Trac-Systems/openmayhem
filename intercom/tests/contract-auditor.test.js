import assert from 'node:assert/strict';
import test from 'node:test';
import MayhemContract from '../contract/contract.js';
import {
  MemoryStorage,
  execute,
  makeIdentity,
  makeTxKey,
  makeVerifier,
  signProbeResult,
  signConsent,
} from './helpers/contract.js';

const rulesHash = 'a9'.repeat(32);
const enclaveId = 'b9'.repeat(32);
const binaryHash = 'c9'.repeat(32);
const modelId = 'mayhem/auditor-canary-model';

const providerRegistration = {
  op: 'register_provider',
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
    {
      type: 'registerEnclave',
      value: {
        op: 'register_enclave',
        enclave_id: enclaveId,
        model_id: modelId,
        model_class: 'text-generation',
        backend: 'llama.cpp',
        artifact_root: 'a'.repeat(64),
        artifact_root_kind: 'blake3_merkle_v1',
        artifact_source: {
          kind: 'huggingface',
          repo: 'mayhem-test/auditor-canary-model',
          revision: '1'.repeat(40),
          path: 'auditor-canary-model.gguf',
        },
        manifest_hash: 'b'.repeat(64),
        att_tier: 1,
        binary_hash: binaryHash,
        caps: { chat: true, tools: false, ctx: 32768 },
      },
      sender: admin.publicKey,
      txNo: 5,
    },
    {
      type: 'publishCatalog',
      value: {
        op: 'publish_catalog',
        catalog_id: 'mayhem-models',
        source_kind: 'huggingface',
        catalog_url: 'https://huggingface.co/TracNetwork/mayhem-catalog/resolve/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/models.json',
        signature_url: 'https://huggingface.co/TracNetwork/mayhem-catalog/resolve/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/models.json.sig',
        catalog_hash: '8'.repeat(64),
        signature_hash: '9'.repeat(64),
        key_id: 'mayhem-catalog-tracnetwork-v1',
        public_key: 'a'.repeat(64),
        model_count: 1,
        artifact_count: 1,
        canaries: [{
          set_id: 'canary-dev-v1',
          url: 'https://huggingface.co/TracNetwork/mayhem-catalog/resolve/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/canaries/canary-dev-v1.json',
          hash: 'b'.repeat(64),
        }],
      },
      sender: admin.publicKey,
      txNo: 6,
    },
  ]) {
    const result = await execute(contract, storage, op.type, op.value, op.sender, op.txNo);
    assert.equal(result.ok, true, result.message);
  }

  return { admin, provider, auditor, storage, contract };
}

const canaryProbe = (provider, auditor, overrides = {}, options = {}) => {
  const value = {
    op: 'probe_result',
    probe_id: 'canary-good',
    probe_kind: 'canary',
    provider: provider.publicKey,
    enclave_id: enclaveId,
    binary_hash: binaryHash,
    epoch: 1,
    at: 10_000,
    canary_set: 'canary-dev-v1',
    verification_method: 'token_fingerprint',
    match_bps: 9_700,
    pass: true,
    session_receipt_hash: 'c'.repeat(64),
    evidence_hash: 'd'.repeat(64),
    ...overrides,
  };
  if (options.sign !== false && value.auditor_sig === undefined) {
    value.auditor_sig = signProbeResult(auditor.wallet, value, auditor.publicKey);
  }
  return value;
};

test('MayhemContract auditor probes write evidence, uptime ticks, and canary violation bans', async () => {
  const { admin, provider, auditor, storage, contract } = await setupAuditorContract();

  const beforeRegister = await execute(
    contract,
    storage,
    'probeResult',
    canaryProbe(provider, auditor),
    auditor.publicKey,
    7
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
    8
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
    registered_at: makeTxKey(8),
    registered_at_seconds: 0,
    accredited_by: admin.publicKey,
    successful_probes: 0,
    submitted_probes: 0,
    false_reports: 0,
    updated_at: makeTxKey(8),
  });

  const missingEnclave = await execute(
    contract,
    storage,
    'probeResult',
    canaryProbe(provider, auditor, { probe_id: 'canary-no-enclave', enclave_id: undefined }),
    auditor.publicKey,
    9
  );
  assert.match(missingEnclave.message, /requires enclave_id/i);

  const missingBinary = await execute(
    contract,
    storage,
    'probeResult',
    canaryProbe(provider, auditor, { probe_id: 'canary-no-binary', binary_hash: undefined }),
    auditor.publicKey,
    9
  );
  assert.match(missingBinary.message, /requires binary_hash/i);

  const missingReceipt = await execute(
    contract,
    storage,
    'probeResult',
    canaryProbe(provider, auditor, { probe_id: 'canary-no-receipt', session_receipt_hash: undefined }),
    auditor.publicKey,
    9
  );
  assert.match(missingReceipt.message, /requires session_receipt_hash/i);

  const missingEvidence = await execute(
    contract,
    storage,
    'probeResult',
    canaryProbe(provider, auditor, { probe_id: 'canary-no-evidence', evidence_hash: undefined }),
    auditor.publicKey,
    9
  );
  assert.match(missingEvidence.message, /requires evidence_hash/i);

  const missingVerificationMethod = await execute(
    contract,
    storage,
    'probeResult',
    canaryProbe(provider, auditor, { probe_id: 'canary-no-method', verification_method: undefined }),
    auditor.publicKey,
    9
  );
  assert.match(missingVerificationMethod.message, /requires verification_method/i);

  const unsupportedVerificationMethod = await execute(
    contract,
    storage,
    'probeResult',
    canaryProbe(provider, auditor, { probe_id: 'canary-bad-method', verification_method: 'provider-local-screenshot' }),
    auditor.publicKey,
    9
  );
  assert.match(unsupportedVerificationMethod.message, /unsupported canary verification_method/i);

  const unsigned = await execute(
    contract,
    storage,
    'probeResult',
    canaryProbe(provider, auditor, { probe_id: 'canary-unsigned' }, { sign: false }),
    auditor.publicKey,
    9
  );
  assert.match(unsigned.message, /requires auditor_sig/i);

  const forged = await execute(
    contract,
    storage,
    'probeResult',
    canaryProbe(provider, auditor, {
      probe_id: 'canary-forged',
      auditor_sig: '1'.repeat(128),
    }),
    auditor.publicKey,
    9
  );
  assert.match(forged.message, /invalid canary auditor signature/i);

  const fabricatedFailure = await execute(
    contract,
    storage,
    'probeResult',
    canaryProbe(provider, auditor, {
      probe_id: 'canary-fabricated-failure',
      match_bps: 1_000,
      pass: false,
      auditor_sig: '2'.repeat(128),
    }),
    auditor.publicKey,
    9
  );
  assert.match(fabricatedFailure.message, /invalid canary auditor signature/i);

  const unbound = canaryProbe(provider, auditor, { probe_id: 'canary-unbound' });
  unbound.session_receipt_hash = 'e'.repeat(64);
  const unboundResult = await execute(
    contract,
    storage,
    'probeResult',
    unbound,
    auditor.publicKey,
    9
  );
  assert.match(unboundResult.message, /invalid canary auditor signature/i);

  const wrongBinary = await execute(
    contract,
    storage,
    'probeResult',
    canaryProbe(provider, auditor, {
      probe_id: 'canary-wrong-binary',
      binary_hash: 'f'.repeat(64),
    }),
    auditor.publicKey,
    9
  );
  assert.match(wrongBinary.message, /binary_hash is not approved/i);

  const unpublishedCanary = await execute(
    contract,
    storage,
    'probeResult',
    canaryProbe(provider, auditor, {
      probe_id: 'canary-unpublished',
      canary_set: 'missing-canary',
    }),
    auditor.publicKey,
    9
  );
  assert.match(unpublishedCanary.message, /not published/i);

  assert.equal((await storage.get(`prov/${provider.publicKey}`)).value.status, 'active');
  assert.equal(await storage.get('ev/probe/canary-forged'), null);
  assert.equal(await storage.get('ev/probe/canary-fabricated-failure'), null);

  const canaryOk = await execute(
    contract,
    storage,
    'probeResult',
    canaryProbe(provider, auditor),
    auditor.publicKey,
    9
  );
  assert.deepEqual(canaryOk, {
    ok: true,
    op: 'probeResult',
    probe_id: 'canary-good',
    provider: provider.publicKey,
    pass: true,
    probe_pass_record: {
      provider: provider.publicKey,
      epoch: 1,
      pass_count: 1,
      last_probe_id: 'canary-good',
      last_evidence_hash: 'd'.repeat(64),
      updated_at: makeTxKey(9),
    },
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
    10
  );
  assert.equal(uptime.ok, true, uptime.message);
  assert.equal(uptime.pass, true);

  const mismatch = await execute(
    contract,
    storage,
    'probeResult',
    canaryProbe(provider, auditor, {
      probe_id: 'canary-bad',
      match_bps: 1_000,
      pass: false,
      evidence_hash: 'f'.repeat(64),
    }),
    auditor.publicKey,
    11
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
  assert.equal(goodProbe.value.verification_method, 'token_fingerprint');
  assert.equal(goodProbe.value.probe_reward_au, '5000000000000000');
  const uptimeProbe = await storage.get('ev/probe/uptime-1');
  assert.equal(uptimeProbe.value.pass, true);
  assert.equal(uptimeProbe.value.probe_reward_au, '5000000000000000');
  assert.equal(await storage.get(`bal/${provider.publicKey}/fiat`), null);
  assert.equal(await storage.get(`bal/${auditor.publicKey}/fiat`), null);

  const badProbe = await storage.get('ev/probe/canary-bad');
  assert.equal(badProbe.value.pass, false);
  assert.equal(badProbe.value.provenance_violation, true);
  assert.equal(badProbe.value.slash.reason, 'canary_mismatch');

  const slash = await storage.get(`ev/slash/${provider.publicKey}/${makeTxKey(11)}`);
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

test('MayhemContract keeps provider and auditor keys separate', async () => {
  const { admin, provider, auditor, storage, contract } = await setupAuditorContract();

  const providerSelfAuditor = await execute(
    contract,
    storage,
    'auditorRegister',
    {
      op: 'auditor_register',
      registered_at_seconds: 31 * 24 * 60 * 60,
    },
    provider.publicKey,
    5
  );
  assert.match(providerSelfAuditor.message, /provider keys cannot register as auditors/i);

  const adminAccreditsProvider = await execute(
    contract,
    storage,
    'auditorRegister',
    {
      op: 'auditor_register',
      auditor: provider.publicKey,
      registered_at_seconds: 0,
    },
    admin.publicKey,
    6
  );
  assert.match(adminAccreditsProvider.message, /provider keys cannot register as auditors/i);

  const registeredAuditor = await execute(
    contract,
    storage,
    'auditorRegister',
    {
      op: 'auditor_register',
      auditor: auditor.publicKey,
      registered_at_seconds: 0,
    },
    admin.publicKey,
    7
  );
  assert.equal(registeredAuditor.ok, true, registeredAuditor.message);

  const auditorAsProvider = await execute(
    contract,
    storage,
    'registerProvider',
    providerRegistration,
    auditor.publicKey,
    8
  );
  assert.match(auditorAsProvider.message, /auditor keys cannot register as providers/i);

  await storage.put(`auditor/${provider.publicKey}`, {
    auditor: provider.publicKey,
    status: 'active',
    registered_at: makeTxKey(9),
    registered_at_seconds: 0,
    accredited_by: admin.publicKey,
    successful_probes: 0,
    submitted_probes: 0,
    false_reports: 0,
    updated_at: makeTxKey(9),
  });
  const providerProbe = await execute(
    contract,
    storage,
    'probeResult',
    canaryProbe(provider, auditor),
    provider.publicKey,
    9
  );
  assert.match(providerProbe.message, /provider keys cannot submit auditor probes/i);
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
        type: 'registerEnclave',
        value: {
          op: 'register_enclave',
          enclave_id: enclaveId,
          model_id: modelId,
          model_class: 'text-generation',
          backend: 'llama.cpp',
          artifact_root: 'a'.repeat(64),
          artifact_root_kind: 'blake3_merkle_v1',
          artifact_source: {
            kind: 'huggingface',
            repo: 'mayhem-test/auditor-canary-model',
            revision: '1'.repeat(40),
            path: 'auditor-canary-model.gguf',
          },
          manifest_hash: 'b'.repeat(64),
          att_tier: 1,
          binary_hash: binaryHash,
          caps: { chat: true, tools: false, ctx: 32768 },
        },
        sender: ctx.admin.publicKey,
        txNo: 5,
      },
      {
        type: 'publishCatalog',
        value: {
          op: 'publish_catalog',
          catalog_id: 'mayhem-models',
          source_kind: 'huggingface',
          catalog_url: 'https://huggingface.co/TracNetwork/mayhem-catalog/resolve/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/models.json',
          signature_url: 'https://huggingface.co/TracNetwork/mayhem-catalog/resolve/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/models.json.sig',
          catalog_hash: '8'.repeat(64),
          signature_hash: '9'.repeat(64),
          key_id: 'mayhem-catalog-tracnetwork-v1',
          public_key: 'a'.repeat(64),
          model_count: 1,
          artifact_count: 1,
          canaries: [{
            set_id: 'canary-dev-v1',
            url: 'https://huggingface.co/TracNetwork/mayhem-catalog/resolve/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/canaries/canary-dev-v1.json',
            hash: 'b'.repeat(64),
          }],
        },
        sender: ctx.admin.publicKey,
        txNo: 6,
      },
      {
        type: 'auditorRegister',
        value: {
          op: 'auditor_register',
          auditor: ctx.auditor.publicKey,
          registered_at_seconds: 0,
        },
        sender: ctx.admin.publicKey,
        txNo: 7,
      },
      {
        type: 'probeResult',
        value: canaryProbe(ctx.provider, ctx.auditor, {
          probe_id: 'canary-bad',
          match_bps: 1_000,
          pass: false,
        }),
        sender: ctx.auditor.publicKey,
        txNo: 8,
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

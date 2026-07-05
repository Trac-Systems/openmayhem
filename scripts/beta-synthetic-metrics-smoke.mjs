#!/usr/bin/env node
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { blake3 } from '../intercom/node_modules/@tracsystems/blake3/dist/wasm/blake3.mjs';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const defaultOutDir = '.mayhem-local/p8.5-synthetic';
const testtracAddress = 'testtrac1n57xm5deqnmzrwmzymvandzvkekkshjpaygcvpdkuwzqr4862rus5lyrjg';
const modelId = 'meta/llama-3.1-8b-instruct@4bit';
const backend = 'llama.cpp';
const artifactRoot = hex('artifact:root');
const artifactRootKind = 'blake3_merkle_v1';
const artifactSource = {
  kind: 'huggingface',
  repo: 'TracNetwork/mayhem-catalog-llama-3.1-8b-instruct-GGUF',
  revision: '0e9e39f249a16976918f6564b8830bc894c89659',
  path: 'llama-3.1-8b-instruct-Q4_K_M.gguf',
};
const sourceSha256 = hex('artifact:source-sha256');
const msbBootstrap = 'c184f4ad8e9cf5e911f9415b60e7dcfb30aed73ebd8a402ef68e1b154624f5ef';
const msbChannel = '1111trac1network1msb1testnet1111';
const dhtBootstrap = [
  '116.202.214.149:10001',
  '157.180.12.214:10001',
  'node1.hyperdht.org:49737',
];
const syntheticWindowStart = '2026-07-04T00:00:00Z';
const syntheticWindowEnd = '2026-07-11T00:00:00Z';
const ed25519Pkcs8SeedPrefix = Buffer.from('302e020100300506032b657004220420', 'hex');
const ed25519SpkiPrefix = Buffer.from('302a300506032b6570032100', 'hex');

function usage() {
  console.log(`Usage: node scripts/beta-synthetic-metrics-smoke.mjs [--out-dir PATH] [--tracker-recorded] [--json]

Builds a deterministic P8.5 synthetic beta metrics rehearsal under an ignored
local directory, runs the canonical-service audit, collects metrics, and runs
the strict metrics validator. Use --tracker-recorded only after the validated
result is being recorded in docs/TRACKER.md.`);
}

function parseArgs(argv) {
  const args = {
    outDir: defaultOutDir,
    trackerRecorded: false,
    json: false,
  };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--out-dir') {
      i += 1;
      if (!argv[i]) throw new Error('--out-dir requires a path');
      args.outDir = argv[i];
    } else if (arg === '--tracker-recorded') {
      args.trackerRecorded = true;
    } else if (arg === '--json') {
      args.json = true;
    } else if (arg === '-h' || arg === '--help') {
      usage();
      process.exit(0);
    } else {
      throw new Error(`unknown argument: ${arg}`);
    }
  }
  return args;
}

function resolveOut(filePath) {
  return path.isAbsolute(filePath) ? filePath : path.resolve(repoRoot, filePath);
}

function relativeFile(filePath) {
  const rel = path.relative(repoRoot, filePath);
  return rel.startsWith('..') ? filePath : rel;
}

function stableJson(value) {
  if (Array.isArray(value)) return `[${value.map(stableJson).join(',')}]`;
  if (value && typeof value === 'object') {
    return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${stableJson(value[key])}`).join(',')}}`;
  }
  return JSON.stringify(value);
}

function hex(label) {
  return crypto.createHash('sha256').update(`mayhem-p8.5:${label}`).digest('hex');
}

function deterministicEd25519Key(label) {
  const seed = crypto.createHash('sha256').update(`mayhem-p8.5:${label}`).digest();
  const privateKey = crypto.createPrivateKey({
    key: Buffer.concat([ed25519Pkcs8SeedPrefix, seed]),
    format: 'der',
    type: 'pkcs8',
  });
  const publicDer = crypto.createPublicKey(privateKey).export({ format: 'der', type: 'spki' });
  if (!Buffer.from(publicDer.subarray(0, ed25519SpkiPrefix.length)).equals(ed25519SpkiPrefix)) {
    throw new Error('unexpected Ed25519 public key DER prefix');
  }
  return {
    privateKey,
    publicKeyHex: Buffer.from(publicDer.subarray(ed25519SpkiPrefix.length)).toString('hex'),
  };
}

async function blake3Hex(text) {
  const digest = await blake3(Buffer.from(text));
  return Buffer.from(digest).toString('hex');
}

async function blake3HexBytes(bytes) {
  const digest = await blake3(bytes);
  return Buffer.from(digest).toString('hex');
}

async function deriveCatalogEnclaveId(adminPubkey, enclave) {
  return blake3Hex(`${adminPubkey}${enclave.model_id}${enclave.artifact_root}${enclave.manifest_hash}${enclave.binary_hash}`);
}

async function deriveRoomId(enclaveId, adminPubkey, nonce) {
  return (await blake3Hex(`${enclaveId}${adminPubkey}${nonce}`)).slice(0, 32);
}

function enclaveDistributionSigningPayload(adminPubkey, enclave) {
  return Buffer.from(stableJson({
    schema_version: 1,
    kind: 'mayhem-enclave-distribution-v1',
    admin_pubkey: adminPubkey,
    enclave_id: enclave.enclave_id,
    model_id: enclave.model_id,
    backend: enclave.backend,
    artifact_root: enclave.artifact_root,
    artifact_root_kind: enclave.artifact_root_kind,
    artifact_source: enclave.artifact_source,
    source_sha256: enclave.source_sha256,
    manifest_hash: enclave.manifest_hash,
    binary_hash: enclave.binary_hash,
    distribution: {
      bundle_url: enclave.distribution?.bundle_url,
      manifest_url: enclave.distribution?.manifest_url,
      bundle_sha256: enclave.distribution?.bundle_sha256,
      bundle_bytes: enclave.distribution?.bundle_bytes,
      mirrors: enclave.distribution?.mirrors || [],
    },
  }));
}

function signEnclaveDistribution(adminKey, enclave) {
  return crypto.sign(null, enclaveDistributionSigningPayload(adminKey.publicKeyHex, enclave), adminKey.privateKey).toString('hex');
}

function writeJson(filePath, value) {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.writeFileSync(filePath, `${JSON.stringify(value, null, 2)}\n`);
  return filePath;
}

function writeText(filePath, value) {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.writeFileSync(filePath, value);
  return filePath;
}

function fileEvidence(filePath, tags = []) {
  const bytes = fs.readFileSync(filePath);
  const sha256 = crypto.createHash('sha256').update(bytes).digest('hex');
  const suffix = tags.length > 0 ? `#${tags.join('#')}` : '';
  return `file:${relativeFile(filePath)}#sha256:${sha256}${suffix}`;
}

async function buildSyntheticCatalog(outDir) {
  const catalog = JSON.parse(fs.readFileSync(path.join(repoRoot, 'catalog/models.json'), 'utf8'));
  const model = catalog.models.find((entry) => entry.model_id === modelId);
  if (!model) throw new Error(`catalog model not found: ${modelId}`);
  const artifactEntry = Object.entries(model.artifacts || {})
    .find(([, artifact]) => artifact.engine === backend);
  if (!artifactEntry) throw new Error(`catalog model ${modelId} has no ${backend} artifact`);
  const [, artifact] = artifactEntry;
  artifact.artifact_root = artifactRoot;
  artifact.artifact_root_kind = artifactRootKind;
  artifact.source = { ...artifactSource };
  artifact.path = artifactSource.path;
  artifact.source_sha256 = sourceSha256;

  const catalogDir = path.join(outDir, 'catalog');
  const keyDir = path.join(catalogDir, 'keys');
  const sigDir = path.join(catalogDir, 'signatures');
  fs.mkdirSync(keyDir, { recursive: true });
  fs.mkdirSync(sigDir, { recursive: true });
  const catalogPath = path.join(catalogDir, 'models.json');
  fs.writeFileSync(catalogPath, `${JSON.stringify(catalog, null, 2)}\n`);

  const catalogKey = deterministicEd25519Key('synthetic-catalog');
  const keyId = 'mayhem-synthetic-catalog-v1';
  writeJson(path.join(keyDir, `${keyId}.json`), {
    key_id: keyId,
    alg: 'ed25519',
    public_key: catalogKey.publicKeyHex,
    status: 'active',
    created_at: syntheticWindowStart,
  });
  const catalogBytes = fs.readFileSync(catalogPath);
  const signature = crypto.sign(null, catalogBytes, catalogKey.privateKey).toString('hex');
  const signaturePath = path.join(sigDir, 'models.json.sig');
  writeJson(signaturePath, {
    schema_version: 1,
    alg: 'ed25519',
    signed_path: relativeFile(catalogPath).replaceAll(path.sep, '/'),
    key_id: keyId,
    public_key: catalogKey.publicKeyHex,
    blake3: await blake3HexBytes(catalogBytes),
    sig: signature,
  });
  return { catalogPath, signaturePath, keyDir };
}

function run(command, args, { json = false } = {}) {
  const child = spawnSync(command, args, {
    cwd: repoRoot,
    encoding: 'utf8',
  });
  if (child.status !== 0) {
    const details = `${child.stdout || ''}${child.stderr || ''}`.trim();
    throw new Error(`${[command, ...args].join(' ')} failed${details ? `:\n${details}` : ''}`);
  }
  return json ? JSON.parse(child.stdout) : child.stdout;
}

function normalizeCanonicalAudit(filePath) {
  const audit = JSON.parse(fs.readFileSync(filePath, 'utf8'));
  audit.generated_at = syntheticWindowStart;
  writeJson(filePath, audit);
}

function providerPubkey(index) {
  return hex(`provider:${String(index).padStart(2, '0')}`);
}

function userPubkey(index) {
  return hex(`user:${String(index).padStart(3, '0')}`);
}

function buildParticipants(outDir, providerIds) {
  const providerRows = providerIds.map((provider, index) => ({
    provider,
    role: 'provider',
    joined_enclave: true,
    synthetic: true,
    ordinal: index,
  }));
  const userRows = Array.from({ length: 100 }, (_, index) => ({
    user: userPubkey(index),
    sessions: 1 + (index % 3),
    mu_usd_spend: 100 + index,
    synthetic: true,
  }));
  return {
    providers: writeJson(path.join(outDir, 'provider-roster.json'), { providers: providerRows }),
    users: writeJson(path.join(outDir, 'user-activity.json'), { users: userRows }),
  };
}

function buildPaymentRailEvidence(outDir) {
  const paygateHealth = writeJson(path.join(outDir, 'paygate-health.json'), {
    ok: true,
    paygate_admin_controls_verified: true,
    fiat_enabled: true,
    tap_enabled: true,
    coinbase_enabled: false,
    stripe_processor_enabled: true,
    ledger_denom: 'mu_usd',
  });
  const fiatCredit = writeJson(path.join(outDir, 'fiat-credit-proof.json'), {
    rail: 'fiat',
    processor: 'stripe',
    ledger_denom: 'mu_usd',
    credits_mu_usd: true,
    evidence_kind: 'synthetic-p8.5-credit',
  });
  const tapCredit = writeJson(path.join(outDir, 'tap-credit-proof.json'), {
    rail: 'tap',
    ledger_denom: 'mu_usd',
    credits_mu_usd: true,
    evidence_kind: 'synthetic-p8.5-credit',
  });
  const tnkCredit = writeJson(path.join(outDir, 'tnk-credit-proof.json'), {
    rail: 'tnk',
    ledger_denom: 'mu_usd',
    credits_mu_usd: true,
    evidence_kind: 'synthetic-p8.5-credit',
  });
  return writeJson(path.join(outDir, 'payment-rails-report.json'), {
    payment_rails: {
      ledger_denom: 'mu_usd',
      fiat_enabled: true,
      tap_enabled: true,
      tnk_enabled: true,
      stripe_processor_enabled: true,
      coinbase_enabled: false,
      rails_credit_mu_usd: true,
      paygate_admin_controls_verified: true,
      evidence: [
        fileEvidence(paygateHealth, ['paygate_admin_controls']),
        fileEvidence(fiatCredit, ['rail:fiat', 'processor:stripe', 'credits_mu_usd']),
        fileEvidence(tapCredit, ['rail:tap', 'credits_mu_usd']),
        fileEvidence(tnkCredit, ['rail:tnk', 'credits_mu_usd']),
      ],
    },
  });
}

function buildEpochEvidence(outDir) {
  return writeJson(path.join(outDir, 'epoch-roots.json'), {
    epoch: 85,
    roots: {
      dep: hex('epoch:dep'),
      use: hex('epoch:use'),
      earn: hex('epoch:earn'),
      fee: hex('epoch:fee'),
      pay: hex('epoch:pay'),
    },
    params: {
      fee_bps: 1500,
    },
    checks: [
      { name: 'receipt_roots_recomputed', ok: true },
      { name: 'payout_evidence_recomputed', ok: true },
      { name: 'fee_split_matches_admin_params', ok: true },
    ],
    payout_evidence_verified: true,
  });
}

function buildGuardianEvidence(outDir) {
  return writeJson(path.join(outDir, 'guardian-report.json'), {
    trips: 0,
    conservation_ok: true,
    monotonic_epochs: true,
    checked_epochs: [84, 85],
  });
}

function buildCanaryEvidence(outDir) {
  return writeJson(path.join(outDir, 'canary-report.json'), {
    set_id: 'canary-launch-v1',
    probes: 3,
    failures: 0,
    probe_records: [
      { probe_id: 'synthetic-canary-0', pass: true },
      { probe_id: 'synthetic-canary-1', pass: true },
      { probe_id: 'synthetic-canary-2', pass: true },
    ],
  });
}

function buildBrowserHandoffs(outDir) {
  return writeText(path.join(outDir, 'checkout-handoffs.log'), [
    'Mayhem stripe checkout',
    'Copy/paste checkout URL: https://checkout.stripe.com/c/pay/cs_test_p85_synthetic',
    '',
  ].join('\n'));
}

async function buildLaunchManifest(outDir, adminKey, providerIds, syntheticCatalog) {
  const adminPubkey = adminKey.publicKeyHex;
  const manifestHash = hex('enclave:manifest');
  const binaryHash = hex('enclave:binary');
  const enclave = {
    enclave_id: '',
    model_id: modelId,
    backend,
    artifact_root: artifactRoot,
    artifact_root_kind: artifactRootKind,
    artifact_source: { ...artifactSource },
    source_sha256: sourceSha256,
    manifest_hash: manifestHash,
    binary_hash: binaryHash,
    distribution: null,
  };
  enclave.enclave_id = await deriveCatalogEnclaveId(adminPubkey, enclave);
  enclave.distribution = {
    bundle_url: `https://downloads.trac.network/mayhem/testnet/enclaves/${enclave.enclave_id}.tar.zst`,
    manifest_url: `https://downloads.trac.network/mayhem/testnet/enclaves/${enclave.enclave_id}.json`,
    bundle_sha256: hex('distribution:bundle'),
    bundle_bytes: 1024,
    admin_signature: null,
  };
  enclave.distribution.admin_signature = signEnclaveDistribution(adminKey, enclave);
  const roomNonce = 'p8.5-synthetic-launch-us-east';
  const roomId = await deriveRoomId(enclave.enclave_id, adminPubkey, roomNonce);

  const evidenceDir = path.join(outDir, 'launch-evidence');
  const bootstrapEvidence = writeJson(path.join(evidenceDir, 'bootstrap-health.json'), {
    ok: true,
    network: 'testnet1',
    msb: {
      address_prefix: 'testtrac',
      network_id: 919,
      bootstrap: msbBootstrap,
      channel: msbChannel,
    },
    peer_dht_bootstrap: dhtBootstrap,
    msb_dht_bootstrap: dhtBootstrap,
  });
  const epochWalletEvidence = writeJson(path.join(evidenceDir, 'epoch-wallet-funding.json'), {
    network: 'testnet1',
    msb: {
      address_prefix: 'testtrac',
      network_id: 919,
      bootstrap: msbBootstrap,
      channel: msbChannel,
    },
    address: testtracAddress,
    funded: true,
    balance_tnk: '4',
    balance_tnk_e18: '4000000000000000000',
  });
  const seedProviderEvidence = writeJson(path.join(evidenceDir, 'seed-provider-opt-ins.json'), {
    free_feature_lifecycle_records: true,
    opt_ins: providerIds.map((provider) => ({
      provider_pubkey: provider,
      enclave_id: enclave.enclave_id,
      rooms: ['launch-us-east'],
      room_ids: [roomId],
    })),
  });
  const downloadEvidence = writeJson(path.join(evidenceDir, 'enclave-downloads.json'), {
    distributions: [
      {
        enclave_id: enclave.enclave_id,
        admin_signed: true,
        ...enclave.distribution,
      },
    ],
  });

  const manifest = {
    schema_version: 1,
    launch_id: 'mayhem-testnet-beta-v1',
    network: {
      name: 'testnet1',
      denom: 'mu_usd',
      msb: {
        address_prefix: 'testtrac',
        network_id: 919,
        bootstrap: msbBootstrap,
        channel: msbChannel,
      },
      subnet: {
        channel: 'mayhem-testnet-beta-v1',
        bootstrap: hex('subnet-bootstrap'),
      },
      dht: {
        peer_bootstrap: [
          ...dhtBootstrap,
        ],
        msb_bootstrap: [
          ...dhtBootstrap,
        ],
      },
    },
    controls: {
      admin_controls_economy: true,
      admin_sets_prices: true,
      admin_sets_rules: true,
      admin_sets_params: true,
      admin_sets_provider_payout_targets: true,
      admin_can_ban_providers: true,
      providers_set_prices: false,
      providers_set_rules: false,
      providers_set_params: false,
      providers_set_payout_terms: false,
      providers_submit_models: false,
      providers_create_canonical_rooms: false,
      providers_only_join_admin_rooms: true,
      provider_payout_targets_admin_verified: true,
      browser_handoffs_print_copy_paste_url: true,
    },
    admin: {
      peer_pubkey: adminPubkey,
      store_name: 'mayhem-beta-admin',
      rpc_url: 'http://127.0.0.1:49223/v1',
      sc_bridge_url: 'ws://127.0.0.1:49222',
      sc_bridge_token_env: 'MAYHEM_BETA_SC_TOKEN',
    },
    catalog: {
      path: relativeFile(syntheticCatalog.catalogPath),
    },
    paygate: {
      public_base_url: 'https://paygate.trac.network',
      health_path: '/v1/health',
      tnk_treasury_address: testtracAddress,
      stripe_enabled: true,
      coinbase_enabled: false,
      checkout_urls: {
        stripe: {
          success_url: 'https://paygate.trac.network/v1/stripe/return?session_id={CHECKOUT_SESSION_ID}',
          cancel_url: 'https://paygate.trac.network/v1/stripe/cancel',
        },
      },
    },
    epoch_wallet: {
      address: testtracAddress,
      min_balance_tnk_e18: '1000000000000000000',
      funded: true,
      pays_for: [
        'epoch_commit',
        'payment_proof_rollup',
        'payout_fee_sweep',
      ],
    },
    canary: {
      set_id: 'canary-launch-v1',
      path: 'catalog/canaries/canary-launch-v1.json',
    },
    evidence: {
      bootstrap_nodes: [fileEvidence(bootstrapEvidence)],
      epoch_wallet_funding: [fileEvidence(epochWalletEvidence)],
      seed_provider_opt_ins: [fileEvidence(seedProviderEvidence)],
      enclave_downloads: [fileEvidence(downloadEvidence)],
      canary_set: [fileEvidence(path.join(repoRoot, 'catalog/canaries/canary-launch-v1.json'))],
    },
    canonical_enclaves: [
      {
        enclave_id: enclave.enclave_id,
        model_id: enclave.model_id,
        backend: enclave.backend,
        artifact_root: enclave.artifact_root,
        artifact_root_kind: enclave.artifact_root_kind,
        artifact_source: enclave.artifact_source,
        source_sha256: enclave.source_sha256,
        manifest_hash: enclave.manifest_hash,
        binary_hash: enclave.binary_hash,
        att_tier: 1,
        caps: {
          chat: true,
          tools: true,
          json: true,
          ctx: 131072,
        },
        distribution: enclave.distribution,
        model_ref_mu: {
          in_per_1k: 18,
          out_per_1k: 55,
        },
        price_mu: {
          denom: 'mu_usd',
          in_per_1k: 18,
          out_per_1k: 55,
          per_req: 0,
          min_session: 100,
          effective_at: 21600,
        },
        rooms: [
          {
            label: 'launch-us-east',
            nonce: roomNonce,
            admin_created: true,
            policy: {
              region_hint: 'us-east',
              canary_set: 'canary-launch-v1',
            },
          },
        ],
      },
    ],
    seed_providers: providerIds.map((provider) => ({
      provider_pubkey: provider,
      payout: {
        admin_approved: true,
        method: 'tnk',
        addr: testtracAddress,
      },
      joins: [
        {
          enclave_id: enclave.enclave_id,
          rooms: ['launch-us-east'],
        },
      ],
    })),
  };

  const manifestPath = writeJson(path.join(outDir, 'testnet.json'), manifest);
  return { manifestPath, enclave, roomId, roomNonce };
}

function buildCanonicalSnapshot(outDir, adminPubkey, providerIds, enclave, roomId) {
  const roomServeIndex = providerIds.map((provider) => ({
    provider,
    enclave_id: enclave.enclave_id,
  }));
  const snapshot = {
    admin: adminPubkey,
    'rules/current': {
      ver: 1,
      hash: hex('rules:v1'),
      set_by: adminPubkey,
      set_by_role: 'admin',
    },
    'params/current': {
      fee_bps: 1500,
      set_by: adminPubkey,
      set_by_role: 'admin',
    },
    providers: providerIds.map((provider) => ({
      provider,
      status: 'active',
      registered_by: provider,
      enclaves: [enclave.enclave_id],
      payout: {
        method: 'tnk',
        addr: testtracAddress,
        set_by: adminPubkey,
        set_by_role: 'admin',
      },
    })),
    enclaves: [
      {
        enclave_id: enclave.enclave_id,
        model_id: enclave.model_id,
        backend: enclave.backend,
        artifact_root: enclave.artifact_root,
        artifact_root_kind: enclave.artifact_root_kind,
        artifact_source: enclave.artifact_source,
        source_sha256: enclave.source_sha256,
        manifest_hash: enclave.manifest_hash,
        binary_hash: enclave.binary_hash,
        att_tier: 1,
        status: 'active',
        created_by: adminPubkey,
        created_by_role: 'admin',
        providers: providerIds,
      },
    ],
    rooms: [
      {
        room_id: roomId,
        model_id: enclave.model_id,
        enclave_id: enclave.enclave_id,
        status: 'open',
        creator: adminPubkey,
        creator_role: 'admin',
        sidechannel: `mx/room/${roomId}`,
        serves: roomServeIndex,
      },
    ],
    serves: providerIds.map((provider) => ({
      provider,
      enclave_id: enclave.enclave_id,
      model_id: enclave.model_id,
      status: 'active',
      rooms: [roomId],
    })),
    roomserve: providerIds.map((provider) => ({
      room_id: roomId,
      provider,
      enclave_id: enclave.enclave_id,
      model_id: enclave.model_id,
      status: 'active',
      sidechannel: `mx/room/${roomId}`,
    })),
    prices: [
      {
        enclave_id: enclave.enclave_id,
        model_id: enclave.model_id,
        denom: 'mu_usd',
        current: {
          enclave_id: enclave.enclave_id,
          model_id: enclave.model_id,
          denom: 'mu_usd',
          in_per_1k_mu: 18,
          out_per_1k_mu: 55,
          per_req_mu: 0,
          min_session_mu: 100,
          effective_at: 21600,
          ver: 1,
          set_by: adminPubkey,
          set_by_role: 'admin',
        },
      },
    ],
  };
  return writeJson(path.join(outDir, 'contract-state.json'), snapshot);
}

async function buildSyntheticBundle(args) {
  const outDir = resolveOut(args.outDir);
  fs.mkdirSync(outDir, { recursive: true });

  const adminKey = deterministicEd25519Key('synthetic-admin');
  const adminPubkey = adminKey.publicKeyHex;
  const providerIds = Array.from({ length: 20 }, (_, index) => providerPubkey(index));
  const syntheticCatalog = await buildSyntheticCatalog(outDir);
  const { manifestPath, enclave, roomId } = await buildLaunchManifest(
    outDir,
    adminKey,
    providerIds,
    syntheticCatalog
  );
  const snapshotPath = buildCanonicalSnapshot(outDir, adminPubkey, providerIds, enclave, roomId);
  const participantPaths = buildParticipants(outDir, providerIds);
  const paymentRailsPath = buildPaymentRailEvidence(outDir);
  const epochPath = buildEpochEvidence(outDir);
  const guardianPath = buildGuardianEvidence(outDir);
  const canaryPath = buildCanaryEvidence(outDir);
  const browserHandoffsPath = buildBrowserHandoffs(outDir);
  const canonicalAuditPath = path.join(outDir, 'canonical-service-audit.json');
  const metricsPath = path.join(outDir, 'metrics.json');

  run(process.execPath, [
    'scripts/beta-canonical-service-audit.mjs',
    '--snapshot',
    relativeFile(snapshotPath),
    '--catalog',
    relativeFile(syntheticCatalog.catalogPath),
    '--catalog-signature',
    relativeFile(syntheticCatalog.signaturePath),
    '--catalog-key-dir',
    relativeFile(syntheticCatalog.keyDir),
    '--out',
    relativeFile(canonicalAuditPath),
  ]);
  normalizeCanonicalAudit(canonicalAuditPath);

  const collectArgs = [
    'scripts/beta-metrics-collect.mjs',
    '--window-start',
    syntheticWindowStart,
    '--window-end',
    syntheticWindowEnd,
    '--launch-manifest',
    relativeFile(manifestPath),
    '--providers',
    relativeFile(participantPaths.providers),
    '--users',
    relativeFile(participantPaths.users),
    '--epoch',
    relativeFile(epochPath),
    '--guardian',
    relativeFile(guardianPath),
    '--canary',
    relativeFile(canaryPath),
    '--browser-handoffs',
    relativeFile(browserHandoffsPath),
    '--canonical-service',
    relativeFile(canonicalAuditPath),
    '--payment-rails',
    relativeFile(paymentRailsPath),
    '--commit-tx',
    hex('epoch:commit-tx'),
    '--apply-tx',
    hex('epoch:apply-tx'),
    '--auditor',
    hex('auditor:0'),
    '--out',
    relativeFile(metricsPath),
  ];
  if (args.trackerRecorded) collectArgs.push('--tracker-recorded');
  const collector = run(process.execPath, collectArgs, { json: false });
  const report = run(process.execPath, [
    'scripts/beta-metrics.mjs',
    '--metrics',
    relativeFile(metricsPath),
    '--json',
  ], { json: true });

  return {
    ok: report.ok === true,
    out_dir: outDir,
    launch_manifest: manifestPath,
    contract_snapshot: snapshotPath,
    canonical_service_audit: canonicalAuditPath,
    metrics: metricsPath,
    providers: 20,
    users: 100,
    audited_epoch: 85,
    guardian_trips: 0,
    metrics_digest: crypto.createHash('sha256').update(fs.readFileSync(metricsPath)).digest('hex'),
    tracker_snippet: report.tracker_snippet,
    collector,
  };
}

async function main() {
  try {
    const args = parseArgs(process.argv.slice(2));
    const report = await buildSyntheticBundle(args);
    if (args.json) {
      console.log(JSON.stringify(report, null, 2));
    } else {
      console.log(`Mayhem P8.5 synthetic metrics rehearsal: ${report.ok ? 'ok' : 'not ready'}`);
      console.log(`Copy/paste metrics path: ${relativeFile(report.metrics)}`);
      console.log(`Provider records: ${report.providers}`);
      console.log(`Users: ${report.users}`);
      console.log(`Audited epoch: ${report.audited_epoch}`);
      console.log(`Guardian trips: ${report.guardian_trips}`);
      console.log(`Metrics SHA-256: ${report.metrics_digest}`);
      console.log('');
      console.log('Copy/paste tracker metrics note:');
      console.log(report.tracker_snippet);
    }
    if (!report.ok) process.exitCode = 1;
  } catch (error) {
    console.error(`error: ${error.message}`);
    process.exitCode = 1;
  }
}

await main();

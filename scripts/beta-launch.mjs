#!/usr/bin/env node
import fs from 'node:fs';
import crypto from 'node:crypto';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';
import { blake3 } from '../intercom/node_modules/@tracsystems/blake3/dist/wasm/blake3.mjs';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const defaultManifest = 'config/beta/testnet.template.json';
const hex64 = /^[0-9a-fA-F]{64}$/;
const pubkey64 = /^[0-9a-fA-F]{64}$/;
const testtracAddress = /^testtrac1[0-9a-z]+$/;
const safeCommandText = /^[a-zA-Z0-9._:@/+~,\-\s<>$:"{}[\]]+$/;
const sha256Evidence = /#sha256:[0-9a-fA-F]{64}(?:$|[#?&])/;
const httpUrl = /^https?:\/\//;

function usage() {
  console.log(`Usage: node scripts/beta-launch.mjs [--manifest PATH] [--allow-placeholders] [--json] [--no-commands]

Validates a Mayhem testnet beta launch manifest and prints copy/paste launch
commands. Strict mode fails on template placeholders; use --allow-placeholders
only to validate the committed template shape.`);
}

function parseArgs(argv) {
  const args = {
    manifest: defaultManifest,
    allowPlaceholders: false,
    json: false,
    commands: true,
  };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--manifest') {
      i += 1;
      if (!argv[i]) throw new Error('--manifest requires a path');
      args.manifest = argv[i];
    } else if (arg === '--allow-placeholders') {
      args.allowPlaceholders = true;
    } else if (arg === '--json') {
      args.json = true;
    } else if (arg === '--no-commands') {
      args.commands = false;
    } else if (arg === '-h' || arg === '--help') {
      usage();
      process.exit(0);
    } else {
      throw new Error(`unknown argument: ${arg}`);
    }
  }
  return args;
}

function readJson(filePath) {
  return JSON.parse(fs.readFileSync(filePath, 'utf8'));
}

function isPlaceholder(value) {
  return typeof value === 'string' && (
    value.includes('<') ||
    value.includes('>') ||
    /\b(TBD|TODO|REPLACE|PLACEHOLDER|CHANGE_ME)\b/i.test(value)
  );
}

function issueFactory({ allowPlaceholders }) {
  const errors = [];
  const warnings = [];
  const add = (kind, message) => {
    if (kind === 'placeholder' && allowPlaceholders) warnings.push(message);
    else errors.push(message);
  };
  return { errors, warnings, add };
}

function requireObject(add, value, name) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    add('error', `${name} must be an object`);
    return false;
  }
  return true;
}

function requireArray(add, value, name, min = 0) {
  if (!Array.isArray(value)) {
    add('error', `${name} must be an array`);
    return false;
  }
  if (value.length < min) add('error', `${name} must contain at least ${min} item(s)`);
  return true;
}

function requireLiteral(add, value, expected, name) {
  if (value !== expected) add('error', `${name} must be ${JSON.stringify(expected)}`);
}

function requireString(add, value, name, regex = null) {
  if (typeof value !== 'string' || value.length === 0) {
    add('error', `${name} must be a non-empty string`);
    return;
  }
  if (isPlaceholder(value)) add('placeholder', `${name} still contains a template placeholder`);
  if (regex && !isPlaceholder(value) && !regex.test(value)) {
    add('error', `${name} has invalid format`);
  }
}

function requirePositiveInteger(add, value, name) {
  if (!Number.isInteger(value) || value <= 0) {
    add('error', `${name} must be a positive integer`);
  }
}

function requireDecimalString(add, value, name) {
  if (typeof value !== 'string' || !/^[0-9]+$/.test(value) || value === '0') {
    add('error', `${name} must be a positive decimal string`);
  }
}

function requireRailCheckoutUrl(add, value, name, rail, expectedPathSegment) {
  requireString(add, value, name, httpUrl);
  if (typeof value !== 'string' || isPlaceholder(value)) return;
  let parsed;
  try {
    parsed = new URL(value);
  } catch {
    add('error', `${name} must be a valid URL`);
    return;
  }
  if (!parsed.pathname.includes(`/${rail}/${expectedPathSegment}`)) {
    add('error', `${name} must route to a ${rail} ${expectedPathSegment} endpoint`);
  }
}

function validateCheckoutUrls(add, paygate) {
  if (!requireObject(add, paygate.checkout_urls, 'paygate.checkout_urls')) return;
  for (const rail of ['stripe', 'coinbase']) {
    const railConfig = paygate.checkout_urls[rail];
    const prefix = `paygate.checkout_urls.${rail}`;
    if (!requireObject(add, railConfig, prefix)) continue;
    requireRailCheckoutUrl(add, railConfig.success_url, `${prefix}.success_url`, rail, 'return');
    requireRailCheckoutUrl(add, railConfig.cancel_url, `${prefix}.cancel_url`, rail, 'cancel');
  }
}

function validateEvidenceArray(add, value, name) {
  if (!requireArray(add, value, name, 1)) return;
  const seen = new Set();
  for (const [index, item] of value.entries()) {
    const itemName = `${name}[${index}]`;
    requireString(add, item, itemName);
    if (typeof item !== 'string' || isPlaceholder(item)) continue;
    if (!sha256Evidence.test(item)) {
      add('error', `${itemName} must include #sha256:<64-hex> durable evidence`);
    }
    if (seen.has(item)) add('error', `${itemName} duplicates another evidence string`);
    seen.add(item);
  }
}

function sha256File(filePath) {
  return crypto.createHash('sha256').update(fs.readFileSync(filePath)).digest('hex');
}

function relativeFile(filePath) {
  const rel = path.relative(repoRoot, filePath);
  return rel.startsWith('..') ? filePath : rel;
}

function requiredFileEvidence(filePath) {
  return `file:${relativeFile(filePath)}#sha256:${sha256File(filePath)}`;
}

function validateCanaryEvidence(add, evidence, canaryPath) {
  validateEvidenceArray(add, evidence, 'evidence.canary_set');
  if (!Array.isArray(evidence) || evidence.some(isPlaceholder)) return;
  const expected = requiredFileEvidence(canaryPath);
  if (!evidence.includes(expected)) {
    add('error', `evidence.canary_set must include ${expected}`);
  }
}

function validateLaunchManifest(manifest, { manifestPath, allowPlaceholders }) {
  const { errors, warnings, add } = issueFactory({ allowPlaceholders });
  const catalogPath = path.join(repoRoot, 'catalog/models.json');
  const catalog = readJson(catalogPath);
  const catalogModels = new Map((catalog.models || []).map((model) => [model.model_id, model]));

  requireLiteral(add, manifest.schema_version, 1, 'schema_version');
  requireString(add, manifest.launch_id, 'launch_id');

  if (requireObject(add, manifest.network, 'network')) {
    requireLiteral(add, manifest.network.name, 'testnet1', 'network.name');
    requireLiteral(add, manifest.network.denom, 'mu_usd', 'network.denom');
    if (requireObject(add, manifest.network.msb, 'network.msb')) {
      requireLiteral(add, manifest.network.msb.address_prefix, 'testtrac', 'network.msb.address_prefix');
      requireLiteral(add, manifest.network.msb.network_id, 919, 'network.msb.network_id');
      requireString(add, manifest.network.msb.bootstrap, 'network.msb.bootstrap', hex64);
      requireString(add, manifest.network.msb.channel, 'network.msb.channel');
    }
    if (requireObject(add, manifest.network.subnet, 'network.subnet')) {
      requireString(add, manifest.network.subnet.channel, 'network.subnet.channel');
      if (manifest.network.subnet.bootstrap !== null && manifest.network.subnet.bootstrap !== undefined) {
        requireString(add, manifest.network.subnet.bootstrap, 'network.subnet.bootstrap', hex64);
        if (
          !isPlaceholder(manifest.network.subnet.bootstrap) &&
          manifest.network.subnet.bootstrap === manifest.network.msb?.bootstrap
        ) {
          add('error', 'network.subnet.bootstrap must not equal network.msb.bootstrap');
        }
      }
    }
  }

  if (requireObject(add, manifest.controls, 'controls')) {
    requireLiteral(add, manifest.controls.admin_controls_economy, true, 'controls.admin_controls_economy');
    requireLiteral(add, manifest.controls.providers_set_prices, false, 'controls.providers_set_prices');
    requireLiteral(add, manifest.controls.providers_set_payout_terms, false, 'controls.providers_set_payout_terms');
    requireLiteral(add, manifest.controls.providers_submit_models, false, 'controls.providers_submit_models');
    requireLiteral(add, manifest.controls.providers_create_canonical_rooms, false, 'controls.providers_create_canonical_rooms');
    requireLiteral(add, manifest.controls.providers_only_join_admin_rooms, true, 'controls.providers_only_join_admin_rooms');
    requireLiteral(add, manifest.controls.provider_payout_targets_admin_verified, true, 'controls.provider_payout_targets_admin_verified');
    requireLiteral(add, manifest.controls.browser_handoffs_print_copy_paste_url, true, 'controls.browser_handoffs_print_copy_paste_url');
  }

  if (requireObject(add, manifest.admin, 'admin')) {
    requireString(add, manifest.admin.peer_pubkey, 'admin.peer_pubkey', pubkey64);
    requireString(add, manifest.admin.store_name, 'admin.store_name');
    requireString(add, manifest.admin.rpc_url, 'admin.rpc_url');
    requireString(add, manifest.admin.sc_bridge_url, 'admin.sc_bridge_url');
  }

  if (requireObject(add, manifest.paygate, 'paygate')) {
    requireString(add, manifest.paygate.public_base_url, 'paygate.public_base_url');
    requireString(add, manifest.paygate.health_path, 'paygate.health_path');
    requireLiteral(add, manifest.paygate.stripe_enabled, true, 'paygate.stripe_enabled');
    requireLiteral(add, manifest.paygate.coinbase_enabled, true, 'paygate.coinbase_enabled');
    validateCheckoutUrls(add, manifest.paygate);
  }

  if (requireObject(add, manifest.epoch_wallet, 'epoch_wallet')) {
    requireString(add, manifest.epoch_wallet.address, 'epoch_wallet.address', testtracAddress);
    if (
      typeof manifest.epoch_wallet.address === 'string' &&
      !isPlaceholder(manifest.epoch_wallet.address) &&
      manifest.epoch_wallet.address.length !== 67
    ) {
      add('error', 'epoch_wallet.address must be a 67-character testtrac1 address');
    }
    requireDecimalString(add, manifest.epoch_wallet.min_balance_tnk_e18, 'epoch_wallet.min_balance_tnk_e18');
    if (manifest.epoch_wallet.funded !== true) {
      add(allowPlaceholders ? 'placeholder' : 'error', 'epoch_wallet.funded must be true for a real beta launch');
    }
  }

  if (requireObject(add, manifest.canary, 'canary')) {
    requireLiteral(add, manifest.canary.set_id, 'canary-launch-v1', 'canary.set_id');
    const canaryPath = path.join(repoRoot, manifest.canary.path || '');
    if (!fs.existsSync(canaryPath)) add('error', `canary.path does not exist: ${manifest.canary.path}`);
  }

  const canaryPath = path.join(repoRoot, manifest.canary?.path || '');
  if (requireObject(add, manifest.evidence, 'evidence')) {
    validateEvidenceArray(add, manifest.evidence.bootstrap_nodes, 'evidence.bootstrap_nodes');
    validateEvidenceArray(add, manifest.evidence.epoch_wallet_funding, 'evidence.epoch_wallet_funding');
    validateEvidenceArray(add, manifest.evidence.seed_provider_opt_ins, 'evidence.seed_provider_opt_ins');
    if (fs.existsSync(canaryPath)) {
      validateCanaryEvidence(add, manifest.evidence.canary_set, canaryPath);
    } else {
      validateEvidenceArray(add, manifest.evidence.canary_set, 'evidence.canary_set');
    }
  }

  const enclaveIds = new Set();
  const roomLabels = new Map();
  if (requireArray(add, manifest.canonical_enclaves, 'canonical_enclaves', 1)) {
    for (const [index, enclave] of manifest.canonical_enclaves.entries()) {
      const prefix = `canonical_enclaves[${index}]`;
      if (!requireObject(add, enclave, prefix)) continue;
      requireString(add, enclave.enclave_id, `${prefix}.enclave_id`);
      if (typeof enclave.enclave_id === 'string') enclaveIds.add(enclave.enclave_id);
      requireString(add, enclave.model_id, `${prefix}.model_id`);
      if (enclave.model_id && !isPlaceholder(enclave.model_id) && !catalogModels.has(enclave.model_id)) {
        add('error', `${prefix}.model_id is not present in catalog/models.json`);
      }
      requireString(add, enclave.backend, `${prefix}.backend`);
      requireString(add, enclave.artifact_root, `${prefix}.artifact_root`, hex64);
      requireString(add, enclave.manifest_hash, `${prefix}.manifest_hash`, hex64);
      requireString(add, enclave.binary_hash, `${prefix}.binary_hash`, hex64);
      if (enclave.att_tier !== 1 && enclave.att_tier !== 2) {
        add('error', `${prefix}.att_tier must be 1 or 2`);
      }
      if (requireObject(add, enclave.model_ref_mu, `${prefix}.model_ref_mu`)) {
        requirePositiveInteger(add, enclave.model_ref_mu.in_per_1k, `${prefix}.model_ref_mu.in_per_1k`);
        requirePositiveInteger(add, enclave.model_ref_mu.out_per_1k, `${prefix}.model_ref_mu.out_per_1k`);
      }
      if (requireObject(add, enclave.price_mu, `${prefix}.price_mu`)) {
        if (enclave.price_mu.denom !== undefined) {
          requireLiteral(add, enclave.price_mu.denom, 'mu_usd', `${prefix}.price_mu.denom`);
        }
        requirePositiveInteger(add, enclave.price_mu.in_per_1k, `${prefix}.price_mu.in_per_1k`);
        requirePositiveInteger(add, enclave.price_mu.out_per_1k, `${prefix}.price_mu.out_per_1k`);
        if (!Number.isInteger(enclave.price_mu.per_req) || enclave.price_mu.per_req < 0) {
          add('error', `${prefix}.price_mu.per_req must be a non-negative integer`);
        }
        if (!Number.isInteger(enclave.price_mu.min_session) || enclave.price_mu.min_session < 0) {
          add('error', `${prefix}.price_mu.min_session must be a non-negative integer`);
        }
        if (!Number.isInteger(enclave.price_mu.effective_at) || enclave.price_mu.effective_at < 0) {
          add('error', `${prefix}.price_mu.effective_at must be a non-negative integer`);
        }
      }
      if (requireArray(add, enclave.rooms, `${prefix}.rooms`, 1)) {
        for (const [roomIndex, room] of enclave.rooms.entries()) {
          const roomPrefix = `${prefix}.rooms[${roomIndex}]`;
          if (!requireObject(add, room, roomPrefix)) continue;
          requireString(add, room.label, `${roomPrefix}.label`);
          requireString(add, room.nonce, `${roomPrefix}.nonce`);
          requireLiteral(add, room.admin_created, true, `${roomPrefix}.admin_created`);
          if (typeof room.label === 'string') {
            if (roomLabels.has(room.label)) {
              add('error', `${roomPrefix}.label duplicates another canonical room label`);
            }
            roomLabels.set(room.label, { enclave, room });
          }
        }
      }
    }
  }

  if (manifest.provider_created_enclaves !== undefined || manifest.provider_rooms !== undefined) {
    add('error', 'provider-created canonical enclaves/rooms are forbidden in the launch manifest');
  }

  if (requireArray(add, manifest.seed_providers, 'seed_providers', 1)) {
    for (const [index, provider] of manifest.seed_providers.entries()) {
      const prefix = `seed_providers[${index}]`;
      if (!requireObject(add, provider, prefix)) continue;
      requireString(add, provider.provider_pubkey, `${prefix}.provider_pubkey`, pubkey64);
      if (provider.submitted_models !== undefined || provider.created_rooms !== undefined || provider.created_enclaves !== undefined) {
        add('error', `${prefix} must not contain provider-created models, rooms, or enclaves`);
      }
      if (requireObject(add, provider.payout, `${prefix}.payout`)) {
        requireLiteral(add, provider.payout.admin_approved, true, `${prefix}.payout.admin_approved`);
        if (!['tnk', 'stripe', 'coinbase'].includes(provider.payout.method)) {
          add('error', `${prefix}.payout.method must be tnk, stripe, or coinbase`);
        }
        requireString(add, provider.payout.addr, `${prefix}.payout.addr`);
      }
      if (requireArray(add, provider.joins, `${prefix}.joins`, 1)) {
        for (const [joinIndex, join] of provider.joins.entries()) {
          const joinPrefix = `${prefix}.joins[${joinIndex}]`;
          if (!requireObject(add, join, joinPrefix)) continue;
          requireString(add, join.enclave_id, `${joinPrefix}.enclave_id`);
          if (!isPlaceholder(join.enclave_id) && !enclaveIds.has(join.enclave_id)) {
            add('error', `${joinPrefix}.enclave_id is not a canonical enclave`);
          }
          if (requireArray(add, join.rooms, `${joinPrefix}.rooms`, 1)) {
            for (const [roomIndex, roomRef] of join.rooms.entries()) {
              requireString(add, roomRef, `${joinPrefix}.rooms[${roomIndex}]`);
              const canonicalRoom = roomLabels.get(roomRef);
              if (!isPlaceholder(roomRef) && !canonicalRoom) {
                add('error', `${joinPrefix}.rooms[${roomIndex}] is not an admin-created canonical room label`);
              } else if (
                canonicalRoom &&
                !isPlaceholder(join.enclave_id) &&
                canonicalRoom.enclave.enclave_id !== join.enclave_id
              ) {
                add('error', `${joinPrefix}.rooms[${roomIndex}] belongs to enclave ${canonicalRoom.enclave.enclave_id}, not ${join.enclave_id}`);
              }
            }
          }
        }
      }
    }
  }

  const allText = JSON.stringify(manifest);
  if (!allowPlaceholders && isPlaceholder(allText)) {
    add('error', 'manifest still contains placeholders; use --allow-placeholders only for template checks');
  }

  return {
    ok: errors.length === 0,
    manifest_path: manifestPath,
    errors,
    warnings,
    counts: {
      canonical_enclaves: manifest.canonical_enclaves?.length ?? 0,
      canonical_rooms: roomLabels.size,
      seed_providers: manifest.seed_providers?.length ?? 0,
    },
  };
}

function sh(value) {
  const text = String(value);
  if (safeCommandText.test(text) && !text.includes("'")) return `'${text}'`;
  return `'${text.replaceAll("'", "'\\''")}'`;
}

function txCommand(value, sim = 0) {
  return `/tx --command ${sh(JSON.stringify(value))} --sim ${sim}`;
}

function commaList(values) {
  return Array.isArray(values) && values.length > 0 ? values.join(',') : '';
}

function joinUrl(base, suffix) {
  if (!base) return '';
  const rawSuffix = suffix.startsWith('/') ? suffix : `/${suffix}`;
  if (base.startsWith('<') && base.endsWith('>')) {
    const inner = base.slice(1, -1).replace(/\/+$/, '');
    return `<${inner}${rawSuffix}>`;
  }
  return `${base.replace(/\/+$/, '')}${rawSuffix}`;
}

function railCheckoutUrl(paygate, rail, field) {
  return paygate.checkout_urls?.[rail]?.[field]
    || paygate[field]
    || `<${rail}-${field.replace('_', '-')}>`;
}

async function deriveRoomId(enclaveId, creator, nonce) {
  const digest = await blake3(Buffer.from(`${enclaveId}${creator}${nonce}`));
  return Buffer.from(digest).toString('hex').slice(0, 32);
}

async function roomIdForLaunchRoom({ enclaveId, adminPubkey, room }) {
  if (
    typeof enclaveId !== 'string' ||
    typeof adminPubkey !== 'string' ||
    typeof room?.nonce !== 'string' ||
    isPlaceholder(enclaveId) ||
    isPlaceholder(adminPubkey) ||
    isPlaceholder(room.nonce)
  ) {
    return room?.room_id || `<room_id returned by open_room for ${room?.label || 'room'}>`;
  }
  return deriveRoomId(enclaveId, adminPubkey, room.nonce);
}

async function buildCommands(manifest) {
  const appDir = path.join(repoRoot, 'intercom');
  const network = manifest.network || {};
  const msb = network.msb || {};
  const subnet = network.subnet || {};
  const dht = network.dht || {};
  const admin = manifest.admin || {};
  const paygate = manifest.paygate || {};
  const scTokenEnv = admin.sc_bridge_token_env || 'MAYHEM_BETA_SC_TOKEN';
  const peerDht = commaList(dht.peer_bootstrap);
  const msbDht = commaList(dht.msb_bootstrap);
  const adminStore = admin.store_name || 'mayhem-beta-admin';
  const paygateBase = String(paygate.public_base_url || '').replace(/\/+$/, '');
  const healthPath = String(paygate.health_path || '/v1/health');
  const roomByLabel = new Map();
  for (const enclave of manifest.canonical_enclaves || []) {
    for (const room of enclave.rooms || []) {
      roomByLabel.set(room.label, {
        enclave,
        room,
        room_id: await roomIdForLaunchRoom({
          enclaveId: enclave.enclave_id,
          adminPubkey: admin.peer_pubkey,
          room,
        }),
      });
    }
  }

  const boot = [
    `cd ${sh(appDir)}`,
    `"$MAYHEM_PEAR_RUNTIME" run . --network testnet1 --peer-store-name ${sh(adminStore)} --msb-store-name ${sh(`${adminStore}-msb`)} --msb-bootstrap ${sh(msb.bootstrap)} --msb-channel ${sh(msb.channel)} --subnet-channel ${sh(subnet.channel)} --sc-bridge 1 --sc-bridge-host 127.0.0.1 --sc-bridge-port 49222 --sc-bridge-token "$${scTokenEnv}" --rpc 1 --rpc-host 127.0.0.1 --rpc-port 49223${peerDht ? ` --peer-dht-bootstrap ${sh(peerDht)}` : ''}${msbDht ? ` --msb-dht-bootstrap ${sh(msbDht)}` : ''}`,
  ];

  const adminTxs = [];
  const providerTxs = [];
  for (const enclave of manifest.canonical_enclaves || []) {
    adminTxs.push(txCommand({
      op: 'set_model_ref',
      model_id: enclave.model_id,
      price_ref_mu: enclave.model_ref_mu,
    }));
    adminTxs.push(txCommand({
      op: 'register_enclave',
      enclave_id: enclave.enclave_id,
      model_id: enclave.model_id,
      backend: enclave.backend,
      artifact_root: enclave.artifact_root,
      manifest_hash: enclave.manifest_hash,
      att_tier: enclave.att_tier,
      binary_hash: enclave.binary_hash,
      caps: enclave.caps,
    }));
    adminTxs.push(txCommand({
      op: 'set_price',
      enclave_id: enclave.enclave_id,
      in_per_1k_mu: enclave.price_mu?.in_per_1k,
      out_per_1k_mu: enclave.price_mu?.out_per_1k,
      per_req_mu: enclave.price_mu?.per_req ?? 0,
      min_session_mu: enclave.price_mu?.min_session ?? 0,
      effective_at: enclave.price_mu?.effective_at ?? 0,
    }));
    for (const room of enclave.rooms || []) {
      const derivedRoom = roomByLabel.get(room.label);
      if (derivedRoom?.room_id) adminTxs.push(`# room ${room.label} => ${derivedRoom.room_id}`);
      adminTxs.push(txCommand({
        op: 'open_room',
        enclave_id: enclave.enclave_id,
        model_id: enclave.model_id,
        nonce: room.nonce,
        label: room.label,
        policy: room.policy || {},
      }));
    }
  }

  for (const provider of manifest.seed_providers || []) {
    adminTxs.push(txCommand({
      op: 'set_provider_payout',
      provider: provider.provider_pubkey,
      payout_addr: provider.payout?.addr,
      payout_method: provider.payout?.method,
    }));
    providerTxs.push(`# provider ${provider.provider_pubkey}`);
    providerTxs.push(txCommand({ op: 'register_provider' }));
    for (const join of provider.joins || []) {
      providerTxs.push(txCommand({ op: 'join_enclave', enclave_id: join.enclave_id }));
      for (const roomRef of join.rooms || []) {
        const room = roomByLabel.get(roomRef);
        providerTxs.push(txCommand({
          op: 'join_room',
          room_id: room?.room_id || `<room_id returned by open_room for ${roomRef}>`,
          enclave_id: join.enclave_id,
        }));
      }
    }
  }

  const paygateHealthUrl = paygateBase ? joinUrl(paygateBase, healthPath) : '';
  const checkoutCommands = [
    `mayhem pay stripe --paygate-url ${sh(paygateBase || '<paygate-url>')} --amount 10 --success-url ${sh(railCheckoutUrl(paygate, 'stripe', 'success_url'))} --cancel-url ${sh(railCheckoutUrl(paygate, 'stripe', 'cancel_url'))}`,
    `mayhem pay coinbase --paygate-url ${sh(paygateBase || '<paygate-url>')} --amount 10 --success-url ${sh(railCheckoutUrl(paygate, 'coinbase', 'success_url'))} --cancel-url ${sh(railCheckoutUrl(paygate, 'coinbase', 'cancel_url'))}`,
  ];

  return {
    boot,
    adminTxs,
    providerTxs,
    paygateHealthUrl,
    checkoutCommands,
    emergencyBan: txCommand({
      op: 'ban_provider',
      provider: '<provider-pubkey>',
      reason_hash: '<reason-hash-64-hex>',
    }),
  };
}

function printHuman(report, commands, { args }) {
  console.log(`Mayhem beta launch manifest: ${report.ok ? 'ok' : 'not ready'}`);
  console.log(`Copy/paste manifest path: ${path.resolve(repoRoot, args.manifest)}`);
  console.log(`Canonical enclaves: ${report.counts.canonical_enclaves}`);
  console.log(`Canonical rooms: ${report.counts.canonical_rooms}`);
  console.log(`Seed providers: ${report.counts.seed_providers}`);

  for (const warning of report.warnings) console.log(`warning: ${warning}`);
  for (const error of report.errors) console.error(`error: ${error}`);
  if (!args.commands || !commands) return;

  console.log('');
  console.log('Copy/paste admin bootstrap command:');
  for (const line of commands.boot) console.log(line);

  console.log('');
  console.log('Copy/paste admin contract commands:');
  for (const command of commands.adminTxs) console.log(command);

  console.log('');
  console.log('Copy/paste provider opt-in commands:');
  for (const command of commands.providerTxs) console.log(command);

  if (commands.paygateHealthUrl) {
    console.log('');
    console.log(`Copy/paste paygate health URL: ${commands.paygateHealthUrl}`);
  }

  console.log('');
  console.log('Copy/paste checkout commands; the CLI prints the hosted checkout URL before any browser open:');
  for (const command of commands.checkoutCommands) console.log(command);

  console.log('');
  console.log('Copy/paste emergency provider ban command:');
  console.log(commands.emergencyBan);
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const manifestPath = path.resolve(repoRoot, args.manifest);
  const manifest = readJson(manifestPath);
  const report = validateLaunchManifest(manifest, {
    manifestPath,
    allowPlaceholders: args.allowPlaceholders,
  });
  const commands = args.commands ? await buildCommands(manifest) : null;

  if (args.json) {
    console.log(JSON.stringify({ ...report, commands }, null, 2));
  } else {
    printHuman(report, commands, { args });
  }

  if (!report.ok) process.exit(1);
}

main().catch((error) => {
  console.error(error?.message || String(error));
  process.exit(1);
});

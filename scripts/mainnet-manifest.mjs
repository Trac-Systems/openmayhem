#!/usr/bin/env node
import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';

import { contractEpochAdminParamDefinitions } from '../intercom/contract/contract.js';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const defaultManifest = 'config/beta/mainnet.template.json';
const hex64 = /^[0-9a-fA-F]{64}$/;
const pubkey64 = /^[0-9a-fA-F]{64}$/;
const ethAddress = /^0x[0-9a-fA-F]{40}$/;
const tracAddress = /^trac1[0-9a-z]+$/;
const officialDht = [
  '116.202.214.149:10001',
  '157.180.12.214:10001',
  'node1.hyperdht.org:49737',
  'node2.hyperdht.org:49737',
  'node3.hyperdht.org:49737',
];

function usage() {
  console.log(`Usage: node scripts/mainnet-manifest.mjs [--manifest PATH] [--allow-placeholders] [--json] [--no-commands]

Validates the Mayhem mainnet manifest shape and prints copy/paste commands for
starting a Pear Intercom node on Trac mainnet. Strict mode fails placeholders;
use --allow-placeholders for the committed template.`);
}

function parseArgs(argv = process.argv.slice(2)) {
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

function resolveRepo(filePath) {
  return path.isAbsolute(filePath) ? filePath : path.resolve(repoRoot, filePath);
}

function readJson(filePath) {
  return JSON.parse(fs.readFileSync(resolveRepo(filePath), 'utf8'));
}

function isPlaceholder(value) {
  return typeof value === 'string' && (
    value.includes('<') ||
    value.includes('>') ||
    /\b(TBD|TODO|REPLACE|PLACEHOLDER|CHANGE_ME)\b/i.test(value)
  );
}

function containsPlaceholder(value) {
  return isPlaceholder(JSON.stringify(value));
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

function requireOnlyKeys(add, value, name, allowedKeys) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return;
  const allowed = new Set(allowedKeys);
  const unknown = Object.keys(value).filter((key) => !allowed.has(key)).sort();
  if (unknown.length > 0) add('error', `${name} contains unsupported field(s): ${unknown.join(', ')}`);
}

function requireLiteral(add, value, expected, name) {
  if (value !== expected) add('error', `${name} must be ${JSON.stringify(expected)}`);
}

function requireString(add, value, name, regex = null) {
  if (typeof value !== 'string' || value.length === 0) {
    add('error', `${name} must be a non-empty string`);
    return;
  }
  if (isPlaceholder(value)) {
    add('placeholder', `${name} still contains a template placeholder`);
    return;
  }
  if (regex && !regex.test(value)) add('error', `${name} has invalid format`);
}

function requireBooleanLiteral(add, value, expected, name) {
  if (value !== expected) add('error', `${name} must be ${expected}`);
}

function requireStringArray(add, value, name, min = 1) {
  if (!Array.isArray(value)) {
    add('error', `${name} must be an array`);
    return;
  }
  if (value.length < min) add('error', `${name} must contain at least ${min} item(s)`);
  for (const [index, item] of value.entries()) {
    requireString(add, item, `${name}[${index}]`);
  }
}

function requireOfficialDht(add, value, name) {
  requireStringArray(add, value, name, 1);
  if (!Array.isArray(value)) return;
  const missing = officialDht.filter((entry) => !value.includes(entry));
  if (missing.length > 0) add('error', `${name} is missing official HyperDHT bootstrap(s): ${missing.join(', ')}`);
}

function requirePositiveDecimalString(add, value, name) {
  if (typeof value !== 'string' || !/^[0-9]+$/.test(value) || BigInt(value) <= 0n) {
    add('error', `${name} must be a positive decimal string`);
  }
}

function validateControls(add, controls) {
  if (!requireObject(add, controls, 'controls')) return;
  requireOnlyKeys(add, controls, 'controls', [
    'admin_controls_economy',
    'admin_sets_prices',
    'admin_sets_rules',
    'admin_sets_params',
    'admin_sets_provider_payout_targets',
    'admin_can_ban_providers',
    'providers_set_prices',
    'providers_set_rules',
    'providers_set_params',
    'providers_set_payout_terms',
    'providers_submit_models',
    'providers_create_canonical_rooms',
    'providers_only_join_admin_rooms',
    'provider_payout_targets_admin_verified',
    'browser_handoffs_print_copy_paste_url',
  ]);
  for (const key of [
    'admin_controls_economy',
    'admin_sets_prices',
    'admin_sets_rules',
    'admin_sets_params',
    'admin_sets_provider_payout_targets',
    'admin_can_ban_providers',
    'providers_only_join_admin_rooms',
    'provider_payout_targets_admin_verified',
    'browser_handoffs_print_copy_paste_url',
  ]) {
    requireBooleanLiteral(add, controls[key], true, `controls.${key}`);
  }
  for (const key of [
    'providers_set_prices',
    'providers_set_rules',
    'providers_set_params',
    'providers_set_payout_terms',
    'providers_submit_models',
    'providers_create_canonical_rooms',
  ]) {
    requireBooleanLiteral(add, controls[key], false, `controls.${key}`);
  }
}

function validateParams(add, params, warnings) {
  if (!requireObject(add, params, 'contract.params')) return;
  const defs = contractEpochAdminParamDefinitions();
  const expectedKeys = Object.keys(defs).sort();
  const gotKeys = Object.keys(params).sort();
  const missing = expectedKeys.filter((key) => !gotKeys.includes(key));
  const extra = gotKeys.filter((key) => !expectedKeys.includes(key));
  if (missing.length > 0) add('error', `contract.params missing admin parameter(s): ${missing.join(', ')}`);
  if (extra.length > 0) add('error', `contract.params contains unknown parameter(s): ${extra.join(', ')}`);
  for (const [key, def] of Object.entries(defs)) {
    if (!(key in params)) continue;
    const value = params[key];
    if (def.money) {
      if (typeof value !== 'string' || !/^[0-9]+$/.test(value)) {
        add('error', `contract.params.${key} must be a non-negative decimal string`);
        continue;
      }
      const parsed = BigInt(value);
      if (parsed < BigInt(def.min) || (def.max !== undefined && parsed > BigInt(def.max))) {
        add('error', `contract.params.${key}=${value} outside contract bounds ${def.min}..${def.max ?? 'unbounded'}`);
      }
      if (value !== def.default) warnings.push(`contract.params.${key} differs from contract default ${def.default}`);
      continue;
    }
    if (!Number.isSafeInteger(value)) {
      add('error', `contract.params.${key} must be a safe integer`);
      continue;
    }
    if (value < def.min || value > def.max) {
      add('error', `contract.params.${key}=${value} outside contract bounds ${def.min}..${def.max}`);
    }
    if (value !== def.default) warnings.push(`contract.params.${key} differs from contract default ${def.default}`);
  }
  if (params.market_max_utilization_bps < params.market_target_utilization_bps) {
    add('error', 'contract.params.market_max_utilization_bps must be >= market_target_utilization_bps');
  }
}

function validateManifest(manifest, { allowPlaceholders = false } = {}) {
  const { errors, warnings, add } = issueFactory({ allowPlaceholders });
  requireOnlyKeys(add, manifest, 'manifest', [
    'schema_version',
    'launch_id',
    'network',
    'startup',
    'controls',
    'contract',
    'payments',
    'catalog',
  ]);
  requireLiteral(add, manifest.schema_version, 1, 'schema_version');
  requireString(add, manifest.launch_id, 'launch_id');

  if (requireObject(add, manifest.network, 'network')) {
    requireOnlyKeys(add, manifest.network, 'network', ['name', 'denom', 'msb', 'subnet', 'dht']);
    requireLiteral(add, manifest.network.name, 'mainnet', 'network.name');
    requireLiteral(add, manifest.network.denom, 'au_usd', 'network.denom');
    if (requireObject(add, manifest.network.msb, 'network.msb')) {
      requireOnlyKeys(add, manifest.network.msb, 'network.msb', ['address_prefix', 'network_id', 'bootstrap', 'channel']);
      requireLiteral(add, manifest.network.msb.address_prefix, 'trac', 'network.msb.address_prefix');
      requireLiteral(add, manifest.network.msb.network_id, 918, 'network.msb.network_id');
      requireLiteral(add, manifest.network.msb.bootstrap, 'acbc3a4344d3a804101d40e53db1dda82b767646425af73599d4cd6577d69685', 'network.msb.bootstrap');
      requireLiteral(add, manifest.network.msb.channel, '0000trac0network0msb0mainnet0000', 'network.msb.channel');
    }
    if (requireObject(add, manifest.network.subnet, 'network.subnet')) {
      requireOnlyKeys(add, manifest.network.subnet, 'network.subnet', ['channel', 'bootstrap']);
      requireString(add, manifest.network.subnet.channel, 'network.subnet.channel');
      requireString(add, manifest.network.subnet.bootstrap, 'network.subnet.bootstrap', hex64);
      if (
        !isPlaceholder(manifest.network.subnet.bootstrap) &&
        manifest.network.subnet.bootstrap === manifest.network.msb?.bootstrap
      ) {
        add('error', 'network.subnet.bootstrap must not equal network.msb.bootstrap');
      }
    }
    if (requireObject(add, manifest.network.dht, 'network.dht')) {
      requireOnlyKeys(add, manifest.network.dht, 'network.dht', ['peer_bootstrap', 'msb_bootstrap']);
      requireOfficialDht(add, manifest.network.dht.peer_bootstrap, 'network.dht.peer_bootstrap');
      requireOfficialDht(add, manifest.network.dht.msb_bootstrap, 'network.dht.msb_bootstrap');
    }
  }

  if (requireObject(add, manifest.startup, 'startup')) {
    requireOnlyKeys(add, manifest.startup, 'startup', [
      'runtime',
      'network_arg',
      'peer_store_name',
      'msb_store_name',
      'peer_rpc_url',
      'sc_bridge_url',
      'sc_bridge_token_env',
    ]);
    requireLiteral(add, manifest.startup.runtime, 'pear', 'startup.runtime');
    requireLiteral(add, manifest.startup.network_arg, 'mainnet', 'startup.network_arg');
    requireString(add, manifest.startup.peer_store_name, 'startup.peer_store_name');
    requireString(add, manifest.startup.msb_store_name, 'startup.msb_store_name');
    requireString(add, manifest.startup.peer_rpc_url, 'startup.peer_rpc_url');
    requireString(add, manifest.startup.sc_bridge_url, 'startup.sc_bridge_url');
    requireString(add, manifest.startup.sc_bridge_token_env, 'startup.sc_bridge_token_env');
  }

  validateControls(add, manifest.controls);

  if (requireObject(add, manifest.contract, 'contract')) {
    requireOnlyKeys(add, manifest.contract, 'contract', ['admin_peer_pubkey', 'key_env', 'params']);
    requireString(add, manifest.contract.admin_peer_pubkey, 'contract.admin_peer_pubkey', pubkey64);
    if (requireObject(add, manifest.contract.key_env, 'contract.key_env')) {
      for (const [key, value] of Object.entries(manifest.contract.key_env)) {
        requireString(add, value, `contract.key_env.${key}`, /^MAYHEM_[A-Z0-9_]+$/);
      }
    }
    validateParams(add, manifest.contract.params, warnings);
  }

  if (requireObject(add, manifest.payments, 'payments')) {
    requireOnlyKeys(add, manifest.payments, 'payments', ['rails', 'tnk', 'tap', 'fiat']);
    const rails = Array.isArray(manifest.payments.rails) ? manifest.payments.rails : [];
    if (rails.join(',') !== 'fiat,tap,tnk') add('error', 'payments.rails must be exactly ["fiat","tap","tnk"]');
    if (requireObject(add, manifest.payments.tnk, 'payments.tnk')) {
      requireOnlyKeys(add, manifest.payments.tnk, 'payments.tnk', [
        'address_prefix',
        'treasury_address',
        'operator_fee_address',
        'min_epoch_wallet_balance_tnk_e18',
        'sponsors_only',
      ]);
      requireLiteral(add, manifest.payments.tnk.address_prefix, 'trac', 'payments.tnk.address_prefix');
      requireString(add, manifest.payments.tnk.treasury_address, 'payments.tnk.treasury_address', tracAddress);
      requireString(add, manifest.payments.tnk.operator_fee_address, 'payments.tnk.operator_fee_address', tracAddress);
      requirePositiveDecimalString(add, manifest.payments.tnk.min_epoch_wallet_balance_tnk_e18, 'payments.tnk.min_epoch_wallet_balance_tnk_e18');
      const sponsorsOnly = Array.isArray(manifest.payments.tnk.sponsors_only) ? manifest.payments.tnk.sponsors_only : [];
      for (const forbidden of ['provider_join', 'user_setup', 'room_join', 'enclave_register']) {
        if (sponsorsOnly.includes(forbidden)) add('error', `payments.tnk.sponsors_only must not include ${forbidden}`);
      }
    }
    if (requireObject(add, manifest.payments.tap, 'payments.tap')) {
      requireOnlyKeys(add, manifest.payments.tap, 'payments.tap', [
        'chain_id',
        'token_address',
        'pool_address',
        'deployment_file_env',
        'eth_rpc_env',
        'eth_rpc_fallbacks_env',
        'max_epoch_delta_wei',
        'operator_address',
      ]);
      requireLiteral(add, manifest.payments.tap.chain_id, 1, 'payments.tap.chain_id');
      requireString(add, manifest.payments.tap.token_address, 'payments.tap.token_address', ethAddress);
      requireString(add, manifest.payments.tap.pool_address, 'payments.tap.pool_address', ethAddress);
      requireLiteral(add, manifest.payments.tap.deployment_file_env, 'MAYHEM_TAP_DEPLOYMENT_FILE', 'payments.tap.deployment_file_env');
      requireLiteral(add, manifest.payments.tap.eth_rpc_env, 'MAYHEM_TAP_ETH_RPC', 'payments.tap.eth_rpc_env');
      requireLiteral(add, manifest.payments.tap.eth_rpc_fallbacks_env, 'MAYHEM_TAP_ETH_RPC_FALLBACKS', 'payments.tap.eth_rpc_fallbacks_env');
      requirePositiveDecimalString(add, manifest.payments.tap.max_epoch_delta_wei, 'payments.tap.max_epoch_delta_wei');
      requireString(add, manifest.payments.tap.operator_address, 'payments.tap.operator_address', ethAddress);
    }
    if (requireObject(add, manifest.payments.fiat, 'payments.fiat')) {
      requireOnlyKeys(add, manifest.payments.fiat, 'payments.fiat', [
        'processor',
        'mode',
        'public_base_url',
        'webhook_path',
        'connect_enabled',
      ]);
      requireLiteral(add, manifest.payments.fiat.processor, 'stripe', 'payments.fiat.processor');
      requireLiteral(add, manifest.payments.fiat.mode, 'live', 'payments.fiat.mode');
      requireLiteral(add, manifest.payments.fiat.connect_enabled, true, 'payments.fiat.connect_enabled');
      requireString(add, manifest.payments.fiat.public_base_url, 'payments.fiat.public_base_url', /^https:\/\//);
      requireLiteral(add, manifest.payments.fiat.webhook_path, '/v1/stripe/webhook', 'payments.fiat.webhook_path');
    }
  }

  if (requireObject(add, manifest.catalog, 'catalog')) {
    requireOnlyKeys(add, manifest.catalog, 'catalog', ['source', 'admin_signed', 'models_command']);
    requireLiteral(add, manifest.catalog.source, 'ledger_anchor', 'catalog.source');
    requireLiteral(add, manifest.catalog.admin_signed, true, 'catalog.admin_signed');
    requireLiteral(add, manifest.catalog.models_command, 'mayhem models', 'catalog.models_command');
  }

  if (!allowPlaceholders && containsPlaceholder(manifest)) {
    errors.push('manifest still contains template placeholders');
  }

  return { ok: errors.length === 0, errors, warnings };
}

function sh(value) {
  const raw = String(value ?? '');
  return `'${raw.replaceAll("'", "'\\''")}'`;
}

function commaList(values) {
  return Array.isArray(values) ? values.join(',') : '';
}

function buildCommands(manifest) {
  const startup = manifest.startup || {};
  const network = manifest.network || {};
  const msb = network.msb || {};
  const subnet = network.subnet || {};
  const dht = network.dht || {};
  const peerDht = commaList(dht.peer_bootstrap);
  const msbDht = commaList(dht.msb_bootstrap);
  return {
    start_intercom: [
      'set -a',
      '. ./.mayhem-local/secrets/mayhem-live.env',
      'set +a',
      'pear_runtime="${MAYHEM_PEAR_RUNTIME:-$HOME/Library/Application Support/pear/current/by-arch/darwin-arm64/bin/pear-runtime}"',
      'cd intercom',
      `"$pear_runtime" run . --network mainnet --peer-store-name ${sh(startup.peer_store_name)} --msb-store-name ${sh(startup.msb_store_name)} --msb-bootstrap ${sh(msb.bootstrap)} --msb-channel ${sh(msb.channel)} --subnet-channel ${sh(subnet.channel)} --subnet-bootstrap ${sh(subnet.bootstrap)} --sc-bridge 1 --sc-bridge-host 127.0.0.1 --sc-bridge-port 49222 --sc-bridge-token "$${startup.sc_bridge_token_env || 'MAYHEM_SC_BRIDGE_TOKEN'}" --rpc 1 --rpc-host 127.0.0.1 --rpc-port 49223${peerDht ? ` --peer-dht-bootstrap ${sh(peerDht)}` : ''}${msbDht ? ` --msb-dht-bootstrap ${sh(msbDht)}` : ''}`,
    ],
    read_state: [
      'curl -sf http://127.0.0.1:49223/v1/health',
      "curl -sf 'http://127.0.0.1:49223/v1/state?prefix=params/&confirmed=false&limit=1000'",
      "curl -sf 'http://127.0.0.1:49223/v1/state?prefix=catalog/&confirmed=false&limit=20'",
    ],
    select_testnet: [
      'pear_runtime="${MAYHEM_PEAR_RUNTIME:-$HOME/Library/Application Support/pear/current/by-arch/darwin-arm64/bin/pear-runtime}"',
      'cd intercom',
      '"$pear_runtime" run . --network testnet1 --msb-bootstrap <testnet-msb-bootstrap-64-hex> --msb-channel <testnet-msb-channel> --subnet-channel <testnet-subnet-channel> --subnet-bootstrap <testnet-subnet-bootstrap-64-hex>',
    ],
  };
}

export function validateMainnetManifest(manifest, options = {}) {
  return validateManifest(manifest, options);
}

if (import.meta.url === `file://${process.argv[1]}`) {
  try {
    const args = parseArgs();
    const manifest = readJson(args.manifest);
    const validation = validateManifest(manifest, { allowPlaceholders: args.allowPlaceholders });
    const commands = args.commands ? buildCommands(manifest) : null;
    const report = {
      ok: validation.ok,
      manifest: args.manifest,
      warnings: validation.warnings,
      errors: validation.errors,
      commands,
    };
    if (args.json) {
      console.log(JSON.stringify(report, null, 2));
    } else {
      console.log(validation.ok ? 'mainnet manifest ok' : 'mainnet manifest failed');
      for (const warning of validation.warnings) console.log('warning:', warning);
      for (const error of validation.errors) console.log('error:', error);
      if (commands) {
        console.log('\nStart mainnet Intercom:');
        console.log(commands.start_intercom.join('\n'));
        console.log('\nRead local replicated state:');
        console.log(commands.read_state.join('\n'));
      }
    }
    process.exit(validation.ok ? 0 : 1);
  } catch (error) {
    console.error('mainnet-manifest:', error?.message || String(error));
    process.exit(1);
  }
}

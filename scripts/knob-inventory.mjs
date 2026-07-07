#!/usr/bin/env node
import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const markdownPath = path.join(repoRoot, 'docs/reference/knob-inventory.md');
const jsonPath = path.join(repoRoot, 'docs/reference/knob-inventory.json');

const cliBinaries = [
  { name: 'mayhem', path: 'target/debug/mayhem', recursive: true },
  { name: 'mayhemd', path: 'target/debug/mayhemd', recursive: false },
  { name: 'mayhem-enclave', path: 'target/debug/mayhem-enclave', recursive: true },
  { name: 'mayhem-gateway', path: 'target/debug/mayhem-gateway', recursive: false },
  { name: 'mayhem-paygate', path: 'target/debug/mayhem-paygate', recursive: false },
];

const configStructSections = new Map([
  ['ConfigIdentity', 'identity'],
  ['ConfigNetwork', 'network'],
  ['ConfigProvider', 'provider'],
  ['ConfigProviderLimits', 'provider.limits'],
  ['ConfigUser', 'user'],
  ['ConfigRole', 'role'],
  ['SupervisorConfig', 'supervisor'],
  ['ChildConfig', 'supervisor.children[]'],
  ['ServerConfigFile', 'server'],
  ['ContractConfigFile', 'contract'],
  ['OracleConfigFile', 'oracle'],
  ['StripeConfigFile', 'stripe'],
  ['CoinbaseConfigFile', 'coinbase'],
]);

const sourceExtensions = new Set(['.rs', '.js', '.mjs', '.sh', '.py']);
const envPrefixes = [
  'MAYHEM_',
  'TRAC_',
  'PEER_',
  'MSB_',
  'SC_BRIDGE',
  'SIDECHANNEL',
  'SESSION_',
  'STRIPE_',
  'COINBASE_',
  'TAP_',
  'TK_',
  'ETH_',
  'HF_',
  'CARGO_',
  'PROCESSOR_',
  'HOME',
  'PATH',
];

function usage() {
  console.log(`Usage: node scripts/knob-inventory.mjs [--write|--check|--json]

Generates the B6 operational knob inventory from current source:
  - Clap/public CLI help for local Mayhem binaries
  - environment variables used by Rust/JS/shell code
  - TOML config keys from config structs and accessors
  - Mayhem HTTP headers
  - operational default constants

Run cargo build -p mayhem-cli -p mayhemd -p mayhem-enclave -p mayhem-gateway -p mayhem-paygate first if binaries are absent.`);
}

function parseArgs(argv) {
  const args = { write: false, check: false, json: false };
  for (const arg of argv) {
    if (arg === '--write') args.write = true;
    else if (arg === '--check') args.check = true;
    else if (arg === '--json') args.json = true;
    else if (arg === '-h' || arg === '--help') {
      usage();
      process.exit(0);
    } else {
      throw new Error(`unknown argument: ${arg}`);
    }
  }
  if (!args.write && !args.check && !args.json) args.write = true;
  return args;
}

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: repoRoot,
    encoding: 'utf8',
    maxBuffer: 24 * 1024 * 1024,
    ...options,
  });
  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(' ')} failed:\n${result.stderr || result.stdout}`);
  }
  return result.stdout;
}

function gitFiles() {
  return run('git', ['ls-files'])
    .split('\n')
    .map((line) => line.trim())
    .filter(Boolean);
}

function sourceFiles() {
  return gitFiles().filter((file) => {
    if (file.includes('/node_modules/') || file.endsWith('package-lock.json')) return false;
    if (file.startsWith('target/') || file.startsWith('docs/')) return false;
    return sourceExtensions.has(path.extname(file));
  });
}

function lineRefs(file, text) {
  const refs = [];
  const lines = text.split('\n');
  for (let i = 0; i < lines.length; i += 1) {
    refs.push({ file, line: i + 1, text: lines[i] });
  }
  return refs;
}

function addOccurrence(map, name, source, detail = '') {
  if (!name || !envPrefixes.some((prefix) => name === prefix || name.startsWith(prefix))) return;
  const item = map.get(name) || { name, sources: new Set(), details: new Set() };
  item.sources.add(source);
  if (detail) item.details.add(detail);
  map.set(name, item);
}

function extractEnvironment(files) {
  const vars = new Map();
  for (const file of files) {
    const text = fs.readFileSync(path.join(repoRoot, file), 'utf8');
    for (const ref of lineRefs(file, text)) {
      for (const regex of [
        /(?:std::)?env::var(?:_os)?\("([A-Z0-9_]+)"\)/g,
        /std::env::var(?:_os)?\("([A-Z0-9_]+)"\)/g,
        /process\.env\.([A-Z0-9_]+)/g,
        /process\.env\[['"]([A-Z0-9_]+)['"]\]/g,
        /\benv\.([A-Z0-9_]+)/g,
        /\$\{?([A-Z][A-Z0-9_]{2,})\}?/g,
      ]) {
        for (const match of ref.text.matchAll(regex)) {
          addOccurrence(vars, match[1], `${ref.file}:${ref.line}`, ref.text.trim());
        }
      }
    }
  }
  return [...vars.values()]
    .map((item) => ({
      name: item.name,
      sources: [...item.sources].sort(),
      details: [...item.details].slice(0, 3).sort(),
    }))
    .sort((a, b) => a.name.localeCompare(b.name));
}

function extractConfigKeys(files) {
  const keys = new Map();
  const add = (key, source, why) => {
    if (!key || !key.includes('.')) return;
    const item = keys.get(key) || { key, sources: new Set(), why: new Set() };
    item.sources.add(source);
    item.why.add(why);
    keys.set(key, item);
  };
  for (const file of files.filter((name) => name.endsWith('.rs'))) {
    const text = fs.readFileSync(path.join(repoRoot, file), 'utf8');
    for (const ref of lineRefs(file, text)) {
      for (const regex of [
        /toml_(?:get_path|set_string|set_u64|remove_path|non_empty_string)\([^,]+,\s*"([^"]+)"/g,
        /toml_get_path\([^,]+,\s*"([^"]+)"/g,
        /validate_hex32_config\("([^"]+)"/g,
      ]) {
        for (const match of ref.text.matchAll(regex)) {
          add(match[1], `${ref.file}:${ref.line}`, 'TOML accessor');
        }
      }
    }
    for (const [structName, section] of configStructSections.entries()) {
      const match = text.match(new RegExp(`struct\\s+${structName}\\s*\\{([\\s\\S]*?)\\n\\}`, 'm'));
      if (!match) continue;
      const startLine = text.slice(0, match.index).split('\n').length;
      const lines = match[1].split('\n');
      for (let i = 0; i < lines.length; i += 1) {
        const field = lines[i].match(/^\s*(?:pub\s+)?([a-z][a-z0-9_]*)\s*:/);
        if (!field) continue;
        add(`${section}.${field[1]}`, `${file}:${startLine + i + 1}`, `deserialized ${structName}`);
      }
    }
  }
  return [...keys.values()]
    .map((item) => ({
      key: item.key,
      sources: [...item.sources].sort(),
      why: [...item.why].sort(),
    }))
    .sort((a, b) => a.key.localeCompare(b.key));
}

function extractHeaders(files) {
  const headers = new Map();
  for (const file of files.filter((name) => name.endsWith('.rs'))) {
    const text = fs.readFileSync(path.join(repoRoot, file), 'utf8');
    for (const ref of lineRefs(file, text)) {
      for (const match of ref.text.matchAll(/x-mayhem-[a-z0-9-]+/g)) {
        const name = match[0].toLowerCase();
        const item = headers.get(name) || { name, sources: new Set() };
        item.sources.add(`${ref.file}:${ref.line}`);
        headers.set(name, item);
      }
    }
  }
  return [...headers.values()]
    .map((item) => ({ name: item.name, sources: [...item.sources].sort() }))
    .sort((a, b) => a.name.localeCompare(b.name));
}

function extractDefaults(files) {
  const defaults = new Map();
  for (const file of files.filter((name) => name.endsWith('.rs'))) {
    const text = fs.readFileSync(path.join(repoRoot, file), 'utf8');
    for (const ref of lineRefs(file, text)) {
      const match = ref.text.match(/(?:pub\s+)?const\s+([A-Z0-9_]*(?:DEFAULT|TIMEOUT|TTL|COOLOFF|CHECKPOINT|PORT|FRAME|MILLIS|SECONDS|BYTES)[A-Z0-9_]*)[^=]*=\s*([^;]+);/);
      if (!match) continue;
      const name = match[1];
      const item = defaults.get(name) || { name, value: match[2].trim(), sources: new Set() };
      item.sources.add(`${ref.file}:${ref.line}`);
      defaults.set(name, item);
    }
  }
  return [...defaults.values()]
    .map((item) => ({ name: item.name, value: item.value, sources: [...item.sources].sort() }))
    .sort((a, b) => a.name.localeCompare(b.name));
}

const contractParamDescriptions = new Map([
  ['epoch_seconds', 'Settlement epoch length used by rollup/watchers.'],
  ['fee_bps', 'Operator fee charged during epoch apply.'],
  ['challenge_epochs', 'Epoch commit/fraud-proof challenge window.'],
  ['holdback_epochs', 'Provider earnings holdback before payout maturity.'],
  ['payout_min_mu', 'Minimum payout/settlement threshold.'],
  ['probation_successful_sessions', 'Successful sessions required before probation can clear.'],
  ['probation_seconds', 'Time window before probation can clear.'],
  ['probation_max_concurrent_sessions_per_user', 'Probation concurrent-session cap per user.'],
  ['probation_price_max_bps', 'Probation price cap relative to reference.'],
  ['probation_weight_bps', 'Routing weight while a provider is on probation.'],
  ['auditor_min_reputation_bps', 'Minimum auditor reputation.'],
  ['auditor_min_age_seconds', 'Minimum auditor account age.'],
  ['canary_match_min_bps', 'Canary probe match threshold.'],
  ['canary_probe_holdback_bps', 'Extra probe-gated provider holdback share.'],
  ['canary_probe_release_min_passes', 'Probe passes needed for gated holdback release.'],
  ['probe_reward_mu', 'Auditor probe reward.'],
  ['uptime_tick_seconds', 'Uptime probe cadence.'],
  ['fraud_slash_bps', 'Slash percentage for canary mismatch and fraud-proof provider penalties.'],
  ['dispute_lost_slash_bps', 'Slash percentage for provider-fault dispute losses.'],
  ['rate_staleness_seconds', 'Maximum accepted TNK/TAP oracle price age.'],
  ['price_min_bps', 'Admin seed price lower bound versus model reference.'],
  ['price_max_bps', 'Admin seed price upper bound versus model reference.'],
  ['rules_grace_seconds', 'Rules/app compatibility grace window.'],
  ['max_apply_batch', 'Maximum epochApply debits plus earnings per page.'],
  ['dispute_deposit_mu', 'Dispute bond amount.'],
  ['price_rate_limit_seconds', 'Admin seed price change throttle.'],
  ['market_target_utilization_bps', 'Target utilization for the market price curve.'],
  ['market_ema_alpha_bps', 'Utilization EMA weight per epoch.'],
  ['market_gain_bps', 'Dampening gain toward the desired market price.'],
  ['market_max_step_bps', 'Per-epoch market price movement clamp.'],
  ['market_cold_start_min_providers', 'Minimum active supply before floating away from the admin seed.'],
  ['market_provider_epoch_target_mu', 'Per-provider epoch capacity unit for utilization.'],
  ['market_max_utilization_bps', 'Utilization cap used by the controller.'],
  ['market_below_target_discount_bps', 'Below-target discount slope cap.'],
  ['market_above_target_slope_bps', 'Above-target premium slope.'],
  ['param_activation_delay_seconds', 'Governance delay for future set_params changes.'],
]);

function extractContractAdminParams() {
  const file = 'intercom/contract/contract.js';
  const full = path.join(repoRoot, file);
  if (!fs.existsSync(full)) return [];
  const text = fs.readFileSync(full, 'utf8');
  const constants = new Map();
  for (const match of text.matchAll(/const\s+([A-Z0-9_]+)\s*=\s*([^;]+);/g)) {
    constants.set(match[1], match[2].trim());
  }
  const block = text.match(/const PARAM_DEFINITIONS = Object\.freeze\(\{\n([\s\S]*?)\n\}\);/);
  if (!block) return [];
  const blockStartLine = text.slice(0, block.index).split('\n').length;
  return block[1]
    .split('\n')
    .map((line, idx) => {
      const match = line.match(/^\s{2}([a-z0-9_]+):\s*\{\s*default:\s*([^,]+),\s*min:\s*([^,]+),\s*max:\s*([^}]+)\s*\},?/);
      if (!match) return null;
      const defaultExpression = match[2].trim();
      const resolvedDefault = constants.get(defaultExpression) ?? null;
      return {
        name: match[1],
        default_expression: defaultExpression,
        resolved_default: resolvedDefault,
        min: match[3].trim(),
        max: match[4].trim(),
        description: contractParamDescriptions.get(match[1]) || 'Admin-governed intercom contract parameter.',
        source: `${file}:${blockStartLine + idx + 1}`,
      };
    })
    .filter(Boolean);
}

function parseHelp(help) {
  const lines = help.replace(/\r/g, '').split('\n');
  const commands = [];
  const options = [];
  let section = '';
  let lastOption = null;
  for (const line of lines) {
    const sectionMatch = line.match(/^([A-Za-z][A-Za-z ]+):\s*$/);
    if (sectionMatch) {
      section = sectionMatch[1].toLowerCase();
      lastOption = null;
      continue;
    }
    if (section === 'commands') {
      const match = line.match(/^\s{2}([a-z0-9][a-z0-9-]*)(?:\s+(.*))?$/);
      if (match) {
        const desc = (match[2] || '').trim();
        commands.push({
          name: match[1],
          description: desc.replace(/\s*\[aliases?:.*?\]\s*/g, '').trim(),
          raw: desc,
        });
      }
      continue;
    }
    if (section === 'options' || section === 'arguments') {
      const match = line.match(/^\s{2,}((?:-[A-Za-z],\s*)?--?[A-Za-z0-9][A-Za-z0-9-]*(?:\s+<[^>]+>|\s+\[[^\]]+\])?|<[A-Z0-9_-]+>|\[[A-Z0-9_-]+\])(?:\s{2,}(.*))?$/);
      if (match) {
        lastOption = {
          usage: match[1].trim(),
          description: (match[2] || '').trim(),
        };
        options.push(lastOption);
      } else if (lastOption && /^\s{8,}\S/.test(line)) {
        lastOption.description = `${lastOption.description} ${line.trim()}`.trim();
      }
    }
  }
  return { commands, options };
}

function helpText(binary, args) {
  const full = path.join(repoRoot, binary.path);
  if (!fs.existsSync(full)) {
    throw new Error(`${binary.path} is missing; run cargo build for ${binary.name}`);
  }
  return run(full, [...args, '--help']);
}

function collectCliCommand(binary, commandPath = []) {
  const help = helpText(binary, commandPath);
  const parsed = parseHelp(help);
  const command = {
    binary: binary.name,
    command: [binary.name, ...commandPath].join(' '),
    path: commandPath,
    options: parsed.options,
    subcommands: parsed.commands,
  };
  const children = [];
  if (binary.recursive) {
    for (const sub of parsed.commands) {
      if (sub.name === 'help') continue;
      try {
        children.push(...collectCliCommand(binary, [...commandPath, sub.name]));
      } catch (error) {
        children.push({
          binary: binary.name,
          command: [binary.name, ...commandPath, sub.name].join(' '),
          path: [...commandPath, sub.name],
          options: [],
          subcommands: [],
          error: error.message,
        });
      }
    }
  }
  return [command, ...children];
}

function collectCli() {
  return cliBinaries.flatMap((binary) => collectCliCommand(binary));
}

function whatCliOption(option) {
  return option.description || 'Clap-defined command-line knob; see command context.';
}

function defaultFromHelp(description) {
  const match = description.match(/\[default:\s*([^\]]+)\]/i);
  return match ? match[1] : 'command default or unset';
}

function whenForName(name, category) {
  const lower = name.toLowerCase();
  if (lower.includes('home')) return 'Use a separate Mayhem home or isolated smoke environment.';
  if (lower.includes('port') || lower.includes('bind')) return 'Change when the default loopback port is occupied or a local service must bind elsewhere.';
  if (lower.includes('rpc') || lower.includes('bridge') || lower.includes('url')) return 'Point this component at a different local peer, bridge, gateway, paygate, or upstream service.';
  if (lower.includes('token') || lower.includes('key') || lower.includes('secret') || lower.includes('password')) return 'Set only from local secret storage or a controlled operator environment.';
  if (lower.includes('timeout') || lower.includes('ttl') || lower.includes('cooloff')) return 'Tune only for slow networks, overloaded providers, or controlled failure testing.';
  if (lower.includes('rail') || lower.includes('stripe') || lower.includes('tap') || lower.includes('tnk')) return 'Use when selecting or operating a payment rail.';
  if (lower.includes('provider') || lower.includes('enclave') || lower.includes('model')) return 'Use when selecting a specific admin-canonical provider, enclave, model, or artifact.';
  if (category === 'header') return 'Set per request when a client needs stricter routing, failover, or dashboard authentication behavior.';
  if (category === 'config') return 'Persist when this value should survive across `mayhem up` runs.';
  return 'Change only when the default does not match the local operator workflow.';
}

function markdownTable(headers, rows) {
  const escape = (value) => String(value ?? '').replace(/\|/g, '\\|').replace(/\n/g, '<br>');
  return [
    `| ${headers.join(' | ')} |`,
    `| ${headers.map(() => '---').join(' | ')} |`,
    ...rows.map((row) => `| ${row.map(escape).join(' | ')} |`),
  ].join('\n');
}

function renderMarkdown(inventory) {
  const out = [];
  out.push('# Mayhem Knob Inventory');
  out.push('');
  out.push('Generated by `node scripts/knob-inventory.mjs --write`. Do not edit this file by hand.');
  out.push('');
  out.push('Run `node scripts/knob-inventory.mjs --check` after CLI/config/header/default changes. A clean check is the B6 proof that the code-derived inventory and this reference match.');
  out.push('');
  out.push('## Summary');
  out.push('');
  out.push(markdownTable(['Surface', 'Count'], [
    ['CLI command pages', inventory.cli.length],
    ['CLI options/arguments', inventory.cli.reduce((sum, item) => sum + item.options.length, 0)],
    ['Environment variables', inventory.environment.length],
    ['TOML config keys', inventory.config.length],
    ['Mayhem HTTP headers', inventory.headers.length],
    ['Intercom contract admin params', inventory.contract_admin_params.length],
    ['Operational defaults', inventory.defaults.length],
  ]));
  out.push('');
  out.push('## CLI Commands And Flags');
  out.push('');
  for (const command of inventory.cli) {
    out.push(`### \`${command.command}\``);
    out.push('');
    if (command.error) {
      out.push(`Inventory warning: ${command.error}`);
      out.push('');
      continue;
    }
    if (command.subcommands.length) {
      out.push(markdownTable(['Subcommand', 'What it does'], command.subcommands.map((sub) => [
        `\`${sub.name}\``,
        sub.description || sub.raw || 'See nested help.',
      ])));
      out.push('');
    }
    if (command.options.length) {
      out.push(markdownTable(['Flag / argument', 'What it does', 'Default', 'When to change it'], command.options.map((option) => [
        `\`${option.usage}\``,
        whatCliOption(option),
        defaultFromHelp(option.description),
        whenForName(option.usage, 'cli'),
      ])));
      out.push('');
    } else if (!command.subcommands.length) {
      out.push('No public flags or subcommands.');
      out.push('');
    }
  }
  out.push('## Environment Variables');
  out.push('');
  out.push(markdownTable(['Variable', 'What it does', 'Default/source', 'When to change it', 'Source'], inventory.environment.map((env) => [
    `\`${env.name}\``,
    env.details[0] || 'Environment override read by source code.',
    env.details.find((detail) => / \|\| |unwrap_or|default/i.test(detail)) || 'Unset unless provided by the shell or service manager.',
    whenForName(env.name, 'env'),
    env.sources.slice(0, 5).join(', '),
  ])));
  out.push('');
  out.push('## TOML Config Keys');
  out.push('');
  out.push(markdownTable(['Key', 'What it does', 'Default/source', 'When to change it', 'Source'], inventory.config.map((entry) => [
    `\`${entry.key}\``,
    entry.why.join('; '),
    'Loaded from config file when present; CLI/env may override some keys.',
    whenForName(entry.key, 'config'),
    entry.sources.slice(0, 5).join(', '),
  ])));
  out.push('');
  out.push('## Mayhem HTTP Headers');
  out.push('');
  out.push(markdownTable(['Header', 'What it does', 'Default', 'When to change it', 'Source'], inventory.headers.map((header) => [
    `\`${header.name}\``,
    header.name.includes('timeout') ? 'Per-request failover timeout control.' :
      header.name.includes('tok-s') ? 'Per-request minimum sustained throughput floor.' :
      header.name.includes('att') ? 'Attestation or dashboard authentication/header evidence field.' :
      header.name.includes('receipt') ? 'Receipt/reporting response header.' :
      header.name.includes('hedge') ? 'Requests an eligible hedge probe route.' :
      'Mayhem request/response metadata header.',
    'Unset unless the client or gateway sets it.',
    whenForName(header.name, 'header'),
    header.sources.slice(0, 5).join(', '),
  ])));
  out.push('');
  out.push('## Intercom Contract Admin Params');
  out.push('');
  out.push('These `PARAM_DEFINITIONS` values are governed by admin `set_params`; providers and users cannot set them.');
  out.push('');
  out.push(markdownTable(['Parameter', 'Default expression', 'Bounds', 'What it controls', 'Source'], inventory.contract_admin_params.map((param) => [
    `\`${param.name}\``,
    param.resolved_default ? `\`${param.default_expression}\` (= \`${param.resolved_default}\`)` : `\`${param.default_expression}\``,
    `\`${param.min}\` .. \`${param.max}\``,
    param.description,
    param.source,
  ])));
  out.push('');
  out.push('## Operational Defaults');
  out.push('');
  out.push(markdownTable(['Constant', 'Default expression', 'What it controls', 'When to change it', 'Source'], inventory.defaults.map((entry) => [
    `\`${entry.name}\``,
    `\`${entry.value}\``,
    entry.name.toLowerCase().replaceAll('_', ' '),
    whenForName(entry.name, 'default'),
    entry.sources.slice(0, 5).join(', '),
  ])));
  return `${out.join('\n').replace(/\n+$/, '')}\n`;
}

function inventoryJson(inventory) {
  return `${JSON.stringify(inventory, null, 2)}\n`;
}

function buildInventory() {
  const files = sourceFiles();
  return {
    generated_by: 'scripts/knob-inventory.mjs',
    cli: collectCli(),
    environment: extractEnvironment(files),
    config: extractConfigKeys(files),
    headers: extractHeaders(files),
    contract_admin_params: extractContractAdminParams(),
    defaults: extractDefaults(files),
  };
}

function ensureDir(filePath) {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
}

function checkFile(filePath, expected) {
  const current = fs.existsSync(filePath) ? fs.readFileSync(filePath, 'utf8') : '';
  if (current !== expected) {
    throw new Error(`${path.relative(repoRoot, filePath)} is stale; run node scripts/knob-inventory.mjs --write`);
  }
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const inventory = buildInventory();
  const md = renderMarkdown(inventory);
  const json = inventoryJson(inventory);
  if (args.json) {
    process.stdout.write(json);
  }
  if (args.write) {
    ensureDir(markdownPath);
    fs.writeFileSync(markdownPath, md);
    fs.writeFileSync(jsonPath, json);
    console.log(`wrote ${path.relative(repoRoot, markdownPath)} and ${path.relative(repoRoot, jsonPath)}`);
  }
  if (args.check) {
    checkFile(markdownPath, md);
    checkFile(jsonPath, json);
    console.log('knob inventory is current');
  }
}

main();

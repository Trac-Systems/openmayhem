#!/usr/bin/env node
import { spawn, spawnSync } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import fsp from 'node:fs/promises';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import process from 'node:process';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { setTimeout as sleep } from 'node:timers/promises';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(__dirname, '..');
const require = createRequire(import.meta.url);
const { blake3 } = require(path.join(ROOT, 'intercom/node_modules/@tracsystems/blake3'));

const DEFAULT_MODEL = 'qwen/qwen3.5-4b-gguf-q4_k_m-dev';
const DEFAULT_ARTIFACT = path.join(
  os.homedir(),
  '.mayhem/cache/huggingface/lmstudio-community/Qwen3.5-4B-GGUF/f9f88ac3e234be915e23811a6d28ea287bdb927e/Qwen3.5-4B-Q4_K_M.gguf'
);
const CHUNK_SIZE = 8 * 1024 * 1024;
const SSH_OPTS = [
  '-F', '/dev/null',
  '-o', 'IdentitiesOnly=yes',
  '-o', 'PreferredAuthentications=password,keyboard-interactive',
  '-o', 'PubkeyAuthentication=no',
  '-o', 'PasswordAuthentication=yes',
  '-o', 'KbdInteractiveAuthentication=yes',
  '-o', 'NumberOfPasswordPrompts=1',
  '-o', 'ConnectTimeout=8',
  '-o', 'ExitOnForwardFailure=yes',
  '-o', 'ServerAliveInterval=15',
  '-o', 'ServerAliveCountMax=3',
  '-o', 'StrictHostKeyChecking=no',
  '-o', `UserKnownHostsFile=${path.join(ROOT, '.mayhem-local/macmini-known-hosts')}`,
];

const children = [];
const sensitiveFiles = [];
const cleanupState = {
  remote: null,
  passFile: null,
  remotePids: [],
  localChildren: [],
  devnetLog: null,
  remoteRun: null,
};
let cleanupStarted = false;

function usage() {
  console.log(`Usage:
  node scripts/dev-phase3-opencode-p2p-smoke.mjs

Runs the P3.6 acceptance smoke:
  - local Pear admin/user gateway
  - Mac mini Pear provider
  - admin-created enclave/room/price
  - opencode tool task through the local OpenAI-compatible gateway

Environment:
  MAYHEM_PHASE3_MACMINI_FILE   Mac mini credential file (default: ../gpd/macmini.txt)
  MAYHEM_PHASE3_REMOTE_ROOT    Remote checkout/staging root (default: ~/mayhem-macmini-p33)
  MAYHEM_PHASE3_MODEL          Catalog model id (default: ${DEFAULT_MODEL})
  MAYHEM_PHASE3_ARTIFACT       Local artifact path (default: cached Qwen GGUF)
  MAYHEM_PHASE3_PROVIDER_MODE  real or shim (default: real)
  MAYHEM_PHASE3_RUN_OPENCODE   Also run the opencode tool-call smoke after curl chat (default: 0)
  MAYHEM_PHASE3_CARGO_OPT_LEVEL Cargo dev opt-level for real provider smoke (default: 3 in real mode)
  MAYHEM_PHASE3_PROVIDER_START_TIMEOUT_SECONDS Remote provider startup timeout (default: 1800 real, 300 shim)
  MAYHEM_PHASE3_SESSION_OPEN_TIMEOUT_SECONDS Gateway direct-session open timeout (default: 90 real, 45 shim)
  MAYHEM_PHASE3_SESSION_TTFT_TIMEOUT_SECONDS Gateway first-token timeout (default: 300 real, 60 shim)
  MAYHEM_PHASE3_SESSION_FRAME_TIMEOUT_SECONDS Gateway frame idle timeout (default: 300 real, 60 shim)
  MAYHEM_PHASE3_CHAT_TIMEOUT_SECONDS Curl-compatible chat timeout (default: 420 real, 120 shim)
  MAYHEM_PHASE3_CHAT_MAX_TOKENS Curl-compatible chat max_tokens (default: 32)
  MAYHEM_PHASE3_CHAT_ATTEMPTS Curl-compatible chat attempts after provider is ready (default: 3)
  MAYHEM_PHASE3_CHAT_RETRY_DELAY_SECONDS Delay between chat retries (default: 35; gateway cooloff is 30s)
  MAYHEM_PHASE3_LOCAL_JOINERS Local dev-net joiners to start, 1 or 2 (default: 2)
  MAYHEM_PHASE3_OPENCODE_BIN   opencode binary path/name (default: opencode)
  MAYHEM_PHASE3_LOCAL_DHT_HOST  Local LAN IP advertised to the Mac mini DHT peer
  MAYHEM_PHASE3_USE_REMOTE_DHT Start/use a temporary Mac mini HyperDHT bootstrap (default: 1)
  MAYHEM_PHASE3_USE_LOCAL_DHT  Start/use a temporary local HyperDHT bootstrap (default: 0)
  MAYHEM_PHASE3_USE_PUBLIC_DHT Use default/public peer DHT bootstrap instead of a private one (default: 0)
  MAYHEM_PHASE3_PEER_DHT_BOOTSTRAP  Explicit peer DHT bootstrap list; overrides local/public default
  MAYHEM_PHASE3_KEEP_REMOTE    Keep remote run stores/logs after cleanup (default: 1)
  MAYHEM_PHASE3_KEEP_LOCAL     Keep local run evidence after cleanup (default: 1)
`);
}

function log(message) {
  process.stderr.write(`[p3.6] ${message}\n`);
}

function fail(message) {
  throw new Error(message);
}

function sh(value) {
  return `'${String(value).replaceAll("'", "'\\''")}'`;
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, 'utf8'));
}

function textRateMapJson(inPer1kMu, outPer1kMu) {
  return JSON.stringify([
    { unit: 'input_token', per_unit_mu: Number(inPer1kMu), granularity: 1000 },
    { unit: 'output_token', per_unit_mu: Number(outPer1kMu), granularity: 1000 },
  ]);
}

function parseFirstJsonObject(text) {
  const start = text.indexOf('{');
  if (start < 0) return null;
  let depth = 0;
  let inString = false;
  let escape = false;
  for (let i = start; i < text.length; i += 1) {
    const ch = text[i];
    if (inString) {
      if (escape) {
        escape = false;
      } else if (ch === '\\') {
        escape = true;
      } else if (ch === '"') {
        inString = false;
      }
      continue;
    }
    if (ch === '"') {
      inString = true;
    } else if (ch === '{') {
      depth += 1;
    } else if (ch === '}') {
      depth -= 1;
      if (depth === 0) return JSON.parse(text.slice(start, i + 1));
    }
  }
  return null;
}

function writeJson(file, value) {
  fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`);
}

function envPositiveInt(name, fallback) {
  const raw = process.env[name];
  if (raw === undefined || raw === '') return fallback;
  const value = Number.parseInt(raw, 10);
  if (!Number.isSafeInteger(value) || value <= 0) {
    fail(`${name} must be a positive integer, got ${raw}`);
  }
  return value;
}

async function runStreamingChatSmoke(gatewayUrl, modelId, runDir, { timeoutMs = 240_000, maxTokens = 32 } = {}) {
  const rawPath = path.join(runDir, 'gateway-chat-stream.sse');
  const summaryPath = path.join(runDir, 'gateway-chat-stream.json');
  const prompt = [
    'Write one short sentence proving this is a live model response.',
    'Include the word mayhem and name a color that is not blue.',
  ].join(' ');
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(new Error('streaming chat timed out')), timeoutMs);
  let raw = '';
  let content = '';
  let dataEvents = 0;
  let jsonEvents = 0;
  let doneSeen = false;
  const startedAt = Date.now();
  let responseHeadersAt = null;
  let firstChunkAt = null;
  let firstDataAt = null;
  let firstJsonAt = null;
  let firstContentAt = null;
  let doneAt = null;
  try {
    const response = await fetch(`${gatewayUrl}/v1/chat/completions`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        model: modelId,
        stream: true,
        temperature: 0.7,
        max_tokens: maxTokens,
        messages: [{ role: 'user', content: prompt }],
      }),
      signal: controller.signal,
    });
    responseHeadersAt = Date.now();
    if (!response.ok) {
      const text = await response.text();
      fail(`streaming chat returned HTTP ${response.status}: ${text}`);
    }
    if (!response.body) fail('streaming chat response had no body');
    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let buffer = '';
    const handleLine = (line) => {
      if (!line.startsWith('data:')) return;
      const data = line.slice(5).trim();
      if (!data) return;
      dataEvents += 1;
      if (firstDataAt === null) firstDataAt = Date.now();
      if (data === '[DONE]') {
        doneSeen = true;
        doneAt = Date.now();
        return;
      }
      let value;
      try {
        value = JSON.parse(data);
      } catch {
        return;
      }
      jsonEvents += 1;
      if (firstJsonAt === null) firstJsonAt = Date.now();
      const choice = Array.isArray(value.choices) ? value.choices[0] : null;
      const delta = choice?.delta?.content;
      const message = choice?.message?.content;
      if (typeof delta === 'string') {
        if (delta.length > 0 && firstContentAt === null) firstContentAt = Date.now();
        content += delta;
      } else if (typeof message === 'string') {
        if (message.length > 0 && firstContentAt === null) firstContentAt = Date.now();
        content += message;
      }
    };
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      if (firstChunkAt === null) firstChunkAt = Date.now();
      const chunk = decoder.decode(value, { stream: true });
      raw += chunk;
      buffer += chunk;
      const lines = buffer.split(/\r?\n/);
      buffer = lines.pop() || '';
      for (const line of lines) handleLine(line);
    }
    const tail = decoder.decode();
    if (tail) {
      raw += tail;
      buffer += tail;
    }
    if (buffer) handleLine(buffer);
  } finally {
    clearTimeout(timer);
  }
  fs.writeFileSync(rawPath, raw);
  if (dataEvents === 0) fail('streaming chat produced no SSE data events');
  if (!content.trim()) fail(`streaming chat produced no model content; raw saved at ${rawPath}`);
  const completedAt = doneAt || Date.now();
  const summary = {
    ok: true,
    model: modelId,
    prompt,
    max_tokens: maxTokens,
    content: content.trim(),
    content_chars: content.trim().length,
    data_events: dataEvents,
    json_events: jsonEvents,
    done_seen: doneSeen,
    timing_ms: {
      response_headers: responseHeadersAt === null ? null : responseHeadersAt - startedAt,
      first_chunk: firstChunkAt === null ? null : firstChunkAt - startedAt,
      first_data: firstDataAt === null ? null : firstDataAt - startedAt,
      first_json: firstJsonAt === null ? null : firstJsonAt - startedAt,
      first_content: firstContentAt === null ? null : firstContentAt - startedAt,
      total: completedAt - startedAt,
    },
    raw_path: path.relative(ROOT, rawPath),
  };
  writeJson(summaryPath, summary);
  return { ...summary, summary_path: path.relative(ROOT, summaryPath) };
}

async function runStreamingChatSmokeWithRetries(gatewayUrl, modelId, runDir, options) {
  const attempts = envPositiveInt('MAYHEM_PHASE3_CHAT_ATTEMPTS', 3);
  const retryDelayMs = envPositiveInt('MAYHEM_PHASE3_CHAT_RETRY_DELAY_SECONDS', 35) * 1000;
  let lastError = null;
  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    try {
      return await runStreamingChatSmoke(gatewayUrl, modelId, runDir, options);
    } catch (err) {
      lastError = err;
      if (attempt >= attempts) break;
      log(`streaming chat attempt ${attempt}/${attempts} failed before final accept: ${err?.message || err}`);
      await sleep(retryDelayMs);
    }
  }
  throw lastError;
}

async function readStateValue(rpcUrl, key) {
  const response = await fetch(`${rpcUrl.replace(/\/$/, '')}/state?key=${encodeURI(key)}`);
  if (!response.ok) {
    const text = await response.text();
    fail(`state read ${key} returned HTTP ${response.status}: ${text}`);
  }
  const body = await response.json();
  if (Object.prototype.hasOwnProperty.call(body, 'value')) return body.value;
  if (body.result && Object.prototype.hasOwnProperty.call(body.result, 'value')) return body.result.value;
  return body.result ?? body;
}

async function waitForLedgerMovement(rpcUrl, user, provider, before, expected, timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs;
  let latest = null;
  while (Date.now() < deadline) {
    latest = await readLedgerSnapshot(rpcUrl, user, provider);
    const actual = {
      debit_mu: before.balance_mu - latest.balance_mu,
      provider_net_mu: latest.provider_total_mu - before.provider_total_mu,
      fee_mu: latest.fee_cum_mu - before.fee_cum_mu,
    };
    if (
      actual.debit_mu === expected.debit_mu &&
      actual.provider_net_mu === expected.provider_net_mu &&
      actual.fee_mu === expected.fee_mu
    ) {
      return { after: latest, actual };
    }
    await sleep(500);
  }
  latest = latest || await readLedgerSnapshot(rpcUrl, user, provider);
  return {
    after: latest,
    actual: {
      debit_mu: before.balance_mu - latest.balance_mu,
      provider_net_mu: latest.provider_total_mu - before.provider_total_mu,
      fee_mu: latest.fee_cum_mu - before.fee_cum_mu,
    },
  };
}

function safeMu(value, label) {
  const number = Number(value ?? 0);
  if (!Number.isSafeInteger(number) || number < 0) fail(`${label} is not a non-negative safe integer: ${value}`);
  return number;
}

async function readDepositRootSnapshot(rpcUrl, epoch) {
  const value = await readStateValue(rpcUrl, `ev/dep/${epoch}`);
  if (!value) return null;
  if (value.type !== 'deposit_root') fail(`ev/dep/${epoch} is not a deposit root`);
  return {
    merkle_root: value.merkle_root,
    count: safeMu(value.count, `ev/dep/${epoch} count`),
    mu_total: safeMu(value.mu_total, `ev/dep/${epoch} mu_total`),
    source: `ev/dep/${epoch}`,
  };
}

async function readLedgerSnapshot(rpcUrl, user, provider) {
  const [balance, earning, fee] = await Promise.all([
    readStateValue(rpcUrl, `bal/${user}/fiat`),
    readStateValue(rpcUrl, `earn/fiat/${provider}`),
    readStateValue(rpcUrl, 'fee/fiat/cum'),
  ]);
  return {
    user,
    provider,
    rail: 'fiat',
    balance,
    earning,
    fee,
    balance_mu: safeMu(balance?.mu, 'fiat balance mu'),
    provider_total_mu: safeMu(earning?.total_mu, 'fiat provider total_mu'),
    fee_cum_mu: safeMu(fee?.cum_mu, 'fiat fee cum_mu'),
  };
}

function runJsonCommandToFile(command, args, outPath, options = {}) {
  const stdout = runSync(command, args, options);
  fs.writeFileSync(outPath, stdout);
  return readJson(outPath);
}

async function settleGatewayReceiptEpoch({
  mayhemBin,
  adminHome,
  adminRpcUrl,
  receiptsPath,
  runDir,
  user,
  provider,
  epoch = 1,
  feeBps = 1500,
}) {
  const before = await readLedgerSnapshot(adminRpcUrl, user, provider);
  const bundlePath = path.join(runDir, 'epoch-bundle.json');
  const exportPath = path.join(runDir, 'epoch-export.json');
  const recomputedPath = path.join(runDir, 'epoch-recomputed.json');
  const commitSimPath = path.join(runDir, 'epoch-commit-sim.json');
  const commitPath = path.join(runDir, 'epoch-commit.json');
  const applySimPath = path.join(runDir, 'epoch-apply-sim.json');
  const applyPath = path.join(runDir, 'epoch-apply.json');

  const exportReport = runJsonCommandToFile(mayhemBin, [
    'receipts', 'export',
    '--epoch', String(epoch),
    '--fee-bps', String(feeBps),
    '--receipts-file', receiptsPath,
    '--output', bundlePath,
    '--no-verify',
    '--json',
  ], exportPath);
  const bundle = readJson(bundlePath);
  const depositRoot = await readDepositRootSnapshot(adminRpcUrl, epoch);
  if (depositRoot) {
    bundle.deposit_root = depositRoot;
    fs.writeFileSync(bundlePath, `${JSON.stringify(bundle, null, 2)}\n`);
  }

  const recomputedStdout = runSync('node', [path.join(ROOT, 'intercom/scripts/recompute-epoch-roots.mjs'), bundlePath]);
  fs.writeFileSync(recomputedPath, recomputedStdout);
  const recomputed = readJson(recomputedPath);
  if (safeMu(recomputed?.totals?.use_mu, 'recomputed use_mu') <= 0) {
    fail('epoch recompute produced zero usage; refusing to mark billing moved');
  }
  const providerEarning = (recomputed.earnings || []).find(
    (entry) => entry.rail === 'fiat' && entry.provider === provider
  );
  if (!providerEarning) fail(`epoch recompute did not include fiat earnings for provider ${provider}`);
  const expectedDebitMu = safeMu(recomputed.totals.use_mu, 'expected debit mu');
  const expectedProviderNetMu =
    safeMu(providerEarning.gross_mu, 'provider gross_mu') -
    Math.floor((safeMu(providerEarning.gross_mu, 'provider gross_mu') * feeBps) / 10_000);
  const expectedFeeMu = safeMu(recomputed.totals.fee_mu, 'expected fee_mu');

  const adminEpochCommon = [
    '--home', adminHome,
    '--peer-store-name', 'admin',
    '--rpc-url', adminRpcUrl,
    '--recomputed-file', recomputedPath,
    '--at', String(epoch),
    '--submit',
    '--json',
  ];
  const commitSim = runJsonCommandToFile(
    mayhemBin,
    ['admin', 'epoch-commit', ...adminEpochCommon, '--sim'],
    commitSimPath
  );
  const commit = runJsonCommandToFile(mayhemBin, ['admin', 'epoch-commit', ...adminEpochCommon], commitPath);
  const applySim = runJsonCommandToFile(
    mayhemBin,
    ['admin', 'epoch-apply', ...adminEpochCommon, '--sim'],
    applySimPath
  );
  const apply = runJsonCommandToFile(mayhemBin, ['admin', 'epoch-apply', ...adminEpochCommon], applyPath);

  const expected = {
    debit_mu: expectedDebitMu,
    provider_net_mu: expectedProviderNetMu,
    fee_mu: expectedFeeMu,
  };
  const { after, actual } = await waitForLedgerMovement(adminRpcUrl, user, provider, before, expected);
  const actualDebitMu = actual.debit_mu;
  const actualProviderNetMu = actual.provider_net_mu;
  const actualFeeMu = actual.fee_mu;
  if (actualDebitMu !== expectedDebitMu) {
    fail(`fiat balance debit mismatch; expected ${expectedDebitMu}, got ${actualDebitMu}`);
  }
  if (actualProviderNetMu !== expectedProviderNetMu) {
    fail(`fiat provider earning mismatch; expected ${expectedProviderNetMu}, got ${actualProviderNetMu}`);
  }
  if (actualFeeMu !== expectedFeeMu) {
    fail(`fiat fee movement mismatch; expected ${expectedFeeMu}, got ${actualFeeMu}`);
  }

  return {
    ok: true,
    epoch,
    fee_bps: feeBps,
    before,
    after,
    expected,
    actual,
    files: {
      bundle: path.relative(ROOT, bundlePath),
      export_report: path.relative(ROOT, exportPath),
      recomputed: path.relative(ROOT, recomputedPath),
      commit_sim: path.relative(ROOT, commitSimPath),
      commit: path.relative(ROOT, commitPath),
      apply_sim: path.relative(ROOT, applySimPath),
      apply: path.relative(ROOT, applyPath),
    },
    reports: {
      export: exportReport,
      commit_sim: commitSim,
      commit,
      apply_sim: applySim,
      apply,
    },
  };
}

async function preauthorizeLocalApps(apps, logFile) {
  if (process.platform !== 'darwin') return;
  const socketfilterfw = '/usr/libexec/ApplicationFirewall/socketfilterfw';
  if (!fs.existsSync(socketfilterfw)) return;
  const lines = [];
  for (const app of apps) {
    if (!app || !fs.existsSync(app)) continue;
    for (const action of ['--add', '--unblockapp']) {
      try {
        const result = spawnSync(socketfilterfw, [action, app], {
          cwd: ROOT,
          encoding: 'utf8',
          maxBuffer: 1024 * 1024,
        });
        lines.push(`${action} ${app} -> ${result.status ?? result.signal}`);
        if (result.stdout) lines.push(result.stdout.trim());
        if (result.stderr) lines.push(result.stderr.trim());
      } catch (err) {
        lines.push(`${action} ${app} -> ${err.message}`);
      }
    }
  }
  if (lines.length) await fsp.appendFile(logFile, `${lines.filter(Boolean).join('\n')}\n`);
}

function sha256File(file) {
  const hash = crypto.createHash('sha256');
  const fd = fs.openSync(file, 'r');
  const buffer = Buffer.allocUnsafe(1024 * 1024);
  try {
    for (;;) {
      const read = fs.readSync(fd, buffer, 0, buffer.length, null);
      if (read === 0) break;
      hash.update(buffer.subarray(0, read));
    }
  } finally {
    fs.closeSync(fd);
  }
  return hash.digest('hex');
}

async function b3(buffer) {
  return Buffer.from(await blake3(buffer));
}

function u64le(value) {
  const out = Buffer.alloc(8);
  out.writeBigUInt64LE(BigInt(value));
  return out;
}

function localPeerDhtHost(remoteHost) {
  const override = (process.env.MAYHEM_PHASE3_LOCAL_DHT_HOST || '').trim();
  if (override) return override;
  const remoteOctets = /^(\d+)\.(\d+)\.(\d+)\.\d+$/.exec(remoteHost || '');
  const addresses = [];
  for (const [name, entries] of Object.entries(os.networkInterfaces())) {
    for (const entry of entries || []) {
      if (entry.family !== 'IPv4' || entry.internal) continue;
      if (entry.address.startsWith('169.254.')) continue;
      addresses.push({ name, address: entry.address });
    }
  }
  if (remoteOctets) {
    const prefix = `${remoteOctets[1]}.${remoteOctets[2]}.${remoteOctets[3]}.`;
    const sameLan = addresses.find((entry) => entry.address.startsWith(prefix));
    if (sameLan) return sameLan.address;
  }
  return (
    addresses.find((entry) => entry.name === 'en0')?.address ||
    addresses.find((entry) => !entry.address.startsWith('10.'))?.address ||
    addresses[0]?.address ||
    '127.0.0.1'
  );
}

async function merkleLeaf(index, len, data) {
  return b3(Buffer.concat([
    Buffer.from('mayhem-blake3-merkle-v1:leaf'),
    u64le(index),
    u64le(len),
    data,
  ]));
}

async function merkleParent(left, right) {
  return b3(Buffer.concat([
    Buffer.from('mayhem-blake3-merkle-v1:node'),
    left,
    right,
  ]));
}

async function merkleRoot(file, chunkSize = CHUNK_SIZE) {
  const handle = await fsp.open(file, 'r');
  const buffer = Buffer.allocUnsafe(chunkSize);
  const leaves = [];
  let totalBytes = 0;
  let index = 0;
  try {
    for (;;) {
      const { bytesRead } = await handle.read(buffer, 0, chunkSize, null);
      if (bytesRead === 0) break;
      totalBytes += bytesRead;
      leaves.push(await merkleLeaf(index, bytesRead, buffer.subarray(0, bytesRead)));
      index += 1;
    }
  } finally {
    await handle.close();
  }
  if (leaves.length === 0) {
    return {
      root: (await b3(Buffer.from('mayhem-blake3-merkle-v1:empty'))).toString('hex'),
      total_bytes: 0,
      chunks: 0,
    };
  }
  let layer = leaves;
  while (layer.length > 1) {
    const next = [];
    for (let i = 0; i < layer.length; i += 2) {
      const left = layer[i];
      const right = layer[i + 1] || left;
      next.push(await merkleParent(left, right));
    }
    layer = next;
  }
  return {
    root: layer[0].toString('hex'),
    total_bytes: totalBytes,
    chunks: leaves.length,
  };
}

async function catalogEnclaveId({ adminPubkey, modelId, artifactRoot, manifestHash, binaryHash }) {
  return (await b3(Buffer.from(`${adminPubkey}${modelId}${artifactRoot}${manifestHash}${binaryHash}`, 'utf8'))).toString('hex');
}

function runSync(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: options.cwd || ROOT,
    env: options.env || process.env,
    encoding: 'utf8',
    maxBuffer: options.maxBuffer || 64 * 1024 * 1024,
  });
  if (result.status !== 0) {
    const stderr = result.stderr?.trim();
    const stdout = result.stdout?.trim();
    fail(`${command} ${args.join(' ')} failed${stderr ? `\n${stderr}` : ''}${stdout ? `\n${stdout}` : ''}`);
  }
  return result.stdout;
}

async function run(command, args, options = {}) {
  const logFile = options.logFile;
  const output = [];
  const errput = [];
  const outStream = logFile ? fs.createWriteStream(logFile, { flags: 'a' }) : null;
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd: options.cwd || ROOT,
      env: options.env || process.env,
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    children.push(child);
    const timer = options.timeoutMs
      ? setTimeout(() => {
          child.kill('SIGTERM');
          reject(new Error(`${command} timed out after ${options.timeoutMs}ms`));
        }, options.timeoutMs)
      : null;
    child.stdout.on('data', (data) => {
      output.push(data);
      outStream?.write(data);
      if (options.echo) process.stderr.write(data);
    });
    child.stderr.on('data', (data) => {
      errput.push(data);
      outStream?.write(data);
      if (options.echo) process.stderr.write(data);
    });
    child.on('error', (err) => {
      if (timer) clearTimeout(timer);
      outStream?.end();
      reject(err);
    });
    child.on('close', (code, signal) => {
      if (timer) clearTimeout(timer);
      outStream?.end();
      const stdout = Buffer.concat(output).toString('utf8');
      const stderr = Buffer.concat(errput).toString('utf8');
      if (code !== 0) {
        reject(new Error(`${command} exited ${code ?? signal}${stderr.trim() ? `\n${stderr.trim()}` : ''}`));
        return;
      }
      resolve({ stdout, stderr });
    });
  });
}

function spawnLogged(command, args, logFile, options = {}) {
  const out = fs.createWriteStream(logFile, { flags: 'a' });
  const child = spawn(command, args, {
    cwd: options.cwd || ROOT,
    env: options.env || process.env,
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  children.push(child);
  child.stdout.pipe(out);
  child.stderr.pipe(out);
  return child;
}

function normalizeWsData(data) {
  if (typeof data === 'string') return data;
  if (data instanceof ArrayBuffer) return Buffer.from(data).toString('utf8');
  if (ArrayBuffer.isView(data)) return Buffer.from(data.buffer, data.byteOffset, data.byteLength).toString('utf8');
  return String(data);
}

async function bridgeRequest(url, token, payload, timeoutMs = 10_000) {
  const socket = new WebSocket(url);
  let authed = false;
  return new Promise((resolve, reject) => {
    let settled = false;
    const timer = setTimeout(() => {
      finish(new Error(`${payload.type} timed out`));
    }, timeoutMs);
    const finish = (err, value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      try { socket.close(); } catch {}
      if (err) reject(err);
      else resolve(value);
    };
    socket.addEventListener('open', () => {
      socket.send(JSON.stringify({ id: 1, type: 'auth', token }));
    });
    socket.addEventListener('error', () => finish(new Error('SC-Bridge websocket error')));
    socket.addEventListener('message', (event) => {
      let message;
      try {
        message = JSON.parse(normalizeWsData(event.data));
      } catch (err) {
        finish(new Error(`SC-Bridge returned invalid JSON: ${err.message}`));
        return;
      }
      if (!authed) {
        if (message.id === 1 && message.type === 'auth_ok') {
          authed = true;
          socket.send(JSON.stringify({ id: 2, ...payload }));
          return;
        }
        if (message.id === 1 && message.type === 'error') {
          finish(new Error(message.error || 'SC-Bridge auth failed'));
        }
        return;
      }
      if (message.id !== 2) return;
      if (message.type === 'error') {
        finish(new Error(message.error || `${payload.type} failed`));
        return;
      }
      finish(null, message);
    });
  });
}

function bridgeStatsContainsPeer(stats, peer) {
  const normalized = String(peer || '').toLowerCase();
  if (!normalized) return false;
  const peers = Array.isArray(stats?.peers) ? stats.peers : [];
  const swarmPeers = Array.isArray(stats?.swarmPeers) ? stats.swarmPeers : [];
  return [...peers, ...swarmPeers].some((value) => String(value).toLowerCase() === normalized);
}

async function waitBridgePeerVisible(url, token, provider, timeoutMs, label = 'bridge') {
  const deadline = Date.now() + timeoutMs;
  let attempts = 0;
  let lastError = null;
  let lastStats = null;
  while (Date.now() < deadline) {
    attempts += 1;
    try {
      const stats = await bridgeRequest(url, token, { type: 'stats' }, 5_000);
      lastStats = stats;
      if (bridgeStatsContainsPeer(stats, provider)) {
        return { label, url, attempts, stats };
      }
      lastError = new Error(`provider not in peer stats: ${JSON.stringify(stats)}`);
    } catch (err) {
      lastError = err;
    }
    await sleep(2_000);
  }
  throw new Error(`${label} did not see provider ${provider} after ${attempts} stats polls: ${lastError?.message || 'unknown error'}${lastStats ? `; last_stats=${JSON.stringify(lastStats)}` : ''}`);
}

async function waitBridgeSessionOpen(url, token, provider, timeoutMs, label = 'bridge') {
  const deadline = Date.now() + timeoutMs;
  let attempts = 0;
  let lastError = null;
  while (Date.now() < deadline) {
    attempts += 1;
    const sessionId = crypto.randomBytes(32).toString('hex');
    const attemptTimeout = Math.max(1_000, deadline - Date.now());
    try {
      await bridgeRequest(
        url,
        token,
        { type: 'peer_connect', remote: provider, wait_ms: Math.min(attemptTimeout, 15_000) },
        Math.min(attemptTimeout, 20_000)
      );
      const opened = await bridgeRequest(
        url,
        token,
        { type: 'session_open', remote: provider, session_id: sessionId },
        Math.min(attemptTimeout, 10_000)
      );
      await bridgeRequest(
        url,
        token,
        { type: 'session_close', remote: provider, session_id: sessionId },
        5_000
      ).catch(() => {});
      return { label, url, attempts, session_id: sessionId, opened };
    } catch (err) {
      lastError = err;
    }
    await sleep(2_000);
  }
  throw new Error(`${label} could not open a direct session to ${provider} after ${attempts} attempts: ${lastError?.message || 'unknown error'}`);
}

async function pickDirectBridge(candidates, token, provider, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  const errors = [];
  while (Date.now() < deadline) {
    for (const candidate of candidates) {
      try {
        const ready = await waitBridgeSessionOpen(candidate.url, token, provider, 60_000, candidate.label);
        return { ...candidate, ready };
      } catch (err) {
        errors.push(`${candidate.label}: ${err?.message || err}`);
      }
    }
  }
  throw new Error(
    `no local SC-Bridge saw provider ${provider}; last errors: ${errors.slice(-candidates.length).join(' | ')}`
  );
}

async function waitForFilePattern(file, pattern, timeoutMs, label, processCheck) {
  const deadline = Date.now() + timeoutMs;
  let last = '';
  while (Date.now() < deadline) {
    if (fs.existsSync(file)) {
      last = fs.readFileSync(file, 'utf8');
      if (pattern.test(last)) return last;
    }
    if (processCheck && !(await processCheck())) {
      fail(`${label} exited before readiness. Log: ${file}`);
    }
    await sleep(500);
  }
  fail(`timed out waiting for ${label}; last log tail:\n${last.split('\n').slice(-40).join('\n')}`);
}

async function waitHttp(url, timeoutMs, label, processCheck) {
  const deadline = Date.now() + timeoutMs;
  let lastErr = '';
  while (Date.now() < deadline) {
    try {
      const response = await fetch(url);
      if (response.ok) return;
      lastErr = `${response.status} ${response.statusText}`;
    } catch (err) {
      lastErr = err.message;
    }
    if (processCheck && !(await processCheck())) {
      fail(`${label} exited before HTTP readiness (${lastErr})`);
    }
    await sleep(500);
  }
  fail(`timed out waiting for ${label} at ${url}: ${lastErr}`);
}

async function freePort() {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address();
      server.close(() => resolve(port));
    });
    server.on('error', reject);
  });
}

function parseMacmini(file) {
  const text = fs.readFileSync(file, 'utf8');
  const lines = text.split(/\r?\n/).filter(Boolean);
  if (lines.length < 3) fail(`${file} must contain user/password/host lines`);
  const value = (line) => line.replace(/^[^:]*:\s*/, '').trim();
  return {
    user: value(lines[0]),
    pass: value(lines[1]),
    host: value(lines[2]),
  };
}

function sshBase(passFile, remote) {
  return ['-f', passFile, 'ssh', ...SSH_OPTS, `${remote.user}@${remote.host}`];
}

async function ssh(remote, passFile, command, options = {}) {
  return run('sshpass', [...sshBase(passFile, remote), command], options);
}

async function scpTo(remote, passFile, localPath, remotePath, options = {}) {
  const attempts = Number.isSafeInteger(options.attempts) ? options.attempts : 3;
  let lastError = null;
  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    try {
      return await rsyncTo(remote, passFile, localPath, remotePath, {
        ...options,
        timeoutMs: Math.max(options.timeoutMs || 0, 180_000),
      });
    } catch (err) {
      lastError = err;
      if (attempt >= attempts) break;
      log(
        `copy retry ${attempt}/${attempts} for ${path.basename(localPath)}: ` +
          `${err?.message || err}`
      );
      await sleep(1_000 * attempt);
    }
  }
  log(`copy fallback via ssh stream for ${path.basename(localPath)}: ${lastError?.message || lastError}`);
  return sshStreamTo(remote, passFile, localPath, remotePath, {
    ...options,
    timeoutMs: Math.max(options.timeoutMs || 0, 180_000),
  });
}

async function rsyncTo(remote, passFile, localPath, remotePath, options = {}) {
  const rsh = ['sshpass', '-f', passFile, 'ssh', ...SSH_OPTS].map(sh).join(' ');
  return run(
    'rsync',
    ['-a', '--partial', '--inplace', '-e', rsh, localPath, `${remote.user}@${remote.host}:${remotePath}`],
    options
  );
}

async function sshStreamTo(remote, passFile, localPath, remotePath, options = {}) {
  const tmpPath = `${remotePath}.tmp-${process.pid}-${Date.now()}`;
  const remoteCommand = `cat > ${sh(tmpPath)} && mv ${sh(tmpPath)} ${sh(remotePath)}`;
  const outStream = options.logFile ? fs.createWriteStream(options.logFile, { flags: 'a' }) : null;
  const output = [];
  const errput = [];
  return new Promise((resolve, reject) => {
    const child = spawn(
      'sshpass',
      ['-f', passFile, 'ssh', ...SSH_OPTS, `${remote.user}@${remote.host}`, remoteCommand],
      {
        cwd: ROOT,
        env: options.env || process.env,
        stdio: ['pipe', 'pipe', 'pipe'],
      }
    );
    children.push(child);
    const input = fs.createReadStream(localPath);
    const timer = options.timeoutMs
      ? setTimeout(() => {
          child.kill('SIGTERM');
          input.destroy();
          reject(new Error(`ssh stream timed out after ${options.timeoutMs}ms`));
        }, options.timeoutMs)
      : null;
    input.on('error', (err) => {
      child.kill('SIGTERM');
      reject(err);
    });
    child.stdout.on('data', (data) => {
      output.push(data);
      outStream?.write(data);
      if (options.echo) process.stderr.write(data);
    });
    child.stderr.on('data', (data) => {
      errput.push(data);
      outStream?.write(data);
      if (options.echo) process.stderr.write(data);
    });
    child.on('error', (err) => {
      if (timer) clearTimeout(timer);
      input.destroy();
      outStream?.end();
      reject(err);
    });
    child.on('close', (code, signal) => {
      if (timer) clearTimeout(timer);
      outStream?.end();
      const stdout = Buffer.concat(output).toString('utf8');
      const stderr = Buffer.concat(errput).toString('utf8');
      if (code !== 0) {
        reject(new Error(`ssh stream exited ${code ?? signal}${stderr.trim() ? `\n${stderr.trim()}` : ''}`));
        return;
      }
      resolve({ stdout, stderr });
    });
    input.pipe(child.stdin);
  });
}

async function remoteSha256(remote, passFile, remotePath) {
  const result = await ssh(
    remote,
    passFile,
    `test -f ${sh(remotePath)} && shasum -a 256 ${sh(remotePath)} | awk '{print $1}' || true`
  );
  return result.stdout.trim();
}

async function remotePidAlive(remote, passFile, pid) {
  const result = await ssh(remote, passFile, `kill -0 ${Number(pid)} >/dev/null 2>&1 && echo alive || true`);
  return result.stdout.trim() === 'alive';
}

async function remoteFreePort(remote, passFile) {
  const script = [
    'import socket',
    's = socket.socket()',
    's.bind(("127.0.0.1", 0))',
    'print(s.getsockname()[1])',
    's.close()',
  ].join('\n');
  const result = await ssh(remote, passFile, `/usr/bin/python3 -c ${sh(script)}`);
  const port = Number.parseInt(result.stdout.trim(), 10);
  if (!Number.isSafeInteger(port) || port <= 0) fail(`remote did not return a free port: ${result.stdout}`);
  return port;
}

async function waitReverseTunnel(remote, passFile, url, tunnel, tunnelLog, label, timeoutMs = 15_000) {
  const deadline = Date.now() + timeoutMs;
  let lastErr = '';
  const script = [
    'import sys, urllib.request',
    `urllib.request.urlopen(${JSON.stringify(url)}, timeout=2).read()`,
  ].join('\n');
  while (Date.now() < deadline) {
    if (tunnel.exitCode !== null) {
      fail(`${label} reverse tunnel exited early; see ${tunnelLog}`);
    }
    try {
      await ssh(remote, passFile, `/usr/bin/python3 -c ${sh(script)}`, { timeoutMs: 10_000 });
      return;
    } catch (err) {
      lastErr = err?.message || String(err);
      await sleep(500);
    }
  }
  fail(`${label} reverse tunnel did not answer ${url}: ${lastErr}; see ${tunnelLog}`);
}

async function main() {
  if (process.argv.includes('--help') || process.argv.includes('-h')) {
    usage();
    return;
  }

  const mode = (process.env.MAYHEM_PHASE3_PROVIDER_MODE || 'real').trim();
  if (!['real', 'shim'].includes(mode)) fail('MAYHEM_PHASE3_PROVIDER_MODE must be real or shim');

  const tag = new Date().toISOString().replace(/[-:]/g, '').replace(/\..*/, '');
  const runDir = path.join(ROOT, '.mayhem-local/p3.6-phase3', tag);
  const logsDir = path.join(runDir, 'logs');
  const subnetChannel = `mayhem-p36-${tag}`;
  await fsp.mkdir(logsDir, { recursive: true });

  const mayhemBin = path.join(ROOT, 'target/debug/mayhem');
  const mayhemEnclaveBin = path.join(ROOT, 'target/debug/mayhem-enclave');
  const modelId = process.env.MAYHEM_PHASE3_MODEL || DEFAULT_MODEL;
  let artifactPath = path.resolve(process.env.MAYHEM_PHASE3_ARTIFACT || DEFAULT_ARTIFACT);
  if (mode === 'shim' && !process.env.MAYHEM_PHASE3_ARTIFACT) {
    artifactPath = path.join(runDir, 'phase3-shim-artifact.bin');
    fs.writeFileSync(
      artifactPath,
      `mayhem phase3 deterministic session artifact\n${tag}\n`,
      { mode: 0o644 }
    );
  }
  const opencodeBin = process.env.MAYHEM_PHASE3_OPENCODE_BIN || 'opencode';
  const macminiFile = path.resolve(ROOT, process.env.MAYHEM_PHASE3_MACMINI_FILE || '../gpd/macmini.txt');

  if (!fs.existsSync(artifactPath)) fail(`missing artifact ${artifactPath}`);

  log(`run dir: ${path.relative(ROOT, runDir)}`);
  log(`provider mode: ${mode}`);
  log('building mayhem CLI/gateway/enclave crates');
  const cargoEnv = { ...process.env };
  if (mode === 'real') {
    cargoEnv.CARGO_PROFILE_DEV_OPT_LEVEL =
      cargoEnv.MAYHEM_PHASE3_CARGO_OPT_LEVEL ||
      cargoEnv.CARGO_PROFILE_DEV_OPT_LEVEL ||
      '3';
    cargoEnv.CARGO_PROFILE_DEV_DEBUG = cargoEnv.CARGO_PROFILE_DEV_DEBUG || '0';
    log(`using Cargo dev opt-level ${cargoEnv.CARGO_PROFILE_DEV_OPT_LEVEL} for real-provider hashing/generation`);
  }
  runSync('cargo', ['build', '-q', '-p', 'mayhem-cli', '-p', 'mayhem-gateway', '-p', 'mayhem-enclave'], {
    env: cargoEnv,
  });
  if (!fs.existsSync(mayhemBin)) fail(`missing ${mayhemBin} after cargo build`);
  if (!fs.existsSync(mayhemEnclaveBin)) fail(`missing ${mayhemEnclaveBin} after cargo build`);

  log('computing artifact Merkle root and binary measurement');
  const artifactSha256 = sha256File(artifactPath);
  const artifactMerkle = await merkleRoot(artifactPath);
  const binaryHashOut = runSync(mayhemEnclaveBin, ['measure-binary', '--binary', mayhemBin]);
  const binaryHash = binaryHashOut.trim().replace(/^binary_hash=/, '');
  const binarySha256 = sha256File(mayhemBin);

  const catalog = readJson(path.join(ROOT, 'catalog/models.json'));
  const model = catalog.models.find((entry) => entry.model_id === modelId);
  if (!model) fail(`catalog model not found: ${modelId}`);
  const inPer1kMu = model.price_ref_mu?.in_per_1k || 18;
  const outPer1kMu = model.price_ref_mu?.out_per_1k || 55;
  const rateMapJson = textRateMapJson(inPer1kMu, outPer1kMu);
  const artifactEntry = Object.entries(model.artifacts).find(([, artifact]) => artifact.engine === 'llama.cpp');
  if (!artifactEntry) fail(`model ${modelId} has no llama.cpp artifact`);
  const [artifactName, artifact] = artifactEntry;
  artifact.artifact_root = artifactMerkle.root;
  artifact.artifact_root_kind = 'blake3_merkle_v1';
  artifact.weights_bytes = artifactMerkle.total_bytes;
  artifact.source_sha256 = artifactSha256;

  const tempCatalogDir = path.join(runDir, 'catalog');
  await fsp.mkdir(tempCatalogDir, { recursive: true });
  const tempCatalogPath = path.join(tempCatalogDir, 'models.json');
  writeJson(tempCatalogPath, catalog);
  const manifestHash = sha256File(tempCatalogPath);

  const remote = parseMacmini(macminiFile);
  const localTmpDir = path.join(ROOT, '.mayhem-local/tmp');
  await fsp.mkdir(localTmpDir, { recursive: true });
  const passFile = path.join(localTmpDir, `macmini-${tag}-${process.pid}.pass`);
  fs.writeFileSync(passFile, remote.pass, { mode: 0o600 });
  sensitiveFiles.push(passFile);
  cleanupState.remote = remote;
  cleanupState.passFile = passFile;
  await fsp.mkdir(path.dirname(SSH_OPTS[SSH_OPTS.length - 1].split('=')[1]), { recursive: true });

  log('checking Mac mini SSH and remote Pear runtime');
  const remoteHome = (await ssh(remote, passFile, 'printf "%s\\n" "$HOME"')).stdout.trim();
  const remoteRootEnv = process.env.MAYHEM_PHASE3_REMOTE_ROOT || '~/mayhem-macmini-p33';
  const remoteRoot = remoteRootEnv.startsWith('~/')
    ? path.posix.join(remoteHome, remoteRootEnv.slice(2))
    : remoteRootEnv.replace('$HOME', remoteHome);
  const remoteRun = path.posix.join(remoteRoot, '.mayhem-local/p3.6-phase3', tag);
  const remoteLogs = path.posix.join(remoteRun, 'logs');
  cleanupState.remoteRun = remoteRun;
  const remoteCatalog = path.posix.join(remoteRun, 'catalog/models.json');
  const remoteRules = path.posix.join(remoteRun, 'RULES.md');
  const remoteArtifact = path.posix.join(remoteRoot, '.mayhem-cache/artifacts', path.basename(artifactPath));
  const remoteMayhem = path.posix.join(remoteRoot, 'target/debug/mayhem');
  const remoteWalletHelper = path.posix.join(remoteRoot, 'crates/mayhem-cli/src/wallet-helper.mjs');
  const remoteIntercomMain = path.posix.join(remoteRoot, 'intercom/src/main.js');
  const remoteScBridgeFeature = path.posix.join(remoteRoot, 'intercom/features/sc-bridge/index.js');
  const remoteDirectSessionFeature = path.posix.join(remoteRoot, 'intercom/features/direct-session/index.js');
  const remotePear = path.posix.join(remoteHome, 'Library/Application Support/pear/current/by-arch/darwin-arm64/bin/pear-runtime');
  const remoteNode = (await ssh(
    remote,
    passFile,
    [
      'for p in',
      `${sh(path.posix.join(remoteHome, 'local/node-v22.22.0-darwin-arm64/bin/node'))}`,
      `${sh(path.posix.join(remoteHome, '.hermes/node/bin/node'))}`,
      '/opt/homebrew/bin/node',
      '/usr/local/bin/node',
      '; do test -x "$p" && printf "%s\\n" "$p" && exit 0; done',
      '; command -v node || true',
    ].join(' ')
  )).stdout.trim().split(/\r?\n/).find(Boolean);
  if (!remoteNode) fail('remote Mac mini has no usable node binary for wallet-helper.mjs');

  await ssh(
    remote,
    passFile,
    [
      `test -x ${sh(remotePear)}`,
      `test -d ${sh(path.posix.join(remoteRoot, 'intercom/node_modules'))}`,
      `mkdir -p ${sh(path.posix.join(remoteRoot, 'target/debug'))} ${sh(path.posix.dirname(remoteArtifact))} ${sh(path.posix.dirname(remoteCatalog))} ${sh(path.posix.dirname(remoteWalletHelper))} ${sh(path.posix.dirname(remoteIntercomMain))} ${sh(path.posix.dirname(remoteScBridgeFeature))} ${sh(path.posix.dirname(remoteDirectSessionFeature))} ${sh(remoteLogs)}`,
    ].join(' && ')
  );

  const remoteBinarySha = await remoteSha256(remote, passFile, remoteMayhem);
  if (remoteBinarySha !== binarySha256) {
    log('copying mayhem binary to Mac mini');
    await scpTo(remote, passFile, mayhemBin, remoteMayhem, {
      logFile: path.join(logsDir, 'scp-mayhem.log'),
      timeoutMs: 180_000,
    });
    await ssh(remote, passFile, `chmod +x ${sh(remoteMayhem)}`);
  }
  await ssh(
    remote,
    passFile,
    [
      'if [ -x /usr/libexec/ApplicationFirewall/socketfilterfw ]; then',
      `for app in ${[remotePear, remoteNode, remoteMayhem].map(sh).join(' ')}; do`,
      'test -e "$app" || continue;',
      '/usr/libexec/ApplicationFirewall/socketfilterfw --add "$app" >/dev/null 2>&1 || true;',
      '/usr/libexec/ApplicationFirewall/socketfilterfw --unblockapp "$app" >/dev/null 2>&1 || true;',
      'done;',
      'fi',
    ].join(' '),
    { logFile: path.join(logsDir, 'remote-firewall.log') }
  );

  log('copying temporary catalog to Mac mini');
  await scpTo(remote, passFile, tempCatalogPath, remoteCatalog, {
    logFile: path.join(logsDir, 'scp-catalog.log'),
    timeoutMs: 60_000,
  });
  await scpTo(remote, passFile, path.join(ROOT, 'RULES.md'), remoteRules, {
    logFile: path.join(logsDir, 'scp-rules.log'),
    timeoutMs: 60_000,
  });
  await scpTo(remote, passFile, path.join(ROOT, 'crates/mayhem-cli/src/wallet-helper.mjs'), remoteWalletHelper, {
    logFile: path.join(logsDir, 'scp-wallet-helper.log'),
    timeoutMs: 60_000,
  });
  await scpTo(remote, passFile, path.join(ROOT, 'intercom/src/main.js'), remoteIntercomMain, {
    logFile: path.join(logsDir, 'scp-intercom-main.log'),
    timeoutMs: 60_000,
  });
  await scpTo(remote, passFile, path.join(ROOT, 'intercom/features/sc-bridge/index.js'), remoteScBridgeFeature, {
    logFile: path.join(logsDir, 'scp-sc-bridge.log'),
    timeoutMs: 60_000,
  });
  await scpTo(remote, passFile, path.join(ROOT, 'intercom/features/direct-session/index.js'), remoteDirectSessionFeature, {
    logFile: path.join(logsDir, 'scp-direct-session.log'),
    timeoutMs: 60_000,
  });

  const remoteArtifactSha = await remoteSha256(remote, passFile, remoteArtifact);
  if (remoteArtifactSha !== artifactSha256) {
    log('copying model artifact to Mac mini; this can take a while');
    await rsyncTo(remote, passFile, artifactPath, remoteArtifact, {
      logFile: path.join(logsDir, 'rsync-artifact.log'),
      timeoutMs: 30 * 60_000,
    });
  } else {
    log('Mac mini already has matching model artifact');
  }

  const localNode = process.env.MAYHEM_PHASE3_LOCAL_NODE_BIN || process.execPath;
  const localPear = path.join(
    os.homedir(),
    'Library/Application Support/pear/current/by-arch/darwin-arm64/bin/pear-runtime'
  );
  await preauthorizeLocalApps([localNode, localPear], path.join(logsDir, 'local-firewall.log'));
  const explicitPeerDhtBootstrap = (process.env.MAYHEM_PHASE3_PEER_DHT_BOOTSTRAP || '').trim();
  const useLocalDht = /^(1|true|yes)$/i.test(process.env.MAYHEM_PHASE3_USE_LOCAL_DHT || '');
  const usePublicDht = /^(1|true|yes)$/i.test(process.env.MAYHEM_PHASE3_USE_PUBLIC_DHT || '');
  const useRemoteDht = !usePublicDht && /^(1|true|yes)?$/i.test(process.env.MAYHEM_PHASE3_USE_REMOTE_DHT || '1');
  let localPeerDht = null;
  let remotePeerDhtPid = null;
  let peerDhtBootstrap = explicitPeerDhtBootstrap;
  if (!peerDhtBootstrap && useLocalDht) {
    const peerDhtPort = await freePort();
    const peerDhtHost = localPeerDhtHost(remote.host);
    peerDhtBootstrap = `${peerDhtHost}:${peerDhtPort}`;
    const localPeerDhtLog = path.join(logsDir, 'local-peer-dht.log');
    log(`starting local peer DHT bootstrap at ${peerDhtBootstrap}`);
    localPeerDht = spawnLogged(
      localNode,
      [
        path.join(ROOT, 'intercom/node_modules/hyperdht/bin.js'),
        '--bootstrap',
        '--host', peerDhtHost,
        '--port', String(peerDhtPort),
      ],
      localPeerDhtLog
    );
    cleanupState.localChildren.push(localPeerDht);
    await waitForFilePattern(
      localPeerDhtLog,
      /Fully started Hyperswarm DHT bootstrap node/,
      30_000,
      'local peer DHT bootstrap',
      async () => localPeerDht.exitCode === null
    );
  } else if (!peerDhtBootstrap && useRemoteDht) {
    const remotePeerDhtPort = await remoteFreePort(remote, passFile);
    const remotePeerDhtLog = path.posix.join(remoteLogs, 'peer-dht.log');
    peerDhtBootstrap = `${remote.host}:${remotePeerDhtPort}`;
    log(`starting Mac mini peer DHT bootstrap at ${peerDhtBootstrap}`);
    const remotePeerDhtCmd = [
      `cd ${sh(path.posix.join(remoteRoot, 'intercom'))}`,
      `nohup ${sh(remoteNode)} ${sh(path.posix.join(remoteRoot, 'intercom/node_modules/hyperdht/bin.js'))} --bootstrap --host ${sh(remote.host)} --port ${remotePeerDhtPort} > ${sh(remotePeerDhtLog)} 2>&1 & echo $!`,
    ].join(' && ');
    remotePeerDhtPid = (await ssh(remote, passFile, remotePeerDhtCmd)).stdout.trim();
    cleanupState.remotePids.push(remotePeerDhtPid);
    const remotePeerDhtWait = `for i in $(seq 1 120); do grep -q 'Fully started Hyperswarm DHT bootstrap node' ${sh(remotePeerDhtLog)} && exit 0; kill -0 ${remotePeerDhtPid} >/dev/null 2>&1 || exit 2; sleep 0.5; done; exit 1`;
    await ssh(remote, passFile, remotePeerDhtWait, { timeoutMs: 75_000 });
  } else if (peerDhtBootstrap) {
    log(`using explicit peer DHT bootstrap ${peerDhtBootstrap}`);
  } else {
    log('using default peer DHT bootstrap');
  }

  log('starting local Pear dev-net');
  const devnetLog = path.join(logsDir, 'dev-net.log');
  const devnet = spawnLogged(
    'bash',
    ['scripts/dev-net.sh', '--cleanup', '--keep-running'],
    devnetLog,
    {
      env: {
        ...process.env,
        MAYHEM_DEVNET_JOINERS: process.env.MAYHEM_PHASE3_LOCAL_JOINERS || '2',
        MAYHEM_DEVNET_SUBNET_CHANNEL: subnetChannel,
        ...(peerDhtBootstrap ? { MAYHEM_DEVNET_PEER_DHT_BOOTSTRAP: peerDhtBootstrap } : {}),
        MAYHEM_DEVNET_REPLICATE_FLUSH_TIMEOUT_MS: '5000',
        SC_BRIDGE_DEBUG: '1',
        SESSION_DEBUG: '1',
      },
    }
  );
  cleanupState.localChildren.push(devnet);
  cleanupState.devnetLog = devnetLog;
  await waitForFilePattern(
    devnetLog,
    /Mayhem dev-net ready\./,
    180_000,
    'local dev-net',
    async () => devnet.exitCode === null
  );
  const devnetText = fs.readFileSync(devnetLog, 'utf8');
  const adminWs = devnetText.match(/admin:\s+(ws:\/\/[^\s]+).*rpc=(http:\/\/[^\s]+)/);
  const joinerWs = devnetText.match(/joiner-a:\s+(ws:\/\/[^\s]+).*rpc=(http:\/\/[^\s]+)/);
  const joinerBWs = devnetText.match(/joiner-b:\s+(ws:\/\/[^\s]+).*rpc=(http:\/\/[^\s]+)/);
  const bootstrap = devnetText.match(/subnet bootstrap:\s+(\S+)/);
  const channel = devnetText.match(/subnet channel:\s+(\S+)/);
  const token = devnetText.match(/sc bridge token:\s+(\S+)/);
  const devnetLogs = devnetText.match(/logs:\s+(\S+)/);
  if (!adminWs || !joinerWs || !bootstrap || !channel || !token || !devnetLogs) {
    fail(`could not parse dev-net output in ${devnetLog}`);
  }
  const adminPeerLog = path.join(devnetLogs[1], 'admin.log');
  const joinerPeerLog = path.join(devnetLogs[1], 'joiner-a.log');
  const adminPubkey = fs.readFileSync(adminPeerLog, 'utf8').match(/Peer pubkey \(hex\):\s+([0-9a-f]{64})/);
  if (!adminPubkey) fail(`could not parse admin pubkey from ${adminPeerLog}`);
  const userPubkey = fs.readFileSync(joinerPeerLog, 'utf8').match(/Peer pubkey \(hex\):\s+([0-9a-f]{64})/);
  if (!userPubkey) fail(`could not parse user pubkey from ${joinerPeerLog}`);
  const adminScBridgeUrl = adminWs[1];
  const adminRpcUrl = adminWs[2];
  const adminRpcPort = new URL(adminRpcUrl).port;
  const userBridgeCandidates = [
    { label: 'joiner-a', url: joinerWs[1] },
    ...(joinerBWs ? [{ label: 'joiner-b', url: joinerBWs[1] }] : []),
    { label: 'admin', url: adminScBridgeUrl },
  ];
  let userScBridge = { label: 'joiner-a', url: joinerWs[1], ready: null };
  const scToken = token[1];
  const roomNonce = `p3.6-${tag}`;
  const enclaveId = await catalogEnclaveId({
    adminPubkey: adminPubkey[1],
    modelId,
    artifactRoot: artifactMerkle.root,
    manifestHash,
    binaryHash,
  });
  const roomId = (await b3(Buffer.from(`${enclaveId}${adminPubkey[1]}${roomNonce}`, 'utf8'))).toString('hex').slice(0, 32);

  log('seeding admin-created enclave, price, and room');
  const adminHome = path.join(runDir, 'admin-home');
  await fsp.mkdir(path.join(adminHome, 'stores'), { recursive: true });
  const adminStore = path.join(ROOT, 'intercom/stores/mayhem-devnet-admin');
  const adminLink = path.join(adminHome, 'stores/admin');
  try {
    await fsp.rm(adminLink, { recursive: true, force: true });
  } catch {}
  await fsp.symlink(adminStore, adminLink);
  const adminCommon = ['--home', adminHome, '--peer-store-name', 'admin', '--rpc-url', adminRpcUrl, '--submit', '--json'];
  const rulesHash = JSON.parse(runSync(mayhemBin, ['rules', 'hash', '--print-json'])).hash;
  const adminReports = {};
  function adminRun(name, args) {
    const stdout = runSync(mayhemBin, ['admin', ...args, ...adminCommon]);
    const outPath = path.join(runDir, `${name}.json`);
    fs.writeFileSync(outPath, stdout);
    adminReports[name] = readJson(outPath);
  }
  adminRun('admin-set-rules', ['set-rules', '--ver', '1', '--hash', rulesHash]);
  adminRun('admin-set-params', [
    'set-params',
    '--submitted-at', '0',
    '--effective-at', '86400',
    '--values-json', '{"fee_bps":1500,"holdback_epochs":0,"challenge_epochs":0,"payout_min_mu":0,"rate_staleness_seconds":86400}',
  ]);
  adminRun('admin-set-model-ref', [
    'set-model-ref',
    '--model', modelId,
    '--rate-map-json', rateMapJson,
  ]);
  adminRun('admin-register-enclave', [
    'register-enclave',
    '--enclave-id', enclaveId,
    '--model', modelId,
    '--backend', 'llama.cpp',
    '--artifact-root', artifactMerkle.root,
    '--artifact-root-kind', artifact.artifact_root_kind,
    '--artifact-repo', artifact.source.repo,
    '--artifact-revision', artifact.source.revision,
    '--artifact-path', artifact.path,
    '--source-sha256', artifactSha256,
    '--catalog-path', tempCatalogPath,
    '--dev-skip-catalog-verify',
    '--manifest-hash', manifestHash,
    '--binary-hash', binaryHash,
    '--caps-json', '{"chat":true,"tools":true,"json":true,"ctx":8192}',
  ]);
  adminRun('admin-set-price', [
    'set-price',
    '--enclave-id', enclaveId,
    '--rate-map-json', rateMapJson,
    '--effective-at', '0',
  ]);
  adminRun('admin-open-room', [
    'open-room',
    '--enclave-id', enclaveId,
    '--model', modelId,
    '--nonce', roomNonce,
    '--label', 'phase3-opencode-p2p',
  ]);
  const gatewayCreditMu = 10_000_000;
  const fiatDepositRef = (await b3(Buffer.from(`phase3-fiat-credit:${tag}:${userPubkey[1]}`, 'utf8'))).toString('hex');
  adminRun('admin-fiat-deposit', [
    'fiat-deposit',
    '--rail', 'stripe',
    '--who', userPubkey[1],
    '--mu', String(gatewayCreditMu),
    '--ext-ref-hash', fiatDepositRef,
    '--fiat-currency', 'usd',
    '--fiat-amount-minor', '1000',
    '--epoch', '1',
    '--at', '0',
  ]);

  log('starting remote Pear provider peer');
  const remoteScPort = await remoteFreePort(remote, passFile);
  const remoteRpcPort = await remoteFreePort(remote, passFile);
  const remoteAdminRpcTunnelPort = await remoteFreePort(remote, passFile);
  const remoteStore = `mayhem-p36-provider-${tag}`;
  const remoteMsbStore = `${remoteStore}-msb`;
  const remotePeerLog = path.posix.join(remoteLogs, 'provider-peer.log');
  const remotePeerArgs = [
    `--network local`,
    `--peer-store-name ${sh(remoteStore)}`,
    `--msb-store-name ${sh(remoteMsbStore)}`,
    `--subnet-channel ${sh(channel[1])}`,
    `--subnet-bootstrap ${sh(bootstrap[1])}`,
  ];
  if (peerDhtBootstrap) remotePeerArgs.push(`--peer-dht-bootstrap ${sh(peerDhtBootstrap)}`);
  remotePeerArgs.push(
    '--headless 1',
    '--peer-interactive 0',
    '--peer-replicate 1',
    '--peer-replicate-flush-timeout-ms 5000',
    '--sidechannel-quiet 1',
    '--sc-bridge 1',
    '--sc-bridge-host 127.0.0.1',
    `--sc-bridge-port ${remoteScPort}`,
    `--sc-bridge-token ${sh(scToken)}`,
    '--sc-bridge-cli 1',
    '--sc-bridge-debug 1',
    '--rpc 1',
    '--rpc-host 127.0.0.1',
    `--rpc-port ${remoteRpcPort}`,
    '--api-tx-exposed 1',
    '--api-tx-local-apply 1',
    '--session-debug 1',
    '--session-max-frame-bytes 262144',
    '--session-rate-bytes-per-second 1000000',
    '--session-rate-burst-bytes 1000000'
  );
  const remotePeerCmd = [
    `cd ${sh(path.posix.join(remoteRoot, 'intercom'))}`,
    `nohup ${sh(remotePear)} run . ${remotePeerArgs.join(' ')} > ${sh(remotePeerLog)} 2>&1 & echo $!`,
  ].join(' && ');
  const remotePeerPid = (await ssh(remote, passFile, remotePeerCmd)).stdout.trim();
  cleanupState.remotePids.push(remotePeerPid);
  const remotePeerReady = async () => remotePidAlive(remote, passFile, remotePeerPid);
  const remoteWaitCmd = `for i in $(seq 1 900); do grep -q 'Sidechannel: ready' ${sh(remotePeerLog)} && exit 0; kill -0 ${remotePeerPid} >/dev/null 2>&1 || exit 2; sleep 0.5; done; exit 1`;
  await ssh(remote, passFile, remoteWaitCmd, { timeoutMs: 480_000 });
  if (localPeerDht && localPeerDht.exitCode !== null) {
    fail(`local peer DHT bootstrap exited before provider discovery; see ${path.join(logsDir, 'local-peer-dht.log')}`);
  }
  if (remotePeerDhtPid && !(await remotePidAlive(remote, passFile, remotePeerDhtPid))) {
    fail(`Mac mini peer DHT bootstrap exited before provider discovery; see ${path.posix.join(remoteLogs, 'peer-dht.log')}`);
  }

  log('opening SSH reverse tunnel for remote provider contract RPC');
  const tunnelLog = path.join(logsDir, 'ssh-reverse-tunnel.log');
  const tunnel = spawnLogged(
    'sshpass',
    [
      '-f', passFile,
      'ssh',
      ...SSH_OPTS,
      '-N',
      '-R', `127.0.0.1:${remoteAdminRpcTunnelPort}:127.0.0.1:${adminRpcPort}`,
      `${remote.user}@${remote.host}`,
    ],
    tunnelLog
  );
  cleanupState.localChildren.push(tunnel);
  await sleep(1500);
  if (tunnel.exitCode !== null) fail(`reverse tunnel exited early; see ${tunnelLog}`);
  const remoteContractHealthUrl =
    `http://127.0.0.1:${remoteAdminRpcTunnelPort}/v1/state?prefix=price%2F&confirmed=false&limit=1`;
  await waitReverseTunnel(
    remote,
    passFile,
    remoteContractHealthUrl,
    tunnel,
    tunnelLog,
    'remote provider contract RPC'
  );

  log('starting remote provider session server');
  const remoteProviderHome = path.posix.join(remoteRun, 'provider-home');
  const remoteProviderSetupLog = path.posix.join(remoteLogs, 'provider-setup.log');
  const remoteProviderLog = path.posix.join(remoteLogs, 'provider-start.log');
  const prepareProviderHome = [
    `mkdir -p ${sh(path.posix.join(remoteProviderHome, 'stores'))}`,
    `rm -rf ${sh(path.posix.join(remoteProviderHome, 'stores/main'))}`,
    `ln -s ${sh(path.posix.join(remoteRoot, 'intercom/stores', remoteStore))} ${sh(path.posix.join(remoteProviderHome, 'stores/main'))}`,
  ].join(' && ');
  await ssh(
    remote,
    passFile,
    [
      prepareProviderHome,
      `cd ${sh(remoteRoot)}`,
      `MAYHEM_WALLET_HELPER=${sh(remoteWalletHelper)} MAYHEM_NODE_BIN=${sh(remoteNode)} ${sh(remoteMayhem)} setup --home ${sh(remoteProviderHome)} --role provider --wallet reuse --peer-store-name main --rpc-url ${sh(`http://127.0.0.1:${remoteAdminRpcTunnelPort}/v1`)} --rules-ver 1 --rules-hash ${sh(rulesHash)} --rules-path ${sh(remoteRules)} --yes --print-json > ${sh(remoteProviderSetupLog)} 2>&1`,
    ].join(' && '),
    { timeoutMs: 180_000 }
  );
  const providerFlags = [
    `--home ${sh(remoteProviderHome)}`,
    `--enclave ${sh(enclaveId)}`,
    `--rpc-url ${sh(`http://127.0.0.1:${remoteAdminRpcTunnelPort}/v1`)}`,
    `--sc-bridge-url ${sh(`ws://127.0.0.1:${remoteScPort}`)}`,
    `--sc-bridge-token ${sh(scToken)}`,
    `--catalog-path ${sh(remoteCatalog)}`,
    `--artifact ${sh(remoteArtifact)}`,
    '--engine-backend llama.cpp',
    '--skip-disk-bench',
    `--chunk-size ${CHUNK_SIZE}`,
    '--serve-sessions',
    '--serve-sessions-seconds 900',
    '--print-json',
    '--dev-skip-catalog-verify',
  ];
  if (mode === 'shim') providerFlags.push('--dev-session-shim');
  const providerCmd = [
    prepareProviderHome,
    `cd ${sh(remoteRoot)}`,
    `MAYHEM_PROVIDER_SESSION_DEBUG=1 MAYHEM_WALLET_HELPER=${sh(remoteWalletHelper)} MAYHEM_NODE_BIN=${sh(remoteNode)} nohup ${sh(remoteMayhem)} provider start ${providerFlags.join(' ')} > ${sh(remoteProviderLog)} 2>&1 & echo $!`,
  ].join(' && ');
  const remoteProviderPid = (await ssh(remote, passFile, providerCmd)).stdout.trim();
  cleanupState.remotePids.push(remoteProviderPid);
  const providerStartTimeoutSeconds = envPositiveInt(
    'MAYHEM_PHASE3_PROVIDER_START_TIMEOUT_SECONDS',
    mode === 'real' ? 1800 : 300
  );
  const providerWaitIterations = Math.ceil(providerStartTimeoutSeconds * 2);
  const providerWaitCmd = `for i in $(seq 1 ${providerWaitIterations}); do grep -q '"self_test"' ${sh(remoteProviderLog)} && exit 0; kill -0 ${remoteProviderPid} >/dev/null 2>&1 || exit 2; sleep 0.5; done; exit 1`;
  await ssh(remote, passFile, providerWaitCmd, { timeoutMs: providerStartTimeoutSeconds * 1_000 + 10_000 });
  const providerStartupText = (await ssh(remote, passFile, `cat ${sh(remoteProviderLog)}`)).stdout;
  const providerStartupReport = parseFirstJsonObject(providerStartupText);
  const providerPubkey = providerStartupReport?.provider;
  if (!providerPubkey) fail(`provider startup report did not include provider pubkey; see ${remoteProviderLog}`);

  log('selecting local SC-Bridge with direct session connectivity to remote provider');
  userScBridge = await pickDirectBridge(userBridgeCandidates, scToken, providerPubkey, 420_000);
  const directReady = userScBridge.ready;

  log('starting local contract-backed gateway');
  const gatewayPort = await freePort();
  const gatewayUrl = `http://127.0.0.1:${gatewayPort}`;
  const gatewayLog = path.join(logsDir, 'gateway.log');
  const gatewayHome = path.join(runDir, 'gateway-home');
  await fsp.mkdir(path.join(gatewayHome, 'stores'), { recursive: true });
  const gatewayStore = path.join(ROOT, 'intercom/stores/mayhem-devnet-joiner-a');
  const gatewayStoreLink = path.join(gatewayHome, 'stores/main');
  await fsp.rm(gatewayStoreLink, { recursive: true, force: true });
  await fsp.symlink(gatewayStore, gatewayStoreLink);
  const sessionOpenTimeoutSeconds = envPositiveInt(
    'MAYHEM_PHASE3_SESSION_OPEN_TIMEOUT_SECONDS',
    mode === 'real' ? 90 : 45
  );
  const sessionTtftTimeoutSeconds = envPositiveInt(
    'MAYHEM_PHASE3_SESSION_TTFT_TIMEOUT_SECONDS',
    mode === 'real' ? 300 : 60
  );
  const sessionFrameTimeoutSeconds = envPositiveInt(
    'MAYHEM_PHASE3_SESSION_FRAME_TIMEOUT_SECONDS',
    mode === 'real' ? 300 : 60
  );
  const chatTimeoutSeconds = envPositiveInt(
    'MAYHEM_PHASE3_CHAT_TIMEOUT_SECONDS',
    mode === 'real' ? 420 : 120
  );
  const chatMaxTokens = envPositiveInt('MAYHEM_PHASE3_CHAT_MAX_TOKENS', 32);
  const gateway = spawnLogged(
    mayhemBin,
    [
      'use',
      '--home', gatewayHome,
      '--rpc-url', adminRpcUrl,
      '--sc-bridge-url', userScBridge.url,
      '--sc-bridge-token', scToken,
      '--session-open-timeout-seconds', String(sessionOpenTimeoutSeconds),
      '--session-ttft-timeout-seconds', String(sessionTtftTimeoutSeconds),
      '--session-frame-timeout-seconds', String(sessionFrameTimeoutSeconds),
      '--dev-catalog-path', tempCatalogPath,
      '--dev-skip-catalog-verify',
      '--bind', `127.0.0.1:${gatewayPort}`,
      '--json',
    ],
    gatewayLog
  );
  cleanupState.localChildren.push(gateway);
  await waitHttp(`${gatewayUrl}/mayhem/status`, 120_000, 'local gateway', async () => gateway.exitCode === null);

  log('running streaming curl-compatible chat through the gateway');
  await waitReverseTunnel(
    remote,
    passFile,
    remoteContractHealthUrl,
    tunnel,
    tunnelLog,
    'remote provider contract RPC before chat'
  );
  const chatStream = await runStreamingChatSmokeWithRetries(gatewayUrl, modelId, runDir, {
    timeoutMs: chatTimeoutSeconds * 1_000,
    maxTokens: chatMaxTokens,
  });

  const runOpencode = /^(1|true|yes)$/i.test(process.env.MAYHEM_PHASE3_RUN_OPENCODE || '');
  const configHome = path.join(runDir, 'xdg-config');
  const opencodeConfig = path.join(configHome, 'opencode/opencode.json');
  await fsp.mkdir(path.dirname(opencodeConfig), { recursive: true });
  writeJson(opencodeConfig, { $schema: 'https://opencode.ai/config.json' });
  const testJsonPath = path.join(runDir, 'mayhem-test.json');
  let testJson;
  if (runOpencode) {
    log('running optional mayhem test with opencode through the gateway');
    const test = await run(
      mayhemBin,
      [
        'test',
        '--home', gatewayHome,
        '--gateway-url', gatewayUrl,
        '--rpc-url', adminRpcUrl,
        '--model', modelId,
        '--skip-direct-tool-smoke',
        '--sync-models',
        '--opencode-config', opencodeConfig,
        '--opencode-bin', opencodeBin,
        '--timeout-seconds', mode === 'real' ? '240' : '120',
        '--json',
      ],
      {
        logFile: path.join(logsDir, 'mayhem-test.log'),
        timeoutMs: mode === 'real' ? 300_000 : 180_000,
      }
    );
    fs.writeFileSync(testJsonPath, test.stdout);
    testJson = JSON.parse(test.stdout);
  } else {
    testJson = {
      ok: true,
      opencode: {
        run: {
          skipped: true,
          reason: 'MAYHEM_PHASE3_RUN_OPENCODE is not set; E1 uses curl-compatible streaming chat',
        },
      },
    };
    writeJson(testJsonPath, testJson);
    fs.writeFileSync(
      path.join(logsDir, 'mayhem-test.log'),
      'opencode skipped; set MAYHEM_PHASE3_RUN_OPENCODE=1 to run the tool-call smoke\n'
    );
  }
  const receipts = await (await fetch(`${gatewayUrl}/mayhem/receipts`)).json();
  const receiptsPath = path.join(runDir, 'gateway-receipts.json');
  writeJson(receiptsPath, receipts);
  const modelsJson = await (await fetch(`${gatewayUrl}/v1/models`)).json();
  const modelsPath = path.join(runDir, 'gateway-models.json');
  writeJson(modelsPath, modelsJson);

  const providerLogText = (await ssh(remote, passFile, `cat ${sh(remoteProviderLog)}`)).stdout;
  const providerReport = parseFirstJsonObject(providerLogText);
  const selectedModel = modelsJson.data?.find((entry) => entry.id === modelId) || null;
  const latestReceipt = Array.isArray(receipts.data) ? receipts.data.at(-1) : null;
  const receiptBody = latestReceipt?.receipt?.body || latestReceipt?.receipt || latestReceipt || null;
  if (!latestReceipt) fail('streaming chat completed but no gateway receipt was recorded');
  log('settling the streamed receipt through epoch commit/apply');
  const epochSettlement = await settleGatewayReceiptEpoch({
    mayhemBin,
    adminHome,
    adminRpcUrl,
    receiptsPath,
    runDir,
    user: userPubkey[1],
    provider: providerPubkey,
    epoch: 1,
    feeBps: 1500,
  });

  const report = {
    ok: true,
    tag,
    provider_mode: mode,
    run_dir: path.relative(ROOT, runDir),
    local: {
      gateway_url: gatewayUrl,
      admin_rpc_url: adminRpcUrl,
      admin_sc_bridge_url: adminScBridgeUrl,
      user_sc_bridge_label: userScBridge.label,
      user_sc_bridge_url: userScBridge.url,
      subnet_channel: subnetChannel,
      peer_dht_bootstrap: peerDhtBootstrap,
    },
    remote: {
      host: remote.host,
      root: remoteRoot,
      run_dir: remoteRun,
      provider_peer_pid: remotePeerPid,
      provider_start_pid: remoteProviderPid,
      sc_bridge_url: `ws://127.0.0.1:${remoteScPort}`,
      provider_log: remoteProviderLog,
    },
    admin: {
      pubkey: adminPubkey[1],
      credited_user: userPubkey[1],
      gateway_credit_mu: gatewayCreditMu,
      room_nonce: roomNonce,
      room_id: roomId,
      rules_hash: rulesHash,
      reports: Object.fromEntries(Object.entries(adminReports).map(([key, value]) => [key, {
        submitted: value.submitted,
        tx_type: value.tx_type,
        command: value.command,
      }])),
    },
    enclave: {
      enclave_id: enclaveId,
      model_id: modelId,
      backend: 'llama.cpp',
      artifact_name: artifactName,
      artifact_root: artifactMerkle.root,
      artifact_chunks: artifactMerkle.chunks,
      artifact_bytes: artifactMerkle.total_bytes,
      artifact_sha256: artifactSha256,
      manifest_hash: manifestHash,
      binary_hash: binaryHash,
      binary_sha256: binarySha256,
    },
    provider: {
      pubkey: providerReport?.provider || providerPubkey,
      self_test: providerReport?.self_test || null,
      attestation: providerReport?.attestation || null,
      rooms: providerReport?.rooms || [],
      features: providerReport?.features || null,
      artifact: providerReport?.artifact || null,
      direct_ready: directReady,
    },
    gateway: {
      chat_stream: chatStream,
      models: modelsPath,
      selected_model: selectedModel,
      receipts: receiptsPath,
      latest_receipt: receiptBody,
      epoch_settlement: epochSettlement,
    },
    opencode: {
      binary: opencodeBin,
      config: opencodeConfig,
      test_report: testJsonPath,
      tool_use_seen: testJson.opencode?.run?.tool_use_seen === true,
      marker_seen: testJson.opencode?.run?.marker_seen === true,
      model: testJson.opencode?.run?.model || null,
    },
    mayhem_test: testJson,
  };
  const reportPath = path.join(runDir, 'report.json');
  writeJson(reportPath, report);
  console.log(JSON.stringify(report, null, 2));

  await cleanup({
    remote,
    passFile,
    remotePids: [remoteProviderPid, remotePeerPid],
    localChildren: [gateway, tunnel, devnet, localPeerDht],
    devnetLog,
    remoteRun,
  });
}

async function cleanup({ remote, passFile, remotePids, localChildren, devnetLog, remoteRun }) {
  if (cleanupStarted) return;
  cleanupStarted = true;
  log('cleaning up processes');
  for (const child of localChildren || []) {
    if (child && child.exitCode === null) child.kill('SIGTERM');
  }
  for (const child of children) {
    if (child && child.exitCode === null) child.kill('SIGTERM');
  }
  for (const pid of remotePids || []) {
    if (remote && passFile && fs.existsSync(passFile) && pid) {
      try {
        await ssh(remote, passFile, `kill ${Number(pid)} >/dev/null 2>&1 || true`);
      } catch {}
    }
  }
  if (devnetLog && fs.existsSync(devnetLog)) {
    const text = fs.readFileSync(devnetLog, 'utf8');
    const match = text.match(/Peers are still running\. Stop them with: kill ([0-9\s]+)/);
    if (match) {
      for (const pid of match[1].trim().split(/\s+/)) {
        try {
          process.kill(Number(pid), 'SIGTERM');
        } catch {}
      }
    }
  }
  if (process.env.MAYHEM_PHASE3_KEEP_REMOTE === '0' && remote && passFile && fs.existsSync(passFile) && remoteRun) {
    try {
      await ssh(remote, passFile, `rm -rf ${sh(remoteRun)}`);
    } catch {}
  }
  for (const file of sensitiveFiles) {
    try {
      await fsp.rm(file, { force: true });
    } catch {}
  }
}

process.on('SIGINT', () => {
  process.stderr.write('\n');
  process.exitCode = 130;
});

main().catch(async (err) => {
  console.error(`[p3.6] ERROR: ${err.stack || err.message}`);
  await cleanup(cleanupState);
  process.exit(1);
});

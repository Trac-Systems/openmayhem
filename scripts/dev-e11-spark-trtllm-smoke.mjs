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

const MODEL_ID = 'qwen/qwen2.5-1.5b-instruct@small';
const TRT_ARTIFACT = 'nvfp4';
const DEFAULT_ACCEL_ARTIFACT = TRT_ARTIFACT;
const GGUF_ARTIFACT = 'gguf-q4_k_m';
const CHUNK_SIZE = 8 * 1024 * 1024;
const DEFAULT_CATALOG_RELEASE_REPO = 'TracNetwork/mayhem-catalog';
const DEFAULT_CATALOG_RELEASE_REVISION = '4852d4856535a49d202bfe84ae016b8d8b2e446f';
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
  '-o', `UserKnownHostsFile=${path.join(ROOT, '.mayhem-local/e11-spark-known-hosts')}`,
];
const KEY_SSH_OPTS = [
  '-F', '/dev/null',
  '-o', 'BatchMode=yes',
  '-o', 'PreferredAuthentications=publickey',
  '-o', 'PasswordAuthentication=no',
  '-o', 'KbdInteractiveAuthentication=no',
  '-o', 'ConnectTimeout=8',
  '-o', 'ExitOnForwardFailure=yes',
  '-o', 'ServerAliveInterval=15',
  '-o', 'ServerAliveCountMax=3',
  '-o', 'StrictHostKeyChecking=no',
  '-o', `UserKnownHostsFile=${path.join(ROOT, '.mayhem-local/e11-spark-known-hosts')}`,
];

const children = [];
const cleanupState = {
  remote: null,
  passFile: null,
  remotePids: [],
  localChildren: [],
  devnetLog: null,
  remoteRun: null,
  remoteProviderCacheRoot: null,
  remoteHfToken: null,
};
let cleanupStarted = false;

function usage() {
  console.log(`Usage:
  node scripts/dev-e11-spark-trtllm-smoke.mjs

Runs the I3-E11 acceptance smoke:
  - local Pear admin/user/provider-bridge dev-net
  - Spark-hosted provider start and NVIDIA accelerated engine
  - admin-created GGUF + accelerated enclaves for ${MODEL_ID}
  - real gateway chat, receipt, and epoch billing
  - local Apple fixture provider-start fallback selects GGUF

Environment:
  MAYHEM_E11_CLUSTER_FILE          Spark credential file (default: .mayhem-local/secrets/cluster.txt)
  MAYHEM_E11_SSH_TARGET            Key-based SSH target, e.g. mayhem@52.230.164.69 (bypasses cluster file)
  MAYHEM_E11_SSH_HOST              Key-based SSH host (with MAYHEM_E11_SSH_USER, default: current user)
  MAYHEM_E11_SSH_KEY               Optional private-key path for key-based SSH; ssh-agent is used when unset
  MAYHEM_E11_TAG                   Reuse a named run directory/tag instead of generating one
  MAYHEM_E11_DEVNET_CLEANUP        Remove dev-net stores before starting (default: 1)
  MAYHEM_E11_REMOTE_ROOT           Spark checkout/staging root (default: $HOME/mayhem/i3-e11-openmayhem)
  MAYHEM_E11_REMOTE_TARGET_DIR     Remote Cargo target dir (default: <remote root>/target)
  MAYHEM_E11_REMOTE_DOWNLOADS      Spark provider download cache (default: $HOME/mayhem/e11-provider-downloads)
  MAYHEM_E11_REMOTE_PROVIDER_CACHE Spark provider sealed-store cache root (default: <remote root>/.mayhem-local/i3-e11-spark-provider-cache)
  MAYHEM_E11_REMOTE_ENV_PRELUDE    Shell prelude sourced before remote build/provider commands
  MAYHEM_E11_NODE_BIN              Remote node binary for wallet-helper signing (default: command -v node)
  MAYHEM_E11_NPM_BIN               Remote npm binary for one-time wallet-helper dependency install
  MAYHEM_E11_ACCEL_ARTIFACT        Accelerated catalog artifact to prove (default: ${DEFAULT_ACCEL_ARTIFACT}; E14 uses vllm-fp16)
  MAYHEM_E11_TRTLLM_PYTHON         Spark TensorRT-LLM Python wrapper (default: $HOME/mayhem/bin/trtllm-python)
  MAYHEM_E11_VLLM_PYTHON           Spark vLLM Python wrapper (default: $HOME/mayhem/bin/vllm-python-e14)
  MAYHEM_E11_VLLM_ENV_PRELUDE      Extra shell env assignments for remote vLLM/provider startup
  MAYHEM_E11_HF_TOKEN_FILE         HF token file copied temporarily to Spark if present (default: .mayhem-local/secrets/hf.txt)
  MAYHEM_E11_CATALOG_RELEASE_REPO  HF repo for the signed catalog release (default: ${DEFAULT_CATALOG_RELEASE_REPO})
  MAYHEM_E11_CATALOG_RELEASE_REVISION  40-hex signed catalog release revision (default: ${DEFAULT_CATALOG_RELEASE_REVISION})
  MAYHEM_E11_SYNC_NODE_MODULES     Sync intercom/node_modules when missing on Spark (default: 1)
  MAYHEM_E11_PROVIDER_START_TIMEOUT_SECONDS  Spark provider start timeout (default: 1800)
  MAYHEM_E11_CHAT_TIMEOUT_SECONDS  Gateway chat timeout (default: 600)
  MAYHEM_E11_CHAT_MAX_TOKENS       Gateway chat max_tokens (default: 24)
  MAYHEM_E11_TOOL_TIMEOUT_SECONDS  Gateway guided tool-call timeout (default: 600)
  MAYHEM_E11_CONCURRENT_TIMEOUT_SECONDS  vLLM concurrent heartbeat timeout (default: 900)
  MAYHEM_E11_CONCURRENT_SESSIONS   Concurrent vLLM sessions for heartbeat proof (default: 2; D1 uses 3)
  MAYHEM_E11_CONCURRENT_MAX_TOKENS vLLM concurrent prompt max_tokens (default: 32)
  MAYHEM_E11_CONCURRENT_ABORT_AFTER_PROOF  Abort extra streams once heartbeat is captured (default: 0)
  MAYHEM_E11_PROVIDER_SESSION_REQUEST_TIMEOUT_MS  Provider accepted-open request timeout (default: 15000)
  MAYHEM_E11_ACCEL_ATT_TIER        Attestation tier for the accelerated enclave (default: 1)
  MAYHEM_E11_ACCEL_LAUNCH_MEASUREMENTS_JSON  Launch measurements JSON required when att tier is 3
  MAYHEM_E11_ACCEL_RATE_MAP_JSON   Optional accelerated enclave price map (default: model ref; Tier-3 default doubles it)
  MAYHEM_E11_ACCEL_PRICE_MULTIPLIER Tier-3 default multiplier when no accelerated rate map is supplied (default: 2)
  MAYHEM_E11_REQUEST_MIN_ATT_TIER  Optional X-Mayhem-Min-Att-Tier header for gateway requests (D6 uses 3)
  MAYHEM_E11_PROVIDER_HW_QUOTE_KIND      Provider hardware quote kind for Tier-2/3 smokes
  MAYHEM_E11_PROVIDER_HW_QUOTE_COMMAND   Remote provider hardware quote command path
  MAYHEM_E11_PROVIDER_HW_QUOTE_TIMEOUT_SECONDS  Provider hardware quote timeout (default: 120)
  MAYHEM_E11_SKIP_MAC_FALLBACK     Skip the local Apple GGUF fallback assertion (default: 0)
  MAYHEM_E11_SKIP_TOOL_CALL        Skip the vLLM guided tool-call smoke (default: 0)
  MAYHEM_E11_KEEP_REMOTE_RUN       Keep Spark run home/logs after cleanup (default: 0; token is always removed)
  MAYHEM_E11_KEEP_PROVIDER_CACHE   Keep Spark sealed-store cache on cleanup (default: 1)
`);
}

function log(message) {
  process.stderr.write(`[e11] ${message}\n`);
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

function writeJson(file, value) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`);
}

function envPositiveInt(name, fallback) {
  const raw = process.env[name];
  if (raw === undefined || raw === '') return fallback;
  const value = Number.parseInt(raw, 10);
  if (!Number.isSafeInteger(value) || value <= 0) fail(`${name} must be a positive integer, got ${raw}`);
  return value;
}

function envFlag(name, fallback) {
  const raw = process.env[name];
  if (raw === undefined || raw === '') return fallback;
  return !/^(0|false|no)$/i.test(raw);
}

function runSync(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: options.cwd || ROOT,
    env: options.env || process.env,
    encoding: 'utf8',
    maxBuffer: options.maxBuffer || 128 * 1024 * 1024,
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
          reject(new Error(`${command} ${args.join(' ')} timed out after ${options.timeoutMs}ms`));
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
        reject(new Error(`${command} ${args.join(' ')} exited ${code ?? signal}${stderr.trim() ? `\n${stderr.trim()}` : ''}`));
        return;
      }
      resolve({ stdout, stderr });
    });
  });
}

function spawnLogged(command, args, logFile, options = {}) {
  fs.mkdirSync(path.dirname(logFile), { recursive: true });
  const out = fs.createWriteStream(logFile, { flags: 'a' });
  const child = spawn(command, args, {
    cwd: options.cwd || ROOT,
    env: options.env || process.env,
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  child.stdout.pipe(out, { end: false });
  child.stderr.pipe(out, { end: false });
  child.on('close', () => out.end());
  children.push(child);
  return child;
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
  fail(`timed out waiting for ${label}; last log tail:\n${last.split('\n').slice(-80).join('\n')}`);
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

function parseCluster(file) {
  const target = process.env.MAYHEM_E11_SSH_TARGET || '';
  const envHost = process.env.MAYHEM_E11_SSH_HOST || '';
  if (target || envHost) {
    const match = /^(?:(?<user>[^@]+)@)?(?<host>[^:]+)(?::\d+)?$/.exec(target || envHost);
    const user = process.env.MAYHEM_E11_SSH_USER || match?.groups?.user || os.userInfo().username;
    const host = match?.groups?.host || envHost;
    if (!user || !host) fail('MAYHEM_E11_SSH_TARGET or MAYHEM_E11_SSH_HOST must include an SSH host');
    return {
      auth: 'key',
      user,
      host,
      key: process.env.MAYHEM_E11_SSH_KEY ? path.resolve(ROOT, process.env.MAYHEM_E11_SSH_KEY) : null,
    };
  }
  const text = fs.readFileSync(file, 'utf8');
  const first = {};
  for (const line of text.split(/\r?\n/)) {
    const match = /^([^:#]+):\s*(.+)$/.exec(line.trim());
    if (!match) continue;
    if (first[match[1]]) continue;
    first[match[1]] = match[2];
  }
  const ssh = first.ssh || '';
  const host = (ssh.match(/@([A-Za-z0-9_.-]+)/) || [])[1] || first.host;
  if (!first.user || !first.pass || !host) {
    fail(`${file} must include user, pass, and ssh host for the Spark`);
  }
  return { auth: 'password', user: first.user, pass: first.pass, host };
}

function remoteTarget(remote) {
  return `${remote.user}@${remote.host}`;
}

function keySshOpts(remote) {
  const opts = [...KEY_SSH_OPTS];
  if (remote.key) opts.push('-i', remote.key, '-o', 'IdentitiesOnly=yes');
  return opts;
}

function sshInvocation(remote, passFile) {
  if (remote.auth === 'key') {
    return { command: 'ssh', args: keySshOpts(remote) };
  }
  return { command: 'sshpass', args: ['-f', passFile, 'ssh', ...SSH_OPTS] };
}

function retryableRemoteError(error) {
  const message = String(error?.message || error || '');
  return /Permission denied|exited 255|Connection (reset|closed|timed out)|kex_exchange_identification|Broken pipe|rsync error.*code 255/i.test(message);
}

async function withRemoteRetries(label, action, attempts = 3) {
  let lastError;
  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    try {
      return await action();
    } catch (error) {
      lastError = error;
      if (attempt >= attempts || !retryableRemoteError(error)) throw error;
      log(`${label} failed transiently (attempt ${attempt}/${attempts}); retrying`);
      await sleep(1000 * attempt);
    }
  }
  throw lastError;
}

async function ssh(remote, passFile, command, options = {}) {
  const invocation = sshInvocation(remote, passFile);
  return withRemoteRetries(
    `ssh ${remote.host}`,
    () => run(invocation.command, [...invocation.args, remoteTarget(remote), command], options)
  );
}

async function rsyncTo(remote, passFile, localPath, remotePath, options = {}) {
  const invocation = sshInvocation(remote, passFile);
  const rsh = [invocation.command, ...invocation.args].map(sh).join(' ');
  return withRemoteRetries(
    `rsync ${remote.host}`,
    () => run(
      'rsync',
      [
        '-a',
        '--partial',
        '--inplace',
        ...(options.extraArgs || []),
        '-e',
        rsh,
        localPath,
        `${remote.user}@${remote.host}:${remotePath}`,
      ],
      options
    )
  );
}

async function rsyncRepoTo(remote, passFile, localPath, remotePath, options = {}) {
  return rsyncTo(remote, passFile, localPath, remotePath, {
    ...options,
    extraArgs: [
      '--exclude', '.git/',
      '--exclude', 'target/',
      '--exclude', '.mayhem-local/',
      '--exclude', 'node_modules/',
      '--exclude', 'intercom/node_modules/',
      '--exclude', 'intercom/stores/',
      ...(options.extraArgs || []),
    ],
  });
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

async function waitReverseTunnel(remote, passFile, url, tunnel, tunnelLog, label, timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs;
  let lastErr = '';
  const script = [
    'import urllib.request',
    `urllib.request.urlopen(${JSON.stringify(url)}, timeout=2).read()`,
  ].join('\n');
  while (Date.now() < deadline) {
    if (tunnel.exitCode !== null) fail(`${label} reverse tunnel exited early; see ${tunnelLog}`);
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

function parseJsonObjectAt(text, start) {
  if (start < 0) return null;
  let depth = 0;
  let inString = false;
  let escape = false;
  for (let i = start; i < text.length; i += 1) {
    const ch = text[i];
    if (inString) {
      if (escape) escape = false;
      else if (ch === '\\') escape = true;
      else if (ch === '"') inString = false;
      continue;
    }
    if (ch === '"') inString = true;
    else if (ch === '{') depth += 1;
    else if (ch === '}') {
      depth -= 1;
      if (depth === 0) {
        try {
          return JSON.parse(text.slice(start, i + 1));
        } catch {
          return null;
        }
      }
    }
  }
  return null;
}

function parseProviderStartupReport(text) {
  for (let start = text.indexOf('{'); start >= 0; start = text.indexOf('{', start + 1)) {
    const parsed = parseJsonObjectAt(text, start);
    if (parsed?.provider && parsed?.self_test) return parsed;
  }
  return null;
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

async function catalogEnclaveId({ adminPubkey, modelId, artifactRoot, artifactSidecarRoots = {}, manifestHash }) {
  const sidecarBinding = Object.entries(artifactSidecarRoots)
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([name, root]) => `${name}${root}`)
    .join('');
  return (await b3(Buffer.from(`${adminPubkey}${modelId}${artifactRoot}${sidecarBinding}${manifestHash}`, 'utf8'))).toString('hex');
}

function artifactSidecarRoots(artifact) {
  return Object.fromEntries(
    Object.entries(artifact.sidecars || {}).map(([name, sidecar]) => [name, sidecar.artifact_root])
  );
}

function textRateMapJson(model) {
  return JSON.stringify([
    { unit: 'input_token', per_unit_au: String(model.price_ref_au?.in_per_1k ?? '18'), granularity: 1000 },
    { unit: 'output_token', per_unit_au: String(model.price_ref_au?.out_per_1k ?? '55'), granularity: 1000 },
  ]);
}

function scaleRateMapJson(rateMapJson, multiplier) {
  if (!Number.isSafeInteger(multiplier) || multiplier <= 0) fail(`price multiplier must be a positive integer, got ${multiplier}`);
  return JSON.stringify(JSON.parse(rateMapJson).map((entry) => ({
    ...entry,
    per_unit_au: (BigInt(String(entry.per_unit_au)) * BigInt(multiplier)).toString(),
  })));
}

function acceleratedCapsJson(backend) {
  if (backend === 'vllm') {
    return JSON.stringify({
      chat: true,
      tools: true,
      json: true,
      ctx: 1024,
      tp_degree: 1,
      max_batch_size: 2,
      max_num_tokens: 1024,
      vllm_dtype: 'bfloat16',
      vllm_gpu_memory_utilization_pct: 40,
    });
  }
  return JSON.stringify({
    chat: true,
    tools: false,
    json: true,
    ctx: 1024,
    tp_degree: 1,
    max_batch_size: 1,
    max_num_tokens: 1024,
  });
}

function isSupportedSparkAccelerator(artifact) {
  return artifact?.engine === 'trt-llm' || artifact?.engine === 'vllm';
}

function bridgeRequest(url, token, payload, timeoutMs = 10_000) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(url);
    const requestId = 2;
    let sent = false;
    const timer = setTimeout(() => {
      try { ws.close(); } catch {}
      reject(new Error(`SC-Bridge request timed out on ${url}`));
    }, timeoutMs);
    const finish = (err, value) => {
      clearTimeout(timer);
      try { ws.close(); } catch {}
      if (err) reject(err);
      else resolve(value);
    };
    ws.onerror = (error) => finish(new Error(error?.message || String(error)));
    ws.onopen = () => ws.send(JSON.stringify({ id: 1, type: 'auth', token }));
    ws.onmessage = (event) => {
      let msg;
      try {
        msg = JSON.parse(event.data);
      } catch {
        return;
      }
      if (msg.id === 1 && msg.type === 'auth_ok') {
        sent = true;
        ws.send(JSON.stringify({ ...payload, id: requestId }));
        return;
      }
      if (msg.id === 1 && msg.type === 'error') {
        finish(new Error(JSON.stringify(msg)));
        return;
      }
      if (sent && msg.id === requestId) {
        if (msg.type === 'error') finish(new Error(JSON.stringify(msg)));
        else finish(null, msg);
      }
    };
  });
}

function heartbeatMessageFromEvent(event) {
  const message = event?.message;
  if (message && typeof message === 'object' && message.t === 'hb') return message;
  if (typeof message === 'string') {
    try {
      const parsed = JSON.parse(message);
      if (parsed && typeof parsed === 'object' && parsed.t === 'hb') return parsed;
    } catch {}
  }
  return null;
}

async function startHeartbeatCollector(url, token, channel, outPath) {
  const events = [];
  const waiters = [];
  let ws;
  let closed = false;
  const record = (event) => {
    events.push(event);
    writeJson(outPath, { channel, events });
    for (const waiter of [...waiters]) {
      if (waiter.predicate(event)) {
        waiter.resolve(event);
        waiters.splice(waiters.indexOf(waiter), 1);
      }
    }
  };
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`heartbeat collector did not subscribe to ${channel}`)), 30_000);
    ws = new WebSocket(url);
    ws.onerror = (error) => {
      clearTimeout(timer);
      reject(new Error(error?.message || String(error)));
    };
    ws.onopen = () => ws.send(JSON.stringify({ id: 1, type: 'auth', token }));
    ws.onmessage = (event) => {
      let msg;
      try {
        msg = JSON.parse(event.data);
      } catch {
        return;
      }
      if (msg.type === 'sidechannel_message' && msg.channel === channel) {
        record(msg);
        return;
      }
      if (msg.id === 1 && msg.type === 'auth_ok') {
        ws.send(JSON.stringify({ id: 2, type: 'join', channel }));
        return;
      }
      if (msg.id === 2 && msg.type === 'joined') {
        ws.send(JSON.stringify({ id: 3, type: 'subscribe', channel }));
        return;
      }
      if (msg.id === 3 && msg.type === 'subscribed') {
        clearTimeout(timer);
        resolve();
        return;
      }
      if (msg.type === 'error') {
        clearTimeout(timer);
        reject(new Error(JSON.stringify(msg)));
      }
    };
  });
  return {
    events,
    async waitFor(predicate, timeoutMs) {
      const existing = events.find(predicate);
      if (existing) return existing;
      return new Promise((resolve, reject) => {
        const waiter = { predicate, resolve };
        waiters.push(waiter);
        const timer = setTimeout(() => {
          const idx = waiters.indexOf(waiter);
          if (idx >= 0) waiters.splice(idx, 1);
          reject(new Error(`timed out waiting for heartbeat on ${channel}`));
        }, timeoutMs);
        waiter.resolve = (event) => {
          clearTimeout(timer);
          resolve(event);
        };
      });
    },
    close() {
      if (closed) return;
      closed = true;
      try { ws.close(); } catch {}
    },
  };
}

async function waitBridgeSessionOpen(url, token, provider, timeoutMs, label) {
  const deadline = Date.now() + timeoutMs;
  let lastError = null;
  let attempts = 0;
  while (Date.now() < deadline) {
    attempts += 1;
    const sessionId = crypto.randomBytes(32).toString('hex');
    const remaining = Math.max(1_000, deadline - Date.now());
    try {
      await bridgeRequest(url, token, { type: 'peer_connect', remote: provider, wait_ms: Math.min(remaining, 15_000) }, Math.min(remaining, 20_000));
      const opened = await bridgeRequest(url, token, { type: 'session_open', remote: provider, session_id: sessionId }, Math.min(remaining, 10_000));
      await bridgeRequest(url, token, { type: 'session_close', remote: provider, session_id: sessionId }, 5_000).catch(() => {});
      return { label, url, attempts, session_id: sessionId, opened };
    } catch (err) {
      lastError = err;
      await sleep(2_000);
    }
  }
  throw new Error(`${label} could not open a direct session to ${provider}: ${lastError?.message || 'unknown error'}`);
}

async function waitGatewayRoute(gatewayUrl, modelId, provider, timeoutMs = 180_000, minAttTier = null) {
  const deadline = Date.now() + timeoutMs;
  let last = null;
  while (Date.now() < deadline) {
    try {
      const models = await (await fetch(`${gatewayUrl}/v1/models`)).json();
      const selected = models.data?.find((entry) => entry.id === modelId);
      const routes = selected?.mayhem?.route_candidates || [];
      if (routes.some((route) => route.provider === provider && (minAttTier === null || Number(route.att_tier || 0) >= minAttTier))) {
        return { selected, routes };
      }
      last = { selected: Boolean(selected), routes };
    } catch (err) {
      last = { error: err.message };
    }
    await sleep(1000);
  }
  fail(`gateway did not expose provider route: ${JSON.stringify(last)}`);
}

async function waitGatewayProbe(gatewayUrl, provider, enclaveId, timeoutMs = 180_000) {
  const deadline = Date.now() + timeoutMs;
  let last = null;
  while (Date.now() < deadline) {
    try {
      const probes = await (await fetch(`${gatewayUrl}/mayhem/probes`)).json();
      const data = Array.isArray(probes.data) ? probes.data : [];
      const selected = data.find((probe) => probe.provider === provider && probe.enclave_id === enclaveId);
      if (selected) {
        if (selected.pass !== true) fail(`gateway canary probe failed: ${JSON.stringify(selected)}`);
        return selected;
      }
      last = { count: data.length, probes: data.slice(-3) };
    } catch (err) {
      last = { error: err.message };
    }
    await sleep(1000);
  }
  fail(`gateway did not record a passing canary probe for ${provider}/${enclaveId}: ${JSON.stringify(last)}`);
}

async function runStreamingChatSmoke(
  gatewayUrl,
  modelId,
  runDir,
  { timeoutMs, maxTokens, prompt, fileStem, allowTransportErrorAfterContent = false, signal = null, minAttTier = null }
) {
  const stem = fileStem || 'gateway-chat-stream';
  const rawPath = path.join(runDir, `${stem}.sse`);
  const summaryPath = path.join(runDir, `${stem}.json`);
  const chatPrompt = prompt || 'Write one short sentence proving this is a live Mayhem model response. Include the word coral.';
  const controller = new AbortController();
  const fetchSignal = signal
    ? (AbortSignal.any ? AbortSignal.any([controller.signal, signal]) : controller.signal)
    : controller.signal;
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
  let streamError = null;
  try {
    const response = await fetch(`${gatewayUrl}/v1/chat/completions`, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        ...(minAttTier ? { 'X-Mayhem-Min-Att-Tier': String(minAttTier) } : {}),
      },
      body: JSON.stringify({
        model: modelId,
        stream: true,
        temperature: 0.7,
        max_tokens: maxTokens,
        messages: [{ role: 'user', content: chatPrompt }],
      }),
      signal: fetchSignal,
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
    for (;;) {
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
  } catch (err) {
    streamError = err;
  } finally {
    clearTimeout(timer);
  }
  fs.writeFileSync(rawPath, raw);
  if (streamError && !(allowTransportErrorAfterContent && content.trim())) {
    fail(`streaming chat transport failed: ${streamError.message}; raw saved at ${rawPath}`);
  }
  if (dataEvents === 0) fail('streaming chat produced no SSE data events');
  if (!content.trim()) fail(`streaming chat produced no model content; raw saved at ${rawPath}`);
  const completedAt = doneAt || Date.now();
  const summary = {
    ok: true,
    model: modelId,
    min_att_tier: minAttTier,
    prompt: chatPrompt,
    max_tokens: maxTokens,
    content: content.trim(),
    content_chars: content.trim().length,
    data_events: dataEvents,
    json_events: jsonEvents,
    done_seen: doneSeen,
    transport_error: streamError ? streamError.message : null,
    transport_error_tolerated: Boolean(streamError && allowTransportErrorAfterContent && content.trim()),
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

function firstFulfilled(promises) {
  return new Promise((resolve, reject) => {
    const errors = [];
    let rejected = 0;
    promises.forEach((promise, index) => {
      promise.then(
        (value) => resolve({ index, value }),
        (error) => {
          errors[index] = error;
          rejected += 1;
          if (rejected === promises.length) {
            reject(new AggregateError(errors, 'all concurrent streams failed'));
          }
        }
      );
    });
  });
}

async function runConcurrentHeartbeatSmoke(
  gatewayUrl,
  modelId,
  runDir,
  heartbeatCollector,
  { timeoutMs, maxTokens, provider, enclaveId, sessionCount, abortAfterProof = false, minAttTier = null }
) {
  const heartbeatEventsPath = path.join(runDir, 'gateway-concurrent-heartbeats.json');
  const promptTopics = [
    'Write a compact checklist about coral restoration. Keep going until the token limit.',
    'Write a compact checklist about satellite safety. Keep going until the token limit.',
    'Write a compact checklist about battery recycling. Keep going until the token limit.',
    'Write a compact checklist about cold-chain logistics. Keep going until the token limit.',
    'Write a compact checklist about urban flood response. Keep going until the token limit.',
  ];
  const prompts = Array.from({ length: sessionCount }, (_, index) => (
    promptTopics[index] || `Write a compact checklist about test topic ${index + 1}. Keep going until the token limit.`
  ));
  const heartbeatPromise = heartbeatCollector.waitFor((event) => {
    const heartbeat = heartbeatMessageFromEvent(event);
    return heartbeat?.provider === provider
      && heartbeat?.enclave_id === enclaveId
      && Number(heartbeat?.slots?.active || 0) >= sessionCount
      && Number(heartbeat?.perf?.ttft_ms || 0) > 0;
  }, timeoutMs);
  const streamControllers = prompts.map(() => new AbortController());
  const streams = prompts.map((prompt, index) => runStreamingChatSmoke(gatewayUrl, modelId, runDir, {
    timeoutMs,
    maxTokens,
    prompt,
    fileStem: `gateway-concurrent-${index + 1}`,
    allowTransportErrorAfterContent: true,
    signal: streamControllers[index].signal,
    minAttTier,
  }));
  const firstStreamPromise = firstFulfilled(streams);
  const streamResultsPromise = Promise.allSettled(streams);

  let heartbeatEvent = null;
  let heartbeatError = null;
  let firstStream = null;
  let firstStreamError = null;
  try {
    heartbeatEvent = await heartbeatPromise;
  } catch (err) {
    heartbeatError = err;
  }
  try {
    firstStream = await firstStreamPromise;
  } catch (err) {
    firstStreamError = err;
  }
  if (abortAfterProof && heartbeatEvent && firstStream) {
    streamControllers.forEach((controller, index) => {
      if (index !== firstStream.index) controller.abort(new Error('concurrent heartbeat proof captured'));
    });
  }
  const streamResults = await streamResultsPromise;
  const successfulStreams = streamResults
    .map((result, index) => ({ result, index }))
    .filter(({ result }) => result.status === 'fulfilled');
  if (successfulStreams.length === 0) {
    if (firstStreamError) throw firstStreamError;
    fail('concurrent heartbeat proof captured no completed stream');
  }
  if (heartbeatError) {
    const observed = heartbeatCollector.events
      .map(heartbeatMessageFromEvent)
      .filter(Boolean)
      .map((heartbeat) => ({
        provider: heartbeat.provider,
        enclave_id: heartbeat.enclave_id,
        seq: heartbeat.seq,
        slots: heartbeat.slots,
        q: heartbeat.q,
        perf: heartbeat.perf,
      }));
    fail(`did not observe slots.active >= ${sessionCount} heartbeat with ttft_ms; observed ${JSON.stringify(observed.slice(-8))}`);
  }

  const heartbeat = heartbeatMessageFromEvent(heartbeatEvent);
  let measuredPerfEvent = null;
  try {
    measuredPerfEvent = await heartbeatCollector.waitFor((event) => {
      const candidate = heartbeatMessageFromEvent(event);
      return candidate?.provider === provider
        && candidate?.enclave_id === enclaveId
        && candidate?.perf?.tok_s_source === 'measured'
        && Number(candidate?.perf?.tok_s || 0) > 0;
    }, Math.min(timeoutMs, 120_000));
  } catch (err) {
    const observed = heartbeatCollector.events
      .map(heartbeatMessageFromEvent)
      .filter(Boolean)
      .map((candidate) => ({
        seq: candidate.seq,
        slots: candidate.slots,
        q: candidate.q,
        perf: candidate.perf,
      }));
    fail(`did not observe measured tok_s heartbeat after concurrent run: ${err.message}; observed ${JSON.stringify(observed.slice(-8))}`);
  }
  let idleEvent = null;
  try {
    idleEvent = await heartbeatCollector.waitFor((event) => {
      const candidate = heartbeatMessageFromEvent(event);
      return candidate?.provider === provider
        && candidate?.enclave_id === enclaveId
        && Number(candidate?.slots?.active || 0) === 0
        && Number(candidate?.q?.depth || 0) === 0;
    }, Math.min(timeoutMs, 120_000));
  } catch (err) {
    const observed = heartbeatCollector.events
      .map(heartbeatMessageFromEvent)
      .filter(Boolean)
      .map((candidate) => ({
        seq: candidate.seq,
        slots: candidate.slots,
        q: candidate.q,
        perf: candidate.perf,
      }));
    fail(`did not observe idle heartbeat decay after concurrent run: ${err.message}; observed ${JSON.stringify(observed.slice(-8))}`);
  }
  const summary = {
    ok: true,
    requested_sessions: sessionCount,
    heartbeat,
    measured_perf_heartbeat: heartbeatMessageFromEvent(measuredPerfEvent),
    idle_heartbeat: heartbeatMessageFromEvent(idleEvent),
    heartbeat_event: heartbeatEvent,
    streams: streamResults.map((result, index) => result.status === 'fulfilled'
      ? result.value
      : {
          ok: false,
          index: index + 1,
          tolerated_after_heartbeat: Boolean(abortAfterProof && heartbeatEvent && successfulStreams.length > 0),
          error: result.reason?.message || String(result.reason),
        }),
    completed_streams: successfulStreams.length,
    heartbeat_events_path: path.relative(ROOT, heartbeatEventsPath),
  };
  writeJson(path.join(runDir, 'gateway-concurrent-heartbeat.json'), summary);
  return summary;
}

async function runGuidedToolCallSmoke(gatewayUrl, modelId, runDir, { timeoutMs, minAttTier = null }) {
  const rawPath = path.join(runDir, 'gateway-tool-call-response.json');
  const summaryPath = path.join(runDir, 'gateway-tool-call.json');
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(new Error('guided tool-call timed out')), timeoutMs);
  const body = {
    model: modelId,
    stream: false,
    temperature: 0,
    max_tokens: 96,
    messages: [
      {
        role: 'user',
        content: 'Use the lookup_room tool for room alpha. Return only the tool call.',
      },
    ],
    tools: [
      {
        type: 'function',
        function: {
          name: 'lookup_room',
          description: 'Look up a Mayhem room by name.',
          parameters: {
            type: 'object',
            additionalProperties: false,
            required: ['room'],
            properties: {
              room: { type: 'string' },
            },
          },
        },
      },
    ],
    tool_choice: { type: 'function', function: { name: 'lookup_room' } },
  };
  let responseBody;
  try {
    const response = await fetch(`${gatewayUrl}/v1/chat/completions`, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        ...(minAttTier ? { 'X-Mayhem-Min-Att-Tier': String(minAttTier) } : {}),
      },
      body: JSON.stringify(body),
      signal: controller.signal,
    });
    responseBody = await response.text();
    if (!response.ok) fail(`guided tool-call returned HTTP ${response.status}: ${responseBody}`);
  } finally {
    clearTimeout(timer);
  }
  fs.writeFileSync(rawPath, responseBody);
  const parsed = JSON.parse(responseBody);
  const choice = Array.isArray(parsed.choices) ? parsed.choices[0] : null;
  const toolCall = choice?.message?.tool_calls?.[0];
  const argsRaw = toolCall?.function?.arguments;
  let args = null;
  if (typeof argsRaw === 'string') {
    try {
      args = JSON.parse(argsRaw);
    } catch {
      args = null;
    }
  } else if (argsRaw && typeof argsRaw === 'object') {
    args = argsRaw;
  }
  if (choice?.finish_reason !== 'tool_calls') {
    fail(`guided tool-call did not finish with tool_calls: ${JSON.stringify(choice)}`);
  }
  if (toolCall?.function?.name !== 'lookup_room') {
    fail(`guided tool-call selected wrong tool: ${JSON.stringify(toolCall)}`);
  }
  if (!args || typeof args.room !== 'string' || args.room.length === 0) {
    fail(`guided tool-call arguments were not valid JSON with a room: ${argsRaw}`);
  }
  const summary = {
    ok: true,
    model: modelId,
    min_att_tier: minAttTier,
    finish_reason: choice.finish_reason,
    tool_name: toolCall.function.name,
    arguments: args,
    raw_path: path.relative(ROOT, rawPath),
  };
  writeJson(summaryPath, summary);
  return { ...summary, summary_path: path.relative(ROOT, summaryPath) };
}

async function readStateValue(rpcUrl, key) {
  const response = await fetch(`${rpcUrl.replace(/\/$/, '')}/state?key=${encodeURI(key)}`);
  if (!response.ok) fail(`state read ${key} returned HTTP ${response.status}: ${await response.text()}`);
  const body = await response.json();
  if (Object.prototype.hasOwnProperty.call(body, 'value')) return body.value;
  if (body.result && Object.prototype.hasOwnProperty.call(body.result, 'value')) return body.result.value;
  return body.result ?? body;
}

async function readStatePrefix(rpcUrl, prefix, limit = 500) {
  const url = `${rpcUrl.replace(/\/$/, '')}/state?prefix=${encodeURIComponent(prefix)}&confirmed=false&limit=${limit}`;
  const response = await fetch(url);
  if (!response.ok) fail(`state prefix ${prefix} returned HTTP ${response.status}: ${await response.text()}`);
  const body = await response.json();
  return body.values || body.result?.values || [];
}

async function readPriceSchedulesForEnclave(rpcUrl, enclaveId) {
  const direct = await readStateValue(rpcUrl, `price/${enclaveId}`).catch(() => null);
  const prefixed = await readStatePrefix(rpcUrl, `price/${enclaveId}/`).catch(() => []);
  const schedules = [];
  if (direct?.current || direct?.pending) {
    schedules.push({ key: `price/${enclaveId}`, value: direct });
  }
  for (const entry of prefixed) {
    const key = entry?.key || '';
    const value = entry?.value || entry;
    if (!key.startsWith(`price/${enclaveId}/`)) continue;
    if (key.includes('/v/')) continue;
    if (!value?.current && !value?.pending) continue;
    schedules.push({ key, value });
  }
  return schedules;
}

async function waitForStateValue(rpcUrl, key, predicate, timeoutMs, label) {
  const deadline = Date.now() + timeoutMs;
  let last = null;
  while (Date.now() < deadline) {
    try {
      last = await readStateValue(rpcUrl, key);
      if (predicate(last)) return last;
    } catch (err) {
      last = { error: err.message };
    }
    await sleep(500);
  }
  fail(`timed out waiting for ${label || key}: ${JSON.stringify(last)}`);
}

async function waitForCanonicalEnclaveReady(rpcUrl, { enclaveId, modelId, backend }, timeoutMs = 60_000) {
  const deadline = Date.now() + timeoutMs;
  let last = {};
  while (Date.now() < deadline) {
    const [enclave, priceSchedules, rooms] = await Promise.all([
      readStateValue(rpcUrl, `enclave/${enclaveId}`).catch((err) => ({ error: err.message })),
      readPriceSchedulesForEnclave(rpcUrl, enclaveId).catch((err) => [{ error: err.message }]),
      readStatePrefix(rpcUrl, 'room/').catch((err) => [{ error: err.message }]),
    ]);
    const priceEntry = priceSchedules.find((entry) => entry?.value?.current);
    const price = priceEntry?.value || null;
    const roomEntry = rooms.find((entry) => {
      const value = entry?.value || entry;
      return value?.enclave_id === enclaveId && value?.model_id === modelId && value?.status === 'open';
    });
    last = {
      enclave_status: enclave?.status,
      enclave_backend: enclave?.backend,
      price_current: Boolean(price?.current),
      price_key: priceEntry?.key || null,
      room_id: roomEntry?.key?.replace(/^room\//, '') || roomEntry?.value?.room_id,
      errors: [enclave?.error, priceSchedules.find((entry) => entry?.error)?.error, rooms.find((entry) => entry?.error)?.error].filter(Boolean),
    };
    if (
      enclave?.status === 'active' &&
      enclave?.model_id === modelId &&
      enclave?.backend === backend &&
      price?.current &&
      roomEntry
    ) {
      return {
        enclave,
        price,
        room: roomEntry.value || roomEntry,
        checked_at_ms: Date.now(),
      };
    }
    await sleep(500);
  }
  fail(`timed out waiting for canonical ${backend} enclave readiness: ${JSON.stringify(last)}`);
}

function safeCount(value, label) {
  const number = Number(value ?? 0);
  if (!Number.isSafeInteger(number) || number < 0) fail(`${label} is not a non-negative safe integer: ${value}`);
  return number;
}

function parseAu(value, label) {
  const raw = String(value ?? '0');
  if (!/^(0|[1-9]\d*)$/.test(raw)) fail(`${label} is not a canonical non-negative atto-USD integer: ${value}`);
  return BigInt(raw);
}

function auString(value, label) {
  return parseAu(value, label).toString();
}

function auDelta(left, right, label) {
  const delta = parseAu(left, `${label} left`) - parseAu(right, `${label} right`);
  return delta.toString();
}

async function readDepositRootSnapshot(rpcUrl, epoch) {
  const value = await readStateValue(rpcUrl, `ev/dep/${epoch}`);
  if (!value) return null;
  if (value.type !== 'deposit_root') fail(`ev/dep/${epoch} is not a deposit root`);
  return {
    merkle_root: value.merkle_root,
    count: safeCount(value.count, `ev/dep/${epoch} count`),
    au_total: auString(value.au_total, `ev/dep/${epoch} au_total`),
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
    balance_au: auString(balance?.au, 'fiat balance au'),
    provider_total_au: auString(earning?.total_au, 'fiat provider total_au'),
    fee_cum_au: auString(fee?.cum_au, 'fiat fee cum_au'),
  };
}

async function waitForLedgerMovement(rpcUrl, user, provider, before, expected, timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs;
  let latest = null;
  while (Date.now() < deadline) {
    latest = await readLedgerSnapshot(rpcUrl, user, provider);
    const actual = {
      debit_au: auDelta(before.balance_au, latest.balance_au, 'fiat debit au'),
      provider_net_au: auDelta(latest.provider_total_au, before.provider_total_au, 'fiat provider net au'),
      fee_au: auDelta(latest.fee_cum_au, before.fee_cum_au, 'fiat fee au'),
    };
    if (
      actual.debit_au === expected.debit_au &&
      actual.provider_net_au === expected.provider_net_au &&
      actual.fee_au === expected.fee_au
    ) {
      return { after: latest, actual };
    }
    await sleep(500);
  }
  latest = latest || await readLedgerSnapshot(rpcUrl, user, provider);
  return {
    after: latest,
    actual: {
      debit_au: auDelta(before.balance_au, latest.balance_au, 'fiat debit au'),
      provider_net_au: auDelta(latest.provider_total_au, before.provider_total_au, 'fiat provider net au'),
      fee_au: auDelta(latest.fee_cum_au, before.fee_cum_au, 'fiat fee au'),
    },
  };
}

function runJsonCommandToFile(command, args, outPath, options = {}) {
  const stdout = runSync(command, args, options);
  fs.writeFileSync(outPath, stdout);
  return readJson(outPath);
}

async function settleGatewayReceiptEpoch({ mayhemBin, adminHome, adminRpcUrl, receiptsPath, runDir, user, provider, epoch = 1, feeBps = 1500 }) {
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
  if (parseAu(recomputed?.totals?.use_au, 'recomputed use_au') <= 0n) fail('epoch recompute produced zero usage');
  const providerEarning = (recomputed.earnings || []).find((entry) => entry.rail === 'fiat' && entry.provider === provider);
  if (!providerEarning) fail(`epoch recompute did not include fiat earnings for provider ${provider}`);
  const expectedDebitAu = auString(recomputed.totals.use_au, 'expected debit au');
  const providerGrossAu = parseAu(providerEarning.gross_au, 'provider gross_au');
  const expectedProviderNetAu = (
    providerGrossAu - ((providerGrossAu * BigInt(feeBps)) / 10_000n)
  ).toString();
  const expectedFeeAu = auString(recomputed.totals.fee_au, 'expected fee_au');

  const adminEpochCommon = [
    '--home', adminHome,
    '--peer-store-name', 'admin',
    '--rpc-url', adminRpcUrl,
    '--recomputed-file', recomputedPath,
    '--at', String(epoch),
    '--submit',
    '--json',
  ];
  const commitSim = runJsonCommandToFile(mayhemBin, ['admin', 'epoch-commit', ...adminEpochCommon, '--sim'], commitSimPath);
  const commit = runJsonCommandToFile(mayhemBin, ['admin', 'epoch-commit', ...adminEpochCommon], commitPath);
  const applySim = runJsonCommandToFile(mayhemBin, ['admin', 'epoch-apply', ...adminEpochCommon, '--sim'], applySimPath);
  const apply = runJsonCommandToFile(mayhemBin, ['admin', 'epoch-apply', ...adminEpochCommon], applyPath);

  const expected = { debit_au: expectedDebitAu, provider_net_au: expectedProviderNetAu, fee_au: expectedFeeAu };
  const { after, actual } = await waitForLedgerMovement(adminRpcUrl, user, provider, before, expected);
  if (actual.debit_au !== expected.debit_au) fail(`fiat balance debit mismatch; expected ${expected.debit_au}, got ${actual.debit_au}`);
  if (actual.provider_net_au !== expected.provider_net_au) fail(`fiat provider earning mismatch; expected ${expected.provider_net_au}, got ${actual.provider_net_au}`);
  if (actual.fee_au !== expected.fee_au) fail(`fiat fee movement mismatch; expected ${expected.fee_au}, got ${actual.fee_au}`);

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
    reports: { export: exportReport, commit_sim: commitSim, commit, apply_sim: applySim, apply },
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
        const result = spawnSync(socketfilterfw, [action, app], { cwd: ROOT, encoding: 'utf8' });
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

async function main() {
  if (process.argv.includes('--help') || process.argv.includes('-h')) {
    usage();
    return;
  }

  const tag = process.env.MAYHEM_E11_TAG || new Date().toISOString().replace(/[-:]/g, '').replace(/\..*/, '');
  const runDir = path.join(ROOT, '.mayhem-local/i3-e11-spark-trtllm', tag);
  const logsDir = path.join(runDir, 'logs');
  await fsp.mkdir(logsDir, { recursive: true });
  log(`run dir: ${path.relative(ROOT, runDir)}`);

  const mayhemBin = path.join(ROOT, 'target/debug/mayhem');
  const enclaveBin = path.join(ROOT, 'target/debug/mayhem-enclave');
  const clusterFile = path.resolve(
    ROOT,
    process.env.MAYHEM_E11_CLUSTER_FILE || '.mayhem-local/secrets/cluster.txt'
  );
  const hfTokenFile = path.resolve(
    ROOT,
    process.env.MAYHEM_E11_HF_TOKEN_FILE || '.mayhem-local/secrets/hf.txt'
  );
  const remote = parseCluster(clusterFile);
  const localTmpDir = path.join(ROOT, '.mayhem-local/tmp');
  await fsp.mkdir(localTmpDir, { recursive: true });
  const passFile = remote.auth === 'password'
    ? path.join(localTmpDir, `spark-e11-${tag}-${process.pid}.pass`)
    : null;
  if (passFile) fs.writeFileSync(passFile, remote.pass, { mode: 0o600 });
  cleanupState.remote = remote;
  cleanupState.passFile = passFile;
  cleanupState.remoteRun = null;

  const catalog = readJson(path.join(ROOT, 'catalog/models.json'));
  const model = catalog.models.find((entry) => entry.model_id === MODEL_ID);
  if (!model) fail(`catalog model not found: ${MODEL_ID}`);
  const accelArtifactName = process.env.MAYHEM_E11_ACCEL_ARTIFACT || DEFAULT_ACCEL_ARTIFACT;
  const accelArtifact = model.artifacts?.[accelArtifactName];
  const ggufArtifact = model.artifacts?.[GGUF_ARTIFACT];
  if (!isSupportedSparkAccelerator(accelArtifact)) {
    fail(`missing supported Spark accelerated artifact ${accelArtifactName}: ${JSON.stringify(accelArtifact)}`);
  }
  if (!ggufArtifact || ggufArtifact.engine !== 'llama.cpp') fail(`missing ${GGUF_ARTIFACT} llama.cpp artifact`);
  const accelBackend = accelArtifact.engine;
  const accelLabel = `${accelArtifactName}/${accelBackend}`;
  const accelAttTier = envPositiveInt('MAYHEM_E11_ACCEL_ATT_TIER', 1);
  if (accelAttTier > 3) fail(`MAYHEM_E11_ACCEL_ATT_TIER must be between 1 and 3, got ${accelAttTier}`);
  const requestMinAttTierRaw = process.env.MAYHEM_E11_REQUEST_MIN_ATT_TIER || '';
  const requestMinAttTier = requestMinAttTierRaw
    ? envPositiveInt('MAYHEM_E11_REQUEST_MIN_ATT_TIER', 1)
    : null;
  if (requestMinAttTier !== null && requestMinAttTier > 4) fail(`MAYHEM_E11_REQUEST_MIN_ATT_TIER must be between 1 and 4, got ${requestMinAttTier}`);
  const accelLaunchMeasurementsJson = process.env.MAYHEM_E11_ACCEL_LAUNCH_MEASUREMENTS_JSON || '';
  if (accelAttTier >= 3 && !accelLaunchMeasurementsJson) {
    fail('MAYHEM_E11_ACCEL_LAUNCH_MEASUREMENTS_JSON is required when MAYHEM_E11_ACCEL_ATT_TIER is 3');
  }
  const providerHwQuoteKind = process.env.MAYHEM_E11_PROVIDER_HW_QUOTE_KIND || '';
  const providerHwQuoteCommand = process.env.MAYHEM_E11_PROVIDER_HW_QUOTE_COMMAND || '';
  if (Boolean(providerHwQuoteKind) !== Boolean(providerHwQuoteCommand)) {
    fail('MAYHEM_E11_PROVIDER_HW_QUOTE_KIND and MAYHEM_E11_PROVIDER_HW_QUOTE_COMMAND must be supplied together');
  }
  if (accelAttTier >= 3 && !providerHwQuoteKind) {
    fail('Tier-3 smoke requires MAYHEM_E11_PROVIDER_HW_QUOTE_*');
  }
  const rateMapJson = textRateMapJson(model);
  const accelRateMapJson = process.env.MAYHEM_E11_ACCEL_RATE_MAP_JSON
    || (accelAttTier >= 3
      ? scaleRateMapJson(rateMapJson, envPositiveInt('MAYHEM_E11_ACCEL_PRICE_MULTIPLIER', 2))
      : rateMapJson);
  const manifestHash = sha256File(path.join(ROOT, 'catalog/models.json'));
  const catalogReleaseRepo = process.env.MAYHEM_E11_CATALOG_RELEASE_REPO || DEFAULT_CATALOG_RELEASE_REPO;
  const catalogReleaseRevision = process.env.MAYHEM_E11_CATALOG_RELEASE_REVISION || DEFAULT_CATALOG_RELEASE_REVISION;
  if (!/^[0-9a-f]{40}$/i.test(catalogReleaseRevision)) {
    fail(`MAYHEM_E11_CATALOG_RELEASE_REVISION must be a pinned 40-hex revision, got ${catalogReleaseRevision}`);
  }
  const catalogReleaseBaseUrl = `https://huggingface.co/${catalogReleaseRepo}/resolve/${catalogReleaseRevision}`;

  log('building local mayhem CLI/gateway/enclave crates');
  runSync('cargo', ['build', '-q', '-p', 'mayhem-cli', '-p', 'mayhem-gateway', '-p', 'mayhem-enclave'], {
    env: { ...process.env, CARGO_PROFILE_DEV_OPT_LEVEL: process.env.CARGO_PROFILE_DEV_OPT_LEVEL || '3', CARGO_PROFILE_DEV_DEBUG: process.env.CARGO_PROFILE_DEV_DEBUG || '0' },
  });
  const localBinaryHash = runSync(enclaveBin, ['measure-binary', '--binary', mayhemBin]).trim().replace(/^binary_hash=/, '');

  const remoteHome = (await ssh(remote, passFile, 'printf "%s\\n" "$HOME"')).stdout.trim();
  const remoteRoot = (process.env.MAYHEM_E11_REMOTE_ROOT || '$HOME/mayhem/i3-e11-openmayhem')
    .replace(/^~(?=\/|$)/, remoteHome)
    .replace('$HOME', remoteHome);
  const remoteTargetDir = (process.env.MAYHEM_E11_REMOTE_TARGET_DIR || path.posix.join(remoteRoot, 'target'))
    .replace(/^~(?=\/|$)/, remoteHome)
    .replace('$HOME', remoteHome);
  const remoteDownloads = (process.env.MAYHEM_E11_REMOTE_DOWNLOADS || '$HOME/mayhem/e11-provider-downloads')
    .replace(/^~(?=\/|$)/, remoteHome)
    .replace('$HOME', remoteHome);
  const remoteProviderCacheRoot = (process.env.MAYHEM_E11_REMOTE_PROVIDER_CACHE || path.posix.join(remoteRoot, '.mayhem-local/i3-e11-spark-provider-cache'))
    .replace(/^~(?=\/|$)/, remoteHome)
    .replace('$HOME', remoteHome);
  cleanupState.remoteProviderCacheRoot = remoteProviderCacheRoot;
  const remoteRun = path.posix.join(remoteRoot, '.mayhem-local/i3-e11-spark-trtllm', tag);
  cleanupState.remoteRun = remoteRun;
  const remoteLogs = path.posix.join(remoteRun, 'logs');
  const remoteProviderHome = path.posix.join(remoteProviderCacheRoot, 'provider-home');
  const remoteKeypair = path.posix.join(remoteProviderHome, 'stores/main/db/keypair.json');
  const remoteHfToken = path.posix.join(remoteRun, 'secrets/hf.txt');
  const remoteMayhem = path.posix.join(remoteTargetDir, 'debug/mayhem');
  const remoteEnclave = path.posix.join(remoteTargetDir, 'debug/mayhem-enclave');
  const remotePear = path.posix.join(
    remoteHome,
    '.config/pear/current/by-arch/linux-arm64/bin/pear-runtime'
  );
  const remoteEnvPrelude = process.env.MAYHEM_E11_REMOTE_ENV_PRELUDE || '';
  const remoteVllmEnvPrelude = process.env.MAYHEM_E11_VLLM_ENV_PRELUDE || '';
  const remoteShell = (body) => remoteEnvPrelude.trim()
    ? `bash -lc ${sh(`${remoteEnvPrelude}\n${body}`)}`
    : body;
  const remoteNode = process.env.MAYHEM_E11_NODE_BIN
    || (await ssh(remote, passFile, remoteShell('command -v node || true'))).stdout.trim()
    || '/usr/bin/node';
  const remoteNpm = process.env.MAYHEM_E11_NPM_BIN
    || (await ssh(remote, passFile, remoteShell('command -v npm || true'))).stdout.trim()
    || 'npm';
  const remoteTrtPython = (process.env.MAYHEM_E11_TRTLLM_PYTHON || '$HOME/mayhem/bin/trtllm-python')
    .replace(/^~(?=\/|$)/, remoteHome)
    .replace('$HOME', remoteHome);
  const remoteVllmPython = (process.env.MAYHEM_E11_VLLM_PYTHON || '$HOME/mayhem/bin/vllm-python-e14')
    .replace(/^~(?=\/|$)/, remoteHome)
    .replace('$HOME', remoteHome);

  log('syncing current source tree to Spark');
  await ssh(remote, passFile, `mkdir -p ${sh(remoteRoot)} ${sh(remoteLogs)} ${sh(remoteDownloads)} ${sh(remoteProviderHome)}`);
  await rsyncRepoTo(remote, passFile, `${ROOT}/`, `${remoteRoot}/`, {
    logFile: path.join(logsDir, 'rsync-source.log'),
    timeoutMs: 20 * 60_000,
    env: process.env,
  });
  const syncNodeModules = !/^(0|false|no)$/i.test(process.env.MAYHEM_E11_SYNC_NODE_MODULES || '1');
  const remoteHasWalletDeps = (await ssh(
    remote,
    passFile,
    `test -d ${sh(path.posix.join(remoteRoot, 'intercom/node_modules/trac-wallet'))} && test -d ${sh(path.posix.join(remoteRoot, 'intercom/node_modules/trac-crypto-api'))} && test -d ${sh(path.posix.join(remoteRoot, 'intercom/node_modules/ethereum-cryptography'))} && echo yes || true`
  )).stdout.trim() === 'yes';
  if (syncNodeModules && !remoteHasWalletDeps) {
    log('syncing intercom/node_modules to Spark for wallet-helper signing');
    await ssh(remote, passFile, `mkdir -p ${sh(path.posix.join(remoteRoot, 'intercom/node_modules'))}`);
    await rsyncTo(remote, passFile, `${ROOT}/intercom/node_modules/`, `${path.posix.join(remoteRoot, 'intercom/node_modules')}/`, {
      logFile: path.join(logsDir, 'rsync-node-modules.log'),
      timeoutMs: 30 * 60_000,
      env: process.env,
    });
  }
  const remoteHasEthers = (await ssh(
    remote,
    passFile,
    remoteShell(`cd ${sh(remoteRoot)}\n${sh(remoteNode)} -e "const req=require('module').createRequire(process.cwd() + '/contracts/package.json'); req.resolve('ethers'); console.log('yes')" 2>/dev/null || true`)
  )).stdout.trim() === 'yes';
  if (syncNodeModules && !remoteHasEthers) {
    log('installing remote contracts dependencies for wallet-helper signing');
    await ssh(
      remote,
      passFile,
      remoteShell(`cd ${sh(path.posix.join(remoteRoot, 'contracts'))}\n${sh(remoteNpm)} install --no-save --omit=optional --ignore-scripts ethers@6.15.0`),
      { logFile: path.join(logsDir, 'remote-contracts-npm-ci.log'), timeoutMs: 20 * 60_000 }
    );
  }

  log('building remote Spark mayhem binary');
  await ssh(
    remote,
    passFile,
    remoteShell([
      `cd ${sh(remoteRoot)}`,
      `mkdir -p ${sh(remoteTargetDir)}`,
      `export CARGO_TARGET_DIR=${sh(remoteTargetDir)}`,
      'export PATH="$HOME/.cargo/bin:$PATH"',
      'export CARGO_PROFILE_DEV_OPT_LEVEL="${CARGO_PROFILE_DEV_OPT_LEVEL:-3}"',
      'export CARGO_PROFILE_DEV_DEBUG="${CARGO_PROFILE_DEV_DEBUG:-0}"',
      'cargo build -q -p mayhem-cli -p mayhem-enclave',
    ].join('\n')),
    { logFile: path.join(logsDir, 'remote-cargo-build.log'), timeoutMs: 30 * 60_000 }
  );
  const remoteBinaryHash = (await ssh(
    remote,
    passFile,
    remoteShell(`cd ${sh(remoteRoot)}\n${sh(remoteEnclave)} measure-binary --binary ${sh(remoteMayhem)}`)
  )).stdout.trim().replace(/^binary_hash=/, '');

  log('starting local Pear dev-net');
  const localNode = process.execPath;
  const localPear = path.join(os.homedir(), 'Library/Application Support/pear/current/by-arch/darwin-arm64/bin/pear-runtime');
  await preauthorizeLocalApps([localNode, localPear, mayhemBin], path.join(logsDir, 'local-firewall.log'));
  const devnetLog = path.join(logsDir, 'dev-net.log');
  const subnetChannel = `mayhem-e11-${tag}`;
  const devnetArgs = ['scripts/dev-net.sh', '--keep-running'];
  if (envFlag('MAYHEM_E11_DEVNET_CLEANUP', true)) devnetArgs.splice(1, 0, '--cleanup');
  const devnet = spawnLogged('bash', devnetArgs, devnetLog, {
    env: {
      ...process.env,
      MAYHEM_DEVNET_JOINERS: '2',
      MAYHEM_DEVNET_SUBNET_CHANNEL: subnetChannel,
      MAYHEM_DEVNET_PRESERVE_KEYPAIRS: '1',
      MAYHEM_DEVNET_REPLICATE_FLUSH_TIMEOUT_MS: '5000',
      SC_BRIDGE_DEBUG: '1',
      SESSION_DEBUG: '1',
    },
  });
  cleanupState.localChildren.push(devnet);
  cleanupState.devnetLog = devnetLog;
  await waitForFilePattern(devnetLog, /Mayhem dev-net ready\./, 180_000, 'local dev-net', async () => devnet.exitCode === null);
  const devnetText = fs.readFileSync(devnetLog, 'utf8');
  const adminWs = devnetText.match(/admin:\s+(ws:\/\/[^\s]+).*rpc=(http:\/\/[^\s]+)/);
  const joinerAWs = devnetText.match(/joiner-a:\s+(ws:\/\/[^\s]+).*rpc=(http:\/\/[^\s]+)/);
  const joinerBWs = devnetText.match(/joiner-b:\s+(ws:\/\/[^\s]+).*rpc=(http:\/\/[^\s]+)/);
  const token = devnetText.match(/sc bridge token:\s+(\S+)/);
  const logs = devnetText.match(/logs:\s+(\S+)/);
  if (!adminWs || !joinerAWs || !joinerBWs || !token || !logs) fail(`could not parse dev-net output in ${devnetLog}`);
  const adminRpcUrl = adminWs[2];
  const adminRpcPort = new URL(adminRpcUrl).port;
  const joinerBPort = new URL(joinerBWs[1]).port;
  const scToken = token[1];
  const adminLog = fs.readFileSync(path.join(logs[1], 'admin.log'), 'utf8');
  const joinerALog = fs.readFileSync(path.join(logs[1], 'joiner-a.log'), 'utf8');
  const joinerBLog = fs.readFileSync(path.join(logs[1], 'joiner-b.log'), 'utf8');
  const adminPubkey = adminLog.match(/Peer pubkey \(hex\):\s+([0-9a-f]{64})/i)?.[1];
  const userPubkey = joinerALog.match(/Peer pubkey \(hex\):\s+([0-9a-f]{64})/i)?.[1];
  const providerPeerPubkey = joinerBLog.match(/Peer pubkey \(hex\):\s+([0-9a-f]{64})/i)?.[1];
  if (!adminPubkey || !userPubkey || !providerPeerPubkey) fail('could not parse dev-net peer pubkeys');

  const accelEnclaveId = await catalogEnclaveId({
    adminPubkey,
    modelId: MODEL_ID,
    artifactRoot: accelArtifact.artifact_root,
    artifactSidecarRoots: artifactSidecarRoots(accelArtifact),
    manifestHash,
    binaryHash: remoteBinaryHash,
  });
  const ggufEnclaveId = await catalogEnclaveId({
    adminPubkey,
    modelId: MODEL_ID,
    artifactRoot: ggufArtifact.artifact_root,
    manifestHash,
    binaryHash: localBinaryHash,
  });

  log(`seeding admin rules, prices, GGUF and ${accelLabel} enclaves`);
  const adminHome = path.join(runDir, 'admin-home');
  await fsp.mkdir(path.join(adminHome, 'stores'), { recursive: true });
  await fsp.rm(path.join(adminHome, 'stores/admin'), { recursive: true, force: true });
  await fsp.symlink(path.join(ROOT, 'intercom/stores/mayhem-devnet-admin'), path.join(adminHome, 'stores/admin'));
  const adminCommon = ['--home', adminHome, '--peer-store-name', 'admin', '--rpc-url', adminRpcUrl, '--submit', '--json'];
  const rulesHash = JSON.parse(runSync(mayhemBin, ['rules', 'hash', '--print-json'])).hash;
  const adminReports = {};
  const adminRun = (name, args) => {
    const outPath = path.join(runDir, `${name}.json`);
    const stdout = runSync(mayhemBin, ['admin', ...args, ...adminCommon]);
    fs.writeFileSync(outPath, stdout);
    adminReports[name] = readJson(outPath);
    return adminReports[name];
  };
  adminRun('admin-set-rules', ['set-rules', '--ver', '1', '--hash', rulesHash]);
  adminRun('admin-set-params', [
    'set-params',
    '--submitted-at', '0',
    '--effective-at', '86400',
    '--values-json', '{"fee_bps":1500,"holdback_epochs":0,"challenge_epochs":0,"payout_min_au":"0","rate_staleness_seconds":86400,"canary_match_min_bps":9000,"probe_reward_au":"5000"}',
  ]);
  adminRun('admin-publish-catalog', [
    'publish-catalog',
    '--catalog-url', `${catalogReleaseBaseUrl}/models.json`,
    '--signature-url', `${catalogReleaseBaseUrl}/models.json.sig`,
    '--canaries-base-url', `${catalogReleaseBaseUrl}/canaries`,
  ]);
  adminRun('admin-set-model-ref', ['set-model-ref', '--model', MODEL_ID, '--rate-map-json', rateMapJson]);
  adminRun('admin-register-gguf-enclave', [
    'register-enclave',
    '--enclave-id', ggufEnclaveId,
    '--model', MODEL_ID,
    '--backend', 'llama.cpp',
    '--artifact-root', ggufArtifact.artifact_root,
    '--artifact-root-kind', ggufArtifact.artifact_root_kind,
    '--artifact-repo', ggufArtifact.source.repo,
    '--artifact-revision', ggufArtifact.source.revision,
    '--artifact-path', ggufArtifact.path,
    '--source-sha256', ggufArtifact.source_sha256,
    '--catalog-path', path.join(ROOT, 'catalog/models.json'),
    '--manifest-hash', manifestHash,
    '--binary-hash', localBinaryHash,
    '--caps-json', '{"chat":true,"tools":true,"json":true,"ctx":1024}',
  ]);
  await waitForStateValue(
    adminRpcUrl,
    `enclave/${ggufEnclaveId}`,
    (value) => value?.status === 'active' && value?.backend === 'llama.cpp',
    180_000,
    'admin-created GGUF enclave'
  );
  adminRun('admin-set-gguf-price', ['set-price', '--enclave-id', ggufEnclaveId, '--rate-map-json', rateMapJson, '--effective-at', '0']);
  adminRun('admin-open-gguf-room', ['open-room', '--enclave-id', ggufEnclaveId, '--model', MODEL_ID, '--nonce', `e11-gguf-${tag}`, '--label', 'i3-e11-gguf-fallback']);
  adminRun('admin-register-accelerated-enclave', [
    'register-enclave',
    '--enclave-id', accelEnclaveId,
    '--model', MODEL_ID,
    '--backend', accelBackend,
    '--artifact-root', accelArtifact.artifact_root,
    '--artifact-root-kind', accelArtifact.artifact_root_kind,
    '--artifact-repo', accelArtifact.source.repo,
    '--artifact-revision', accelArtifact.source.revision,
    '--artifact-path', accelArtifact.path,
    '--source-sha256', accelArtifact.source_sha256,
    '--catalog-path', path.join(ROOT, 'catalog/models.json'),
    '--manifest-hash', manifestHash,
    '--binary-hash', remoteBinaryHash,
    '--att-tier', String(accelAttTier),
    ...(accelLaunchMeasurementsJson ? ['--launch-measurements-json', accelLaunchMeasurementsJson] : []),
    '--caps-json', acceleratedCapsJson(accelBackend),
  ]);
  await waitForStateValue(
    adminRpcUrl,
    `enclave/${accelEnclaveId}`,
    (value) => value?.status === 'active' && value?.backend === accelBackend,
    180_000,
    `admin-created ${accelLabel} enclave`
  );
  adminRun('admin-set-accelerated-price', ['set-price', '--enclave-id', accelEnclaveId, '--rate-map-json', accelRateMapJson, '--effective-at', '0']);
  adminRun('admin-open-accelerated-room', [
    'open-room',
    '--enclave-id', accelEnclaveId,
    '--model', MODEL_ID,
    '--nonce', `e11-${accelBackend}-${tag}`,
    '--label', `i3-e11-${accelBackend}-spark`,
  ]);
  const fiatDepositRef = (await b3(Buffer.from(`i3-e11-fiat-credit:${tag}:${userPubkey}`, 'utf8'))).toString('hex');
  adminRun('admin-fiat-deposit', [
    'fiat-deposit',
    '--rail', 'stripe',
    '--who', userPubkey,
    '--au', '10000000000000000000',
    '--ext-ref-hash', fiatDepositRef,
    '--fiat-currency', 'usd',
    '--fiat-amount-minor', '1000',
    '--epoch', '1',
    '--at', '0',
  ]);
  const accelReadiness = await waitForCanonicalEnclaveReady(adminRpcUrl, {
    enclaveId: accelEnclaveId,
    modelId: MODEL_ID,
    backend: accelBackend,
  }, 180_000);
  writeJson(path.join(runDir, 'admin-accelerated-readiness.json'), {
    enclave_id: accelEnclaveId,
    model_id: MODEL_ID,
    backend: accelBackend,
    price_ver: accelReadiness.price.current.ver,
    room_id: accelReadiness.room.room_id,
    sidechannel: accelReadiness.room.sidechannel,
    checked_at_ms: accelReadiness.checked_at_ms,
  });

  log('copying provider peer wallet to Spark and opening reverse tunnels');
  await ssh(remote, passFile, `mkdir -p ${sh(path.posix.dirname(remoteKeypair))} ${sh(path.posix.dirname(remoteHfToken))}`);
  await rsyncTo(remote, passFile, path.join(ROOT, 'intercom/stores/mayhem-devnet-joiner-b/db/keypair.json'), remoteKeypair, {
    logFile: path.join(logsDir, 'rsync-provider-keypair.log'),
    timeoutMs: 60_000,
  });
  if (fs.existsSync(hfTokenFile)) {
    await rsyncTo(remote, passFile, hfTokenFile, remoteHfToken, {
      logFile: path.join(logsDir, 'rsync-hf-token.log'),
      timeoutMs: 60_000,
    });
    cleanupState.remoteHfToken = remoteHfToken;
    await ssh(remote, passFile, `chmod 600 ${sh(remoteHfToken)}`);
  }
  const remoteAdminTunnelPort = await remoteFreePort(remote, passFile);
  const remoteBridgeTunnelPort = await remoteFreePort(remote, passFile);
  const tunnelLog = path.join(logsDir, 'ssh-reverse-tunnel.log');
  const tunnelInvocation = sshInvocation(remote, passFile);
  const tunnel = spawnLogged(
    tunnelInvocation.command,
    [
      ...tunnelInvocation.args,
      '-N',
      '-R', `127.0.0.1:${remoteAdminTunnelPort}:127.0.0.1:${adminRpcPort}`,
      '-R', `127.0.0.1:${remoteBridgeTunnelPort}:127.0.0.1:${joinerBPort}`,
      remoteTarget(remote),
    ],
    tunnelLog
  );
  cleanupState.localChildren.push(tunnel);
  await sleep(1500);
  await waitReverseTunnel(
    remote,
    passFile,
    `http://127.0.0.1:${remoteAdminTunnelPort}/v1/state?prefix=price%2F&confirmed=false&limit=1`,
    tunnel,
    tunnelLog,
    'Spark contract RPC'
  );

  if (accelBackend === 'vllm') {
    const vllmReady = (await ssh(remote, passFile, `test -x ${sh(remoteVllmPython)} && echo yes || true`)).stdout.trim() === 'yes';
    if (!vllmReady) fail(`vLLM Python wrapper is not executable on Spark: ${remoteVllmPython}`);
  }

  log('configuring Spark provider home');
  const remoteProviderSetupLog = path.posix.join(remoteLogs, 'provider-setup.log');
  await ssh(
    remote,
    passFile,
    remoteShell([
      remoteVllmEnvPrelude,
      `cd ${sh(remoteRoot)}`,
      `MAYHEM_PEAR_RUNTIME=${sh(remotePear)} ${sh(remoteMayhem)} setup --home ${sh(remoteProviderHome)} --role provider --wallet reuse --peer-store-name main --rpc-url ${sh(`http://127.0.0.1:${remoteAdminTunnelPort}/v1`)} --no-consent --yes --print-json > ${sh(remoteProviderSetupLog)} 2>&1`,
    ].filter(Boolean).join('\n')),
    { logFile: path.join(logsDir, 'remote-provider-setup.log'), timeoutMs: 180_000 }
  );

  log(`starting Spark provider start with auto backend selection for ${accelLabel}`);
  const remoteProviderLog = path.posix.join(remoteLogs, 'provider-start.log');
  const providerEnv = [
    'MAYHEM_PROVIDER_SESSION_DEBUG=1',
    'MAYHEM_PROVIDER_CANDIDATE_DEBUG=1',
    `MAYHEM_TRTLLM_PYTHON=${sh(remoteTrtPython)}`,
    'MAYHEM_TRTLLM_REQUEST_TIMEOUT_SECS=600',
    `MAYHEM_VLLM_PYTHON=${sh(remoteVllmPython)}`,
    'MAYHEM_VLLM_REQUEST_TIMEOUT_SECS=900',
    `MAYHEM_PROVIDER_SESSION_REQUEST_TIMEOUT_MS=${envPositiveInt('MAYHEM_E11_PROVIDER_SESSION_REQUEST_TIMEOUT_MS', 15_000)}`,
    `MAYHEM_PEAR_RUNTIME=${sh(remotePear)}`,
    'PATH="$HOME/.cargo/bin:$PATH"',
  ].join(' ');
  const providerArgs = [
    `setsid env ${providerEnv} ${sh(remoteMayhem)} provider start`,
    `--home ${sh(remoteProviderHome)}`,
    `--rpc-url ${sh(`http://127.0.0.1:${remoteAdminTunnelPort}/v1`)}`,
    `--session-rpc-url ${sh(`http://127.0.0.1:${remoteAdminTunnelPort}/v1`)}`,
    `--sc-bridge-url ${sh(`ws://127.0.0.1:${remoteBridgeTunnelPort}`)}`,
    `--sc-bridge-token ${sh(scToken)}`,
    `--catalog-path ${sh(path.posix.join(remoteRoot, 'catalog/models.json'))}`,
    `--downloads-dir ${sh(remoteDownloads)}`,
    fs.existsSync(hfTokenFile) ? `--hf-token-file ${sh(remoteHfToken)}` : '',
    providerHwQuoteKind ? `--hardware-quote-kind ${sh(providerHwQuoteKind)}` : '',
    providerHwQuoteCommand ? `--hardware-quote-command ${sh(providerHwQuoteCommand)}` : '',
    providerHwQuoteKind ? `--hardware-quote-timeout-seconds ${envPositiveInt('MAYHEM_E11_PROVIDER_HW_QUOTE_TIMEOUT_SECONDS', 120)}` : '',
    '--skip-disk-bench',
    `--chunk-size ${CHUNK_SIZE}`,
    '--serve-sessions',
    '--serve-sessions-seconds 1200',
    '--print-json',
  ].filter(Boolean).join(' ');
  const providerScript = [
    remoteEnvPrelude,
    remoteVllmEnvPrelude,
    `mkdir -p ${sh(path.posix.dirname(remoteKeypair))} ${sh(remoteDownloads)} ${sh(remoteLogs)}`,
    `cd ${sh(remoteRoot)}`,
    `${providerArgs} > ${sh(remoteProviderLog)} 2>&1 < /dev/null &`,
    'pid=$!',
    'printf "%s\\n" "$pid"',
    'disown "$pid" 2>/dev/null || true',
  ].join('\n');
  const providerCmd = `bash -lc ${sh(providerScript)}`;
  const remoteProviderPid = (await ssh(remote, passFile, providerCmd, { timeoutMs: 60_000 })).stdout.trim();
  cleanupState.remotePids.push(remoteProviderPid);
  const providerStartTimeout = envPositiveInt('MAYHEM_E11_PROVIDER_START_TIMEOUT_SECONDS', 1800);
  const providerWaitCmd = `for i in $(seq 1 ${Math.ceil(providerStartTimeout * 2)}); do grep -q '"self_test"' ${sh(remoteProviderLog)} && exit 0; kill -0 ${remoteProviderPid} >/dev/null 2>&1 || { tail -n 160 ${sh(remoteProviderLog)} >&2 || true; exit 2; }; sleep 0.5; done; tail -n 160 ${sh(remoteProviderLog)} >&2 || true; exit 1`;
  await ssh(remote, passFile, providerWaitCmd, { logFile: path.join(logsDir, 'remote-provider-wait.log'), timeoutMs: providerStartTimeout * 1000 + 10_000 });
  if (cleanupState.remoteHfToken) {
    await ssh(remote, passFile, `rm -f ${sh(cleanupState.remoteHfToken)}`).catch(() => {});
    cleanupState.remoteHfToken = null;
  }
  const providerLogText = (await ssh(remote, passFile, `cat ${sh(remoteProviderLog)}`)).stdout;
  fs.writeFileSync(path.join(logsDir, 'provider-start.spark.log'), providerLogText);
  const providerReport = parseProviderStartupReport(providerLogText);
  if (!providerReport?.provider) fail(`provider startup report missing provider; see ${path.join(logsDir, 'provider-start.spark.log')}`);
  if (providerReport.provider !== providerPeerPubkey) fail(`Spark provider wallet ${providerReport.provider} did not match Pear provider peer ${providerPeerPubkey}`);
  if (providerReport.artifact?.name !== accelArtifactName || providerReport.artifact?.engine !== accelBackend) {
    fail(`Spark provider did not auto-select ${accelLabel}: ${JSON.stringify(providerReport.artifact)}`);
  }

  let macProviderPath = null;
  let macProviderReport = null;
  if (envFlag('MAYHEM_E11_SKIP_MAC_FALLBACK', false)) {
    log('skipping local Apple fallback assertion');
  } else {
    log('checking local Apple fallback provider-start selects GGUF');
    const macProviderHome = path.join(runDir, 'mac-provider-home');
    await fsp.mkdir(path.join(macProviderHome, 'stores'), { recursive: true });
    await fsp.rm(path.join(macProviderHome, 'stores/main'), { recursive: true, force: true });
    await fsp.symlink(path.join(ROOT, 'intercom/stores/mayhem-devnet-joiner-a'), path.join(macProviderHome, 'stores/main'));
    macProviderPath = path.join(runDir, 'mac-provider-start.json');
    const macArgs = [
      'provider', 'start',
      '--home', macProviderHome,
      '--rpc-url', adminRpcUrl,
      '--catalog-path', path.join(ROOT, 'catalog/models.json'),
      '--downloads-dir', path.join(runDir, 'mac-provider-downloads'),
      ...(fs.existsSync(hfTokenFile) ? ['--hf-token-file', hfTokenFile] : []),
      '--fixture', 'apple-silicon',
      '--skip-disk-bench',
      '--no-heartbeat',
      '--sim',
      '--print-json',
    ];
    macProviderReport = runJsonCommandToFile(mayhemBin, macArgs, macProviderPath, {
      timeoutMs: 20 * 60_000,
    });
    if (macProviderReport.artifact?.name !== GGUF_ARTIFACT || macProviderReport.artifact?.engine !== 'llama.cpp') {
      fail(`Apple fallback provider start did not select ${GGUF_ARTIFACT}/llama.cpp: ${JSON.stringify(macProviderReport.artifact)}`);
    }
  }

  log('waiting for direct session connectivity and starting gateway');
  const directReady = await waitBridgeSessionOpen(joinerAWs[1], scToken, providerReport.provider, 420_000, 'joiner-a');
  const gatewayPort = await freePort();
  const gatewayUrl = `http://127.0.0.1:${gatewayPort}`;
  const gatewayHome = path.join(runDir, 'gateway-home');
  await fsp.mkdir(path.join(gatewayHome, 'stores'), { recursive: true });
  await fsp.rm(path.join(gatewayHome, 'stores/main'), { recursive: true, force: true });
  await fsp.symlink(path.join(ROOT, 'intercom/stores/mayhem-devnet-joiner-a'), path.join(gatewayHome, 'stores/main'));
  const gatewayLog = path.join(logsDir, 'gateway.log');
  const gateway = spawnLogged(
    mayhemBin,
    [
      'use',
      '--home', gatewayHome,
      '--rpc-url', adminRpcUrl,
      '--sc-bridge-url', joinerAWs[1],
      '--sc-bridge-token', scToken,
      '--dev-catalog-path', path.join(ROOT, 'catalog/models.json'),
      '--dev-skip-catalog-verify',
      '--session-open-timeout-seconds', '120',
      '--session-ttft-timeout-seconds', '420',
      '--session-frame-timeout-seconds', '420',
      '--canary-probe-min-interval-sessions', '1',
      '--canary-probe-max-interval-sessions', '1',
      '--canary-probe-epoch', '1',
      '--bind', `127.0.0.1:${gatewayPort}`,
      '--json',
    ],
    gatewayLog
  );
  cleanupState.localChildren.push(gateway);
  await waitHttp(`${gatewayUrl}/mayhem/status`, 120_000, 'local gateway', async () => gateway.exitCode === null);
  const routeInfo = await waitGatewayRoute(gatewayUrl, MODEL_ID, providerReport.provider, 180_000, requestMinAttTier);
  const minTierModels = requestMinAttTier === null
    ? null
    : runJsonCommandToFile(mayhemBin, [
        'models',
        '--home', gatewayHome,
        '--gateway',
        '--gateway-url', gatewayUrl,
        '--min-att-tier', String(requestMinAttTier),
        '--json',
      ], path.join(runDir, `gateway-models-min-att-tier-${requestMinAttTier}.json`));

  log(`running real streaming chat through Spark ${accelLabel} provider`);
  const chatStream = await runStreamingChatSmoke(gatewayUrl, MODEL_ID, runDir, {
    timeoutMs: envPositiveInt('MAYHEM_E11_CHAT_TIMEOUT_SECONDS', 600) * 1000,
    maxTokens: envPositiveInt('MAYHEM_E11_CHAT_MAX_TOKENS', 24),
    fileStem: 'gateway-chat-stream',
    minAttTier: requestMinAttTier,
  });
  const toolCall = accelBackend === 'vllm' && !envFlag('MAYHEM_E11_SKIP_TOOL_CALL', false)
    ? await runGuidedToolCallSmoke(gatewayUrl, MODEL_ID, runDir, {
        timeoutMs: envPositiveInt('MAYHEM_E11_TOOL_TIMEOUT_SECONDS', 600) * 1000,
        minAttTier: requestMinAttTier,
      })
    : null;
  let concurrentHeartbeat = null;
  if (accelBackend === 'vllm') {
    const concurrentSessions = envPositiveInt('MAYHEM_E11_CONCURRENT_SESSIONS', 2);
    log(`running ${concurrentSessions} concurrent vLLM sessions and capturing live heartbeat load`);
    const heartbeatCollector = await startHeartbeatCollector(
      joinerAWs[1],
      scToken,
      accelReadiness.room.sidechannel,
      path.join(runDir, 'gateway-concurrent-heartbeats.json')
    );
    try {
      concurrentHeartbeat = await runConcurrentHeartbeatSmoke(gatewayUrl, MODEL_ID, runDir, heartbeatCollector, {
        timeoutMs: envPositiveInt('MAYHEM_E11_CONCURRENT_TIMEOUT_SECONDS', 900) * 1000,
        maxTokens: envPositiveInt('MAYHEM_E11_CONCURRENT_MAX_TOKENS', 32),
        provider: providerReport.provider,
        enclaveId: accelEnclaveId,
        sessionCount: concurrentSessions,
        abortAfterProof: envFlag('MAYHEM_E11_CONCURRENT_ABORT_AFTER_PROOF', false),
        minAttTier: requestMinAttTier,
      });
    } finally {
      heartbeatCollector.close();
    }
  }
  const canaryProbe = await waitGatewayProbe(gatewayUrl, providerReport.provider, accelEnclaveId, 180_000);
  const receipts = await (await fetch(`${gatewayUrl}/mayhem/receipts`)).json();
  const receiptsPath = path.join(runDir, 'gateway-receipts.json');
  writeJson(receiptsPath, receipts);
  const latestReceipt = Array.isArray(receipts.data) ? receipts.data.at(-1) : null;
  const receiptBody = latestReceipt?.receipt?.body || latestReceipt?.receipt || latestReceipt || null;
  if (!latestReceipt) fail('streaming chat completed but no gateway receipt was recorded');
  if (receiptBody?.provider !== providerReport.provider || receiptBody?.enclave_id !== accelEnclaveId) {
    fail(`latest receipt did not bind Spark ${accelLabel} provider/enclave: ${JSON.stringify(receiptBody)}`);
  }

  log('settling receipt through epoch commit/apply');
  const epochSettlement = await settleGatewayReceiptEpoch({
    mayhemBin,
    adminHome,
    adminRpcUrl,
    receiptsPath,
    runDir,
    user: userPubkey,
    provider: providerReport.provider,
    epoch: 1,
    feeBps: 1500,
  });

  const report = {
    ok: true,
    tag,
    run_dir: path.relative(ROOT, runDir),
    model_id: MODEL_ID,
    catalog: {
      manifest_hash: manifestHash,
      accelerated_artifact: accelArtifactName,
      accelerated_backend: accelBackend,
      accelerated_att_tier: accelAttTier,
      request_min_att_tier: requestMinAttTier,
      provider_hw_quote_kind: providerHwQuoteKind || null,
      gguf_artifact: GGUF_ARTIFACT,
      gateway_catalog_source: 'contract-plus-dev-local-current-signed',
    },
    local: {
      admin_rpc_url: adminRpcUrl,
      admin_sc_bridge_url: adminWs[1],
      user_sc_bridge_url: joinerAWs[1],
      provider_bridge_url: joinerBWs[1],
      subnet_channel: subnetChannel,
    },
    remote: {
      host: remote.host,
      root: remoteRoot,
      target_dir: remoteTargetDir,
      downloads: remoteDownloads,
      provider_log_local_copy: path.relative(ROOT, path.join(logsDir, 'provider-start.spark.log')),
    },
    admin: {
      pubkey: adminPubkey,
      user_pubkey: userPubkey,
      provider_pubkey: providerReport.provider,
      rules_hash: rulesHash,
      reports: Object.fromEntries(Object.entries(adminReports).map(([key, value]) => [key, {
        submitted: value.submitted,
        tx_type: value.tx_type,
        command: value.command,
      }])),
    },
    enclaves: {
      accelerated: {
        enclave_id: accelEnclaveId,
        backend: accelBackend,
        artifact_name: accelArtifactName,
        att_tier: accelAttTier,
        artifact_root: accelArtifact.artifact_root,
        binary_hash: remoteBinaryHash,
      },
      gguf: {
        enclave_id: ggufEnclaveId,
        backend: 'llama.cpp',
        artifact_name: GGUF_ARTIFACT,
        artifact_root: ggufArtifact.artifact_root,
        binary_hash: localBinaryHash,
      },
    },
    provider: {
      self_test: providerReport.self_test,
      artifact: providerReport.artifact,
      hardware: providerReport.hardware,
      attestation: providerReport.attestation,
      direct_ready: directReady,
      hardware_quote_kind: providerHwQuoteKind || null,
    },
    mac_fallback: {
      skipped: macProviderReport === null,
      artifact: macProviderReport?.artifact || null,
      hardware: macProviderReport?.hardware || null,
      self_test: macProviderReport?.self_test || null,
      report: macProviderPath ? path.relative(ROOT, macProviderPath) : null,
    },
    gateway: {
      url: gatewayUrl,
      route_info: {
        selected_model_id: routeInfo.selected?.id || null,
        route_count: routeInfo.routes.length,
        request_min_att_tier: requestMinAttTier,
        routes: routeInfo.routes.map((route) => ({
          provider: route.provider,
          enclave_id: route.enclave_id,
          att_tier: route.att_tier,
          quant: route.quant,
          price_ver: route.price_ref_au?.ver,
          rate_map: route.price_ref_au?.rate_map || null,
        })),
      },
      min_att_tier_models: minTierModels,
      chat_stream: chatStream,
      guided_tool_call: toolCall,
      concurrent_heartbeat: concurrentHeartbeat,
      canary_probe: canaryProbe,
      latest_receipt: receiptBody,
      receipts: path.relative(ROOT, receiptsPath),
      epoch_settlement: epochSettlement,
    },
  };
  const reportPath = path.join(runDir, 'report.json');
  writeJson(reportPath, report);
  console.log(JSON.stringify(report, null, 2));

  await cleanup(cleanupState);
}

async function cleanup(state) {
  if (cleanupStarted) return;
  cleanupStarted = true;
  log('cleaning up processes');
  for (const child of state.localChildren || []) {
    if (child && child.exitCode === null) child.kill('SIGTERM');
  }
  for (const child of children) {
    if (child && child.exitCode === null) child.kill('SIGTERM');
  }
  const { remote, passFile } = state;
  if (remote && (remote.auth === 'key' || (passFile && fs.existsSync(passFile)))) {
    if (state.remoteHfToken) {
      try {
        await ssh(remote, passFile, `rm -f ${sh(state.remoteHfToken)}`);
      } catch {}
    }
    for (const pid of state.remotePids || []) {
      if (pid) {
        try {
          await ssh(remote, passFile, `kill ${Number(pid)} >/dev/null 2>&1 || true`);
        } catch {}
      }
    }
    if (!/^(1|true|yes)$/i.test(process.env.MAYHEM_E11_KEEP_REMOTE_RUN || '0') && state.remoteRun) {
      try {
        await ssh(remote, passFile, `rm -rf ${sh(state.remoteRun)}`);
      } catch {}
    }
    if (/^(0|false|no)$/i.test(process.env.MAYHEM_E11_KEEP_PROVIDER_CACHE || '1') && state.remoteProviderCacheRoot) {
      try {
        await ssh(remote, passFile, `rm -rf ${sh(state.remoteProviderCacheRoot)}`);
      } catch {}
    }
  }
  if (state.devnetLog && fs.existsSync(state.devnetLog)) {
    const text = fs.readFileSync(state.devnetLog, 'utf8');
    const match = text.match(/Peers are still running\. Stop them with: kill ([0-9\s]+)/);
    if (match) {
      for (const pid of match[1].trim().split(/\s+/)) {
        try {
          process.kill(Number(pid), 'SIGTERM');
        } catch {}
      }
    }
  }
  if (passFile) {
    try {
      await fsp.rm(passFile, { force: true });
    } catch {}
  }
}

process.on('SIGINT', () => {
  process.stderr.write('\n');
  process.exitCode = 130;
});

main().catch(async (err) => {
  console.error(`[e11] ERROR: ${err.stack || err.message}`);
  await cleanup(cleanupState);
  process.exit(1);
});

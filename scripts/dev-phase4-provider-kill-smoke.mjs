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
const SSH_KEY = (process.env.MAYHEM_P43_SSH_KEY || '').trim();
const SSH_OPTS = [
  '-F', '/dev/null',
  '-o', 'IdentitiesOnly=yes',
  ...(SSH_KEY
    ? [
      '-i', path.resolve(SSH_KEY),
      '-o', 'PreferredAuthentications=publickey,password,keyboard-interactive',
      '-o', 'PubkeyAuthentication=yes',
    ]
    : [
      '-o', 'PreferredAuthentications=password,keyboard-interactive',
      '-o', 'PubkeyAuthentication=no',
    ]),
  '-o', 'PasswordAuthentication=yes',
  '-o', 'KbdInteractiveAuthentication=yes',
  '-o', 'NumberOfPasswordPrompts=1',
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
let fallbackRemotePort = 61000 + crypto.randomInt(0, 3000);

function usage() {
  console.log(`Usage:
  node scripts/dev-phase4-provider-kill-smoke.mjs

Runs the P4.3 live failover acceptance:
  - local Pear admin/user gateway
  - two Mac mini Pear provider peers in one admin-created room
  - one gateway request over direct SC-Bridge sessions
  - kill -9 of the provider that starts serving the request
  - recovery through the second provider in under 20 s

Environment:
  MAYHEM_P43_MACMINI_FILE    Mac mini credential file (default: ../gpd/macmini.txt)
  MAYHEM_P43_REMOTE_ROOT     Remote checkout/staging root (default: ~/mayhem-macmini-p33)
  MAYHEM_P43_MODEL           Catalog model id (default: ${DEFAULT_MODEL})
  MAYHEM_P43_PROVIDER_MODE   real or shim (default: real)
  MAYHEM_P43_AGENT_LOOP      Run E13 multi-model/stickiness smoke instead of provider-kill smoke (default: 0)
  MAYHEM_P43_SECONDARY_MODEL Secondary smoke-only model id for E13 (default: <model>-e13-helper)
  MAYHEM_P43_ARTIFACT        Real provider artifact path (default: ${DEFAULT_ARTIFACT})
  MAYHEM_P43_DELTA_DELAY_MS  Delay after early content deltas (default: 2500)
  MAYHEM_P43_DELTA_DELAY_COUNT Number of early content deltas to delay (default: 12 real, 1 shim)
  MAYHEM_P43_CHAT_MAX_TOKENS Streaming failover max_tokens (default: 128)
  MAYHEM_P43_SESSION_OPEN_TIMEOUT_SECONDS Direct-session open timeout (default: 60 real E13, 30 shim E13, 3 provider-kill)
  MAYHEM_P43_RECEIPT_CHECKPOINT_TOKENS Receipt checkpoint token window (default: 32)
  MAYHEM_P43_RECEIPT_CHECKPOINT_MS Receipt checkpoint wall-clock window (default: 30000)
  MAYHEM_P43_LOCAL_DHT_HOST  Local LAN IP advertised to the Mac mini DHT peer
  MAYHEM_P43_PEER_DHT_BOOTSTRAP Explicit peer DHT bootstrap list (debug override only)
  MAYHEM_P43_USE_LOCAL_DHT   Start/use a temporary local HyperDHT bootstrap (debug override, default: 0)
  MAYHEM_P43_USE_REMOTE_DHT  Start/use a temporary Mac mini HyperDHT bootstrap (debug override, default: 0)
  MAYHEM_P43_USE_PUBLIC_DHT  Use default/public peer DHT bootstrap (default: 1)
  MAYHEM_P43_PROBE_SESSION_OPEN Also preflight throwaway direct sessions before gateway start (default: 0)
  MAYHEM_P43_KEEP_REMOTE     Keep remote run logs after cleanup (default: 1)
  MAYHEM_P43_KEEP_LOCAL      Keep local run evidence after cleanup (default: 1)
`);
}

function log(message) {
  process.stderr.write(`[p4.3] ${message}\n`);
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

function writeJson(file, value) {
  fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`);
}

function envPositiveInt(name, fallback) {
  const raw = process.env[name];
  if (!raw) return fallback;
  const value = Number.parseInt(raw, 10);
  if (!Number.isSafeInteger(value) || value <= 0) {
    fail(`${name} must be a positive integer`);
  }
  return value;
}

function envFlag(name, fallback = false) {
  const raw = process.env[name];
  if (raw === undefined || raw === '') return fallback;
  return /^(1|true|yes)$/i.test(raw);
}

function safePathComponent(value) {
  return String(value).replace(/[^A-Za-z0-9._-]/g, '_');
}

function providerSidecarCacheFile(artifactName, name, sidecar) {
  return [
    `root-${safePathComponent(sidecar.artifact_root)}`,
    `sha-${safePathComponent(sidecar.source_sha256)}`,
    safePathComponent(artifactName),
    safePathComponent(name),
  ].join('-');
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

function runSync(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: options.cwd || ROOT,
    env: options.env || process.env,
    encoding: 'utf8',
    maxBuffer: options.maxBuffer || 64 * 1024 * 1024,
  });
  if (result.status !== 0) {
    fail(`${command} ${args.join(' ')} failed\n${result.stderr || ''}${result.stdout || ''}`);
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
    let timedOut = false;
    let killTimer = null;
    const timer = options.timeoutMs
      ? setTimeout(() => {
          timedOut = true;
          child.kill('SIGTERM');
          killTimer = setTimeout(() => child.kill('SIGKILL'), 2_000);
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
      if (killTimer) clearTimeout(killTimer);
      outStream?.end();
      reject(err);
    });
    child.on('close', (code, signal) => {
      if (timer) clearTimeout(timer);
      if (killTimer) clearTimeout(killTimer);
      outStream?.end();
      const stdout = Buffer.concat(output).toString('utf8');
      const stderr = Buffer.concat(errput).toString('utf8');
      if (timedOut) {
        reject(new Error(`${command} timed out after ${options.timeoutMs}ms`));
        return;
      }
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

async function catalogEnclaveId({
  adminPubkey,
  modelId,
  artifactRoot,
  artifactSidecarRoots = {},
  manifestHash,
  binaryHash,
}) {
  const sidecarParts = Object.entries(artifactSidecarRoots)
    .sort(([left], [right]) => left.localeCompare(right))
    .flatMap(([name, root]) => [name, root]);
  return (await b3(Buffer.from(
    [adminPubkey, modelId, artifactRoot, ...sidecarParts, manifestHash, binaryHash].join(''),
    'utf8'
  ))).toString('hex');
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
    const finish = (err, value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      try { socket.close(); } catch {}
      if (err) reject(err);
      else resolve(value);
    };
    const timer = setTimeout(() => finish(new Error(`${payload.type} timed out`)), timeoutMs);
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

async function waitBridgeSessionOpen(url, token, provider, timeoutMs, label = 'bridge') {
  const deadline = Date.now() + timeoutMs;
  let attempts = 0;
  let lastError = null;
  while (Date.now() < deadline) {
    attempts += 1;
    const sessionId = crypto.randomBytes(32).toString('hex');
    try {
      const opened = await bridgeRequest(
        url,
        token,
        { type: 'session_open', remote: provider, session_id: sessionId },
        Math.max(1_000, Math.min(20_000, deadline - Date.now()))
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
    await sleep(1_000);
  }
  throw new Error(`${label} could not open direct session to ${provider}: ${lastError?.message || 'unknown error'}`);
}

async function waitBridgePeerConnect(url, token, provider, timeoutMs, label = 'bridge') {
  const connected = await bridgeRequest(
    url,
    token,
    { type: 'peer_connect', remote: provider, wait_ms: timeoutMs },
    timeoutMs + 2_000
  );
  if (connected.connected === false) {
    throw new Error(`${label} did not connect to ${provider}`);
  }
  return { label, url, provider, connected };
}

async function pickBridgeForProviders(candidates, token, providers, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  const errors = [];
  let nextProgressLog = Date.now() + 10_000;
  const probeSessionOpen = /^(1|true|yes)$/i.test(process.env.MAYHEM_P43_PROBE_SESSION_OPEN || '');
  while (Date.now() < deadline) {
    for (const candidate of candidates) {
      const remainingMs = deadline - Date.now();
      if (remainingMs <= 0) break;
      try {
        const ready = [];
        for (const provider of providers) {
          const peerConnectTimeoutMs = Math.min(10_000, Math.max(1_000, deadline - Date.now()));
          ready.push(await waitBridgePeerConnect(
            candidate.url,
            token,
            provider,
            peerConnectTimeoutMs,
            candidate.label
          ));
          if (probeSessionOpen) {
            const sessionOpenTimeoutMs = Math.min(5_000, Math.max(1_000, deadline - Date.now()));
            ready.push(await waitBridgeSessionOpen(candidate.url, token, provider, sessionOpenTimeoutMs, candidate.label));
          }
        }
        return { ...candidate, ready };
      } catch (err) {
        errors.push(`${candidate.label}: ${err?.message || err}`);
        if (errors.length > 50) errors.splice(0, errors.length - 50);
      }
    }
    if (Date.now() >= nextProgressLog) {
      log(`SC-Bridge selection still probing; last error: ${errors.at(-1) || 'none yet'}`);
      nextProgressLog = Date.now() + 10_000;
    }
    await sleep(750);
  }
  throw new Error(`no local SC-Bridge could open both providers; last errors: ${errors.slice(-candidates.length).join(' | ')}`);
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
    await sleep(1_000);
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

async function runStreamingChatSmoke(gatewayUrl, modelId, runDir, {
  timeoutMs = 240_000,
  maxTokens = 128,
  prompt = [
    'Write a long deterministic failover response.',
    'Use short numbered lines.',
    'Each line must include mayhem and a different plain word.',
    'Keep going until the token budget is exhausted; do not summarize or stop early.',
  ].join(' '),
  fileStem = 'gateway-provider-kill-stream',
  metadata = null,
  user = null,
} = {}) {
  const rawPath = path.join(runDir, `${fileStem}.sse`);
  const summaryPath = path.join(runDir, `${fileStem}.json`);
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(new Error('streaming chat timed out')), timeoutMs);
  let raw = '';
  let content = '';
  let dataEvents = 0;
  let jsonEvents = 0;
  let doneSeen = false;
  let errorEvent = null;
  const startedAt = Date.now();
  let responseHeadersAt = null;
  let firstChunkAt = null;
  let firstDataAt = null;
  let firstJsonAt = null;
  let firstContentAt = null;
  let doneAt = null;
  try {
    const body = {
      model: modelId,
      stream: true,
      temperature: 0.4,
      max_tokens: maxTokens,
      messages: [{ role: 'user', content: prompt }],
    };
    if (metadata && Object.keys(metadata).length > 0) body.metadata = metadata;
    if (user) body.user = user;
    const response = await fetch(`${gatewayUrl}/v1/chat/completions`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body),
      signal: controller.signal,
    });
    responseHeadersAt = Date.now();
    if (!response.ok) {
      const text = await response.text();
      fail(`streaming failover chat returned HTTP ${response.status}: ${text}`);
    }
    if (!response.body) fail('streaming failover chat response had no body');
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
      if (value.error) {
        errorEvent = value.error;
        return;
      }
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
  if (errorEvent) fail(`streaming failover chat emitted SSE error: ${JSON.stringify(errorEvent)}`);
  if (dataEvents === 0) fail('streaming failover chat produced no SSE data events');
  if (!doneSeen) fail(`streaming failover chat did not emit [DONE]; raw saved at ${rawPath}`);
  if (!content.trim()) fail(`streaming failover chat produced no model content; raw saved at ${rawPath}`);
  const completedAt = doneAt || Date.now();
  const summary = {
    ok: true,
    model: modelId,
    prompt,
    metadata,
    user,
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

function sshProgram(passFile) {
  return SSH_KEY
    ? { command: 'ssh', args: [...SSH_OPTS] }
    : { command: 'sshpass', args: ['-f', passFile, 'ssh', ...SSH_OPTS] };
}

function sshBase(passFile, remote) {
  const base = sshProgram(passFile);
  return [base.command, ...base.args, `${remote.user}@${remote.host}`];
}

async function ssh(remote, passFile, command, options = {}) {
  const [program, ...args] = sshBase(passFile, remote);
  return run(program, [...args, command], options);
}

async function rsyncTo(remote, passFile, localPath, remotePath, options = {}) {
  const base = sshProgram(passFile);
  const rsh = [base.command, ...base.args].map(sh).join(' ');
  return run(
    'rsync',
    ['-a', '--partial', '--inplace', '-e', rsh, localPath, `${remote.user}@${remote.host}:${remotePath}`],
    options
  );
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
      log(`copy retry ${attempt}/${attempts} for ${path.basename(localPath)}: ${err?.message || err}`);
      await sleep(1_000 * attempt);
    }
  }
  log(`copy fallback via ssh stream for ${path.basename(localPath)}: ${lastError?.message || lastError}`);
  return sshStreamTo(remote, passFile, localPath, remotePath, {
    ...options,
    timeoutMs: Math.max(options.timeoutMs || 0, 180_000),
  });
}

async function sshStreamTo(remote, passFile, localPath, remotePath, options = {}) {
  const tmpPath = `${remotePath}.tmp-${process.pid}-${Date.now()}`;
  const remoteCommand = `cat > ${sh(tmpPath)} && mv ${sh(tmpPath)} ${sh(remotePath)}`;
  const outStream = options.logFile ? fs.createWriteStream(options.logFile, { flags: 'a' }) : null;
  const output = [];
  const errput = [];
  return new Promise((resolve, reject) => {
    const [program, ...args] = sshBase(passFile, remote);
    const child = spawn(program, [...args, remoteCommand], {
      cwd: ROOT,
      env: options.env || process.env,
      stdio: ['pipe', 'pipe', 'pipe'],
    });
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
      input.destroy();
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

async function waitRemoteLogPattern(remote, passFile, file, pattern, timeoutMs, label, pid = null) {
  const deadline = Date.now() + timeoutMs;
  let last = '';
  while (Date.now() < deadline) {
    last = (await ssh(
      remote,
      passFile,
      `test -f ${sh(file)} && tail -n 120 ${sh(file)} || true`,
      { timeoutMs: 10_000 }
    )).stdout;
    if (pattern.test(last)) return last;
    if (pid && !(await remotePidAlive(remote, passFile, pid))) {
      fail(`${label} exited before readiness. Log: ${file}`);
    }
    await sleep(500);
  }
  fail(`timed out waiting for ${label}; last log tail:\n${last.split('\n').slice(-40).join('\n')}`);
}

async function cleanupLocalSmokeProcesses({ allP43 = false, tag = null } = {}) {
  const patterns = [];
  if (allP43) {
    patterns.push('[m]ayhem-p43-');
    patterns.push('[.]mayhem-local/p4[.]3-provider-kill');
  } else if (tag) {
    const safeTag = String(tag).replace(/[^\w.-]/g, '');
    if (safeTag) {
      patterns.push(`[m]ayhem-p43-.*${safeTag}`);
      patterns.push(`[.]mayhem-local/p4[.]3-provider-kill/${safeTag}`);
    }
  }
  if (!patterns.length) return;
  for (const pattern of patterns) {
    await run('pkill', ['-TERM', '-f', pattern], { timeoutMs: 5_000 }).catch(() => {});
  }
  await sleep(1_000);
  for (const pattern of patterns) {
    await run('pkill', ['-KILL', '-f', pattern], { timeoutMs: 5_000 }).catch(() => {});
  }
}

async function cleanupRemoteSmokeProcesses(remote, passFile, { remoteRun = null, allP43 = false } = {}) {
  if (!remote || !passFile || !fs.existsSync(passFile)) return;
  const patterns = [];
  if (allP43) {
    patterns.push('[m]ayhem-p43-');
    patterns.push('[.]mayhem-local/p4[.]3-provider-kill');
  } else if (remoteRun) {
    const tag = path.posix.basename(remoteRun).replace(/[^\w.-]/g, '');
    if (tag) {
      patterns.push(`[m]ayhem-p43-.*${tag}`);
      patterns.push(`[.]mayhem-local/p4[.]3-provider-kill/${tag}`);
    }
  }
  if (!patterns.length) return;
  const term = patterns.map((pattern) => `pkill -TERM -f ${sh(pattern)} >/dev/null 2>&1 || true`);
  const kill = patterns.map((pattern) => `pkill -KILL -f ${sh(pattern)} >/dev/null 2>&1 || true`);
  await ssh(remote, passFile, [...term, 'sleep 1', ...kill].join('; '), { timeoutMs: 20_000 }).catch((err) => {
    log(`remote stale p4.3 cleanup warning: ${err?.message || err}`);
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
  try {
    const result = await ssh(remote, passFile, `/usr/bin/python3 -c ${sh(script)}`, { timeoutMs: 10_000 });
    const port = Number.parseInt(result.stdout.trim(), 10);
    if (!Number.isSafeInteger(port) || port <= 0) {
      throw new Error(`remote did not return a free port: ${result.stdout}`);
    }
    return port;
  } catch (err) {
    const port = fallbackRemotePort++;
    if (port > 65000) fail(`remote port fallback exhausted after probe failure: ${err?.message || err}`);
    log(`remote free-port probe failed; using fallback loopback port ${port}: ${err?.message || err}`);
    return port;
  }
}

function localPeerDhtHost(remoteHost) {
  const override = (process.env.MAYHEM_P43_LOCAL_DHT_HOST || '').trim();
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

async function waitForGatewayRoutes(gatewayUrl, modelId, providerPubkeys, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  let last = null;
  while (Date.now() < deadline) {
    try {
      const models = await (await fetch(`${gatewayUrl}/v1/models`)).json();
      const selected = models.data?.find((entry) => entry.id === modelId) || null;
      const routes = selected?.mayhem?.route_candidates || [];
      const routeProviders = new Set(routes.map((route) => route.provider));
      if (selected && providerPubkeys.every((provider) => routeProviders.has(provider))) {
        return { models, selected, routes };
      }
      last = { selected, routes };
    } catch (err) {
      last = { error: err.message };
    }
    await sleep(1_000);
  }
  throw new Error(`gateway did not expose both provider routes: ${JSON.stringify(last)}`);
}

async function waitAndKillAfterCheckpointAck(remote, passFile, providers, timeoutMs) {
  const checkpointPattern = /sending checkpoint s\.receipt seq (\d+)/;
  const postCheckpointProgressPattern =
    /live buffering token #\d+|live streaming s\.delta #\d+|sending checkpoint s\.receipt seq [2-9]\d*|sending final s\.delta/;
  const last = {};
  const checkpointSeen = new Map();
  const watchers = [];
  let settled = false;

  return new Promise((resolve, reject) => {
    const finish = (err, value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      for (const child of watchers) {
        if (child.exitCode === null) child.kill('SIGTERM');
      }
      if (err) reject(err);
      else resolve(value);
    };
    const timer = setTimeout(() => {
      finish(new Error(
        `no provider reached checkpointed post-receipt progress before timeout; last lines=${JSON.stringify(last, null, 2)}`
      ));
    }, timeoutMs);

    for (const provider of providers) {
      const [program, ...args] = sshBase(passFile, remote);
      const child = spawn(program, [...args, `tail -n 0 -F ${sh(provider.providerLog)}`], {
        cwd: ROOT,
        env: process.env,
        stdio: ['ignore', 'pipe', 'pipe'],
      });
      children.push(child);
      cleanupState.localChildren.push(child);
      watchers.push(child);

      let buffer = '';
      const handleLine = async (line) => {
        if (settled || !line.trim()) return;
        last[provider.label] = line;
        const checkpoint = line.match(checkpointPattern);
        if (checkpoint) {
          checkpointSeen.set(provider.label, Number.parseInt(checkpoint[1], 10));
          return;
        }
        if (!checkpointSeen.has(provider.label) || !postCheckpointProgressPattern.test(line)) return;
        const killedAt = Date.now();
        try {
          await ssh(remote, passFile, `kill -9 ${Number(provider.providerPid)} >/dev/null 2>&1 || true`);
          finish(null, {
            label: provider.label,
            provider: provider.pubkey,
            provider_pid: provider.providerPid,
            provider_log: provider.providerLog,
            killed_at_ms: killedAt,
            trigger: 'post_checkpoint_progress_after_receipt',
            checkpoint_seq: checkpointSeen.get(provider.label),
            trigger_line: line,
          });
        } catch (err) {
          finish(err);
        }
      };

      child.stdout.on('data', (data) => {
        buffer += data.toString('utf8');
        const lines = buffer.split(/\r?\n/);
        buffer = lines.pop() || '';
        for (const line of lines) {
          handleLine(line).catch((err) => finish(err));
        }
      });
      child.stderr.on('data', (data) => {
        last[`${provider.label}:stderr`] = data.toString('utf8').trim();
      });
      child.on('error', (err) => finish(err));
      child.on('close', (code, signal) => {
        if (!settled && code !== 0 && signal !== 'SIGTERM') {
          finish(new Error(`provider log watcher for ${provider.label} exited ${code ?? signal}`));
        }
      });
    }
  });
}

function storedReceiptBody(entry) {
  return entry?.receipt?.body || entry?.receipt || entry || {};
}

function receiptIsFinal(entry) {
  const body = storedReceiptBody(entry);
  return body.final === true || body.final_receipt === true;
}

async function fetchStoredReceipts(gatewayUrl) {
  const receipts = await (await fetch(`${gatewayUrl}/mayhem/receipts`)).json();
  return Array.isArray(receipts.data) ? receipts.data : [];
}

function newFinalReceiptForModel(beforeCount, receipts, modelId) {
  const finals = receipts
    .slice(beforeCount)
    .filter((entry) => {
      const body = storedReceiptBody(entry);
      return receiptIsFinal(entry) && body.model_id === modelId;
    });
  if (finals.length !== 1) {
    fail(`expected exactly one new final receipt for ${modelId}, got ${finals.length}`);
  }
  return { entry: finals[0], body: storedReceiptBody(finals[0]) };
}

function providerLogOutputTailProof(text, checkpointedOutputTokens) {
  let maxBufferedTokenIndex = -1;
  const tokenPattern = /live buffering token #(\d+)/g;
  let tokenMatch;
  while ((tokenMatch = tokenPattern.exec(text))) {
    const value = Number.parseInt(tokenMatch[1], 10);
    if (Number.isSafeInteger(value) && value > maxBufferedTokenIndex) {
      maxBufferedTokenIndex = value;
    }
  }

  const checkpointSeqs = [];
  const checkpointPattern = /sending checkpoint s\.receipt seq (\d+)/g;
  let checkpointMatch;
  while ((checkpointMatch = checkpointPattern.exec(text))) {
    const value = Number.parseInt(checkpointMatch[1], 10);
    if (Number.isSafeInteger(value)) checkpointSeqs.push(value);
  }

  const observedOutputTokens = maxBufferedTokenIndex >= 0 ? maxBufferedTokenIndex + 1 : null;
  const uncheckpointedOutputTokens = observedOutputTokens === null
    ? null
    : Math.max(0, observedOutputTokens - checkpointedOutputTokens);
  return {
    observed_output_tokens: observedOutputTokens,
    max_buffered_token_index: maxBufferedTokenIndex,
    checkpoint_sequences: checkpointSeqs,
    latest_checkpoint_seq: checkpointSeqs.at(-1) || null,
    checkpointed_output_tokens: checkpointedOutputTokens,
    uncheckpointed_output_tokens: uncheckpointedOutputTokens,
  };
}

async function startRemoteProvider({
  label,
  remote,
  passFile,
  remoteRoot,
  remoteRun,
  remoteLogs,
  remoteHome,
  remoteNode,
  remotePear,
  remoteMayhem,
  remoteWalletHelper,
  remoteCatalog,
  remoteArtifact,
  remoteDownloadsDir,
  adminRpcTunnelPort,
  rulesHash,
  channel,
  bootstrap,
  peerDhtBootstrap,
  scToken,
  enclaveId,
  delayMs,
  providerMode,
}) {
  log(`starting remote Pear provider peer ${label}`);
  const remoteScPort = await remoteFreePort(remote, passFile);
  const remoteRpcPort = await remoteFreePort(remote, passFile);
  const remoteStore = `mayhem-p43-${label}-${path.basename(remoteRun)}`;
  const remoteMsbStore = `${remoteStore}-msb`;
  const remotePeerLog = path.posix.join(remoteLogs, `provider-${label}-peer.log`);
  const remotePeerArgs = [
    '--network local',
    `--peer-store-name ${sh(remoteStore)}`,
    `--msb-store-name ${sh(remoteMsbStore)}`,
    `--subnet-channel ${sh(channel)}`,
    `--subnet-bootstrap ${sh(bootstrap)}`,
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
  await waitRemoteLogPattern(
    remote,
    passFile,
    remotePeerLog,
    /Sidechannel: ready/,
    360_000,
    `remote provider peer ${label}`,
    remotePeerPid
  );

  const providerHome = path.posix.join(remoteRun, `provider-${label}-home`);
  const providerSetupLog = path.posix.join(remoteLogs, `provider-${label}-setup.log`);
  const providerLog = path.posix.join(remoteLogs, `provider-${label}-start.log`);
  const prepareHome = [
    `mkdir -p ${sh(path.posix.join(providerHome, 'stores'))}`,
    `rm -rf ${sh(path.posix.join(providerHome, 'stores/main'))}`,
    `ln -s ${sh(path.posix.join(remoteRoot, 'intercom/stores', remoteStore))} ${sh(path.posix.join(providerHome, 'stores/main'))}`,
  ].join(' && ');
  await ssh(
    remote,
    passFile,
    [
      prepareHome,
      `cd ${sh(remoteRoot)}`,
      `MAYHEM_WALLET_HELPER=${sh(remoteWalletHelper)} MAYHEM_NODE_BIN=${sh(remoteNode)} ${sh(remoteMayhem)} setup --home ${sh(providerHome)} --role provider --wallet reuse --peer-store-name main --rpc-url ${sh(`http://127.0.0.1:${adminRpcTunnelPort}/v1`)} --rules-ver 1 --rules-hash ${sh(rulesHash)} --yes --print-json > ${sh(providerSetupLog)} 2>&1`,
    ].join(' && '),
    { timeoutMs: 180_000 }
  );
  const providerFlags = [
    `--home ${sh(providerHome)}`,
    `--enclave ${sh(enclaveId)}`,
    `--rpc-url ${sh(`http://127.0.0.1:${adminRpcTunnelPort}/v1`)}`,
    `--session-rpc-url ${sh(`http://127.0.0.1:${remoteRpcPort}/v1`)}`,
    `--sc-bridge-url ${sh(`ws://127.0.0.1:${remoteScPort}`)}`,
    `--sc-bridge-token ${sh(scToken)}`,
    `--catalog-path ${sh(remoteCatalog)}`,
    `--artifact ${sh(remoteArtifact)}`,
    `--downloads-dir ${sh(remoteDownloadsDir)}`,
    '--engine-backend llama.cpp',
    '--skip-disk-bench',
    `--chunk-size ${CHUNK_SIZE}`,
    '--serve-sessions',
    '--serve-sessions-seconds 900',
    '--print-json',
    '--dev-skip-catalog-verify',
  ];
  if (providerMode === 'shim') providerFlags.push('--dev-session-shim');
  const deltaDelayEnv = delayMs > 0
    ? [
      `MAYHEM_PROVIDER_SESSION_DELTA_DELAY_MS=${Number(delayMs)}`,
      `MAYHEM_PROVIDER_SESSION_DELTA_DELAY_COUNT=${envPositiveInt(
        'MAYHEM_P43_DELTA_DELAY_COUNT',
        providerMode === 'real' ? 12 : 1
      )}`,
    ]
    : [];
  const providerCmd = [
    prepareHome,
    `cd ${sh(remoteRoot)}`,
    [
      `MAYHEM_PROVIDER_SESSION_DEBUG=1`,
      ...deltaDelayEnv,
      `MAYHEM_WALLET_HELPER=${sh(remoteWalletHelper)}`,
      `MAYHEM_NODE_BIN=${sh(remoteNode)}`,
      `nohup ${sh(remoteMayhem)} provider start ${providerFlags.join(' ')} > ${sh(providerLog)} 2>&1 & echo $!`,
    ].join(' '),
  ].join(' && ');
  const providerPid = (await ssh(remote, passFile, providerCmd)).stdout.trim();
  cleanupState.remotePids.push(providerPid);
  const providerWaitCmd = `for i in $(seq 1 1200); do grep -q '"self_test"' ${sh(providerLog)} && exit 0; kill -0 ${providerPid} >/dev/null 2>&1 || exit 2; sleep 0.5; done; exit 1`;
  await ssh(remote, passFile, providerWaitCmd, { timeoutMs: 610_000 });
  const startupText = (await ssh(remote, passFile, `cat ${sh(providerLog)}`)).stdout;
  const startupReport = parseFirstJsonObject(startupText);
  if (!startupReport?.provider) fail(`provider ${label} startup report missing provider pubkey; see ${providerLog}`);
  return {
    label,
    pubkey: startupReport.provider,
    peerPid: remotePeerPid,
    providerPid,
    providerHome,
    peerLog: remotePeerLog,
    providerLog,
    scBridgeUrl: `ws://127.0.0.1:${remoteScPort}`,
    rpcUrl: `http://127.0.0.1:${remoteRpcPort}/v1`,
    startup: startupReport,
  };
}

async function startRemoteClientBridge({
  label,
  remote,
  passFile,
  remoteRoot,
  remoteRun,
  remoteLogs,
  remotePear,
  channel,
  bootstrap,
  peerDhtBootstrap,
  scToken,
}) {
  log(`starting remote Pear client SC-Bridge peer ${label}`);
  const remoteScPort = await remoteFreePort(remote, passFile);
  const remoteStore = `mayhem-p43-${label}-${path.basename(remoteRun)}`;
  const remoteMsbStore = `${remoteStore}-msb`;
  const remotePeerLog = path.posix.join(remoteLogs, `${label}-peer.log`);
  const remotePeerArgs = [
    '--network local',
    `--peer-store-name ${sh(remoteStore)}`,
    `--msb-store-name ${sh(remoteMsbStore)}`,
    `--subnet-channel ${sh(channel)}`,
    `--subnet-bootstrap ${sh(bootstrap)}`,
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
  await waitRemoteLogPattern(
    remote,
    passFile,
    remotePeerLog,
    /Sidechannel: ready/,
    360_000,
    `remote client bridge ${label}`,
    remotePeerPid
  );
  const startupText = (await ssh(remote, passFile, `cat ${sh(remotePeerLog)}`)).stdout;
  const pubkey = startupText.match(/Peer pubkey \(hex\):\s+([0-9a-f]{64})/);
  if (!pubkey) fail(`remote client bridge ${label} missing peer pubkey; see ${remotePeerLog}`);
  return {
    label,
    pubkey: pubkey[1],
    peerPid: remotePeerPid,
    peerLog: remotePeerLog,
    scBridgeUrl: `ws://127.0.0.1:${remoteScPort}`,
  };
}

async function openRemoteScBridgeTunnel({ remote, passFile, remoteScBridgeUrl, logsDir, label }) {
  const remotePort = new URL(remoteScBridgeUrl).port;
  const localPort = await freePort();
  const localUrl = `ws://127.0.0.1:${localPort}`;
  const tunnelLog = path.join(logsDir, `${label}-sc-bridge-tunnel.log`);
  const tunnelBase = sshProgram(passFile);
  const tunnel = spawnLogged(
    tunnelBase.command,
    [
      ...tunnelBase.args,
      '-N',
      '-L', `127.0.0.1:${localPort}:127.0.0.1:${remotePort}`,
      `${remote.user}@${remote.host}`,
    ],
    tunnelLog
  );
  cleanupState.localChildren.push(tunnel);
  await sleep(1500);
  if (tunnel.exitCode !== null) fail(`SC-Bridge tunnel exited early; see ${tunnelLog}`);
  return {
    label,
    url: localUrl,
    tunnel,
    tunnel_log: tunnelLog,
    remote_url: remoteScBridgeUrl,
  };
}

async function main() {
  if (process.argv.includes('--help') || process.argv.includes('-h')) {
    usage();
    return;
  }

  const tag = new Date().toISOString().replace(/[-:]/g, '').replace(/\..*/, '');
  const runDir = path.join(ROOT, '.mayhem-local/p4.3-provider-kill', tag);
  const logsDir = path.join(runDir, 'logs');
  const subnetChannel = `mayhem-p43-${tag}`;
  await fsp.mkdir(logsDir, { recursive: true });

  const mayhemBin = path.join(ROOT, 'target/debug/mayhem');
  const mayhemEnclaveBin = path.join(ROOT, 'target/debug/mayhem-enclave');
  const modelId = process.env.MAYHEM_P43_MODEL || DEFAULT_MODEL;
  const agentLoopMode = envFlag('MAYHEM_P43_AGENT_LOOP');
  const secondaryModelId = process.env.MAYHEM_P43_SECONDARY_MODEL || `${modelId}-e13-helper`;
  if (agentLoopMode && secondaryModelId === modelId) {
    fail('MAYHEM_P43_SECONDARY_MODEL must differ from MAYHEM_P43_MODEL');
  }
  const providerMode = (process.env.MAYHEM_P43_PROVIDER_MODE || 'real').trim();
  if (!['real', 'shim'].includes(providerMode)) fail('MAYHEM_P43_PROVIDER_MODE must be real or shim');
  const delayMs = Number.parseInt(process.env.MAYHEM_P43_DELTA_DELAY_MS || '2500', 10);
  if (!Number.isSafeInteger(delayMs) || delayMs < 0) fail('MAYHEM_P43_DELTA_DELAY_MS must be a non-negative integer');
  let artifactPath = path.resolve(process.env.MAYHEM_P43_ARTIFACT || DEFAULT_ARTIFACT);
  if (providerMode === 'shim' && !process.env.MAYHEM_P43_ARTIFACT) {
    artifactPath = path.join(runDir, 'p43-shim-artifact.bin');
    fs.writeFileSync(
      artifactPath,
      `mayhem phase4 provider-kill deterministic artifact\n${tag}\n`,
      { mode: 0o644 }
    );
  }
  if (!fs.existsSync(artifactPath)) fail(`artifact missing: ${artifactPath}`);
  const macminiFile = path.resolve(ROOT, process.env.MAYHEM_P43_MACMINI_FILE || '../gpd/macmini.txt');

  log(`run dir: ${path.relative(ROOT, runDir)}`);
  log(`provider mode: ${providerMode}`);
  log('building mayhem CLI/gateway/enclave crates');
  const cargoEnv = { ...process.env };
  if (providerMode === 'real') {
    cargoEnv.CARGO_PROFILE_DEV_OPT_LEVEL =
      cargoEnv.MAYHEM_P43_CARGO_OPT_LEVEL ||
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
  let secondaryModel = null;
  if (agentLoopMode) {
    secondaryModel = structuredClone(model);
    secondaryModel.model_id = secondaryModelId;
    secondaryModel.family = `${secondaryModel.family || 'mayhem'}-e13-helper`;
    catalog.models.push(secondaryModel);
  }
  const tempCatalogDir = path.join(runDir, 'catalog');
  await fsp.mkdir(tempCatalogDir, { recursive: true });
  const tempCatalogPath = path.join(tempCatalogDir, 'models.json');
  writeJson(tempCatalogPath, catalog);
  const manifestHash = sha256File(tempCatalogPath);

  const remote = parseMacmini(macminiFile);
  const localTmpDir = path.join(ROOT, '.mayhem-local/tmp');
  await fsp.mkdir(localTmpDir, { recursive: true });
  const passFile = path.join(localTmpDir, `macmini-p43-${tag}-${process.pid}.pass`);
  fs.writeFileSync(passFile, `${remote.pass}\n`, { mode: 0o600 });
  sensitiveFiles.push(passFile);
  cleanupState.remote = remote;
  cleanupState.passFile = passFile;
  await fsp.mkdir(path.dirname(SSH_OPTS[SSH_OPTS.length - 1].split('=')[1]), { recursive: true });

  log('checking Mac mini SSH and remote runtime');
  const remoteHome = (await ssh(remote, passFile, 'printf "%s\\n" "$HOME"')).stdout.trim();
  const remoteRootEnv = process.env.MAYHEM_P43_REMOTE_ROOT || '~/mayhem-macmini-p33';
	  const remoteRoot = remoteRootEnv.startsWith('~/')
	    ? path.posix.join(remoteHome, remoteRootEnv.slice(2))
	    : remoteRootEnv.replace('$HOME', remoteHome);
	  if (process.env.MAYHEM_P43_SKIP_STALE_CLEANUP !== '1') {
	    log('cleaning stale local p4.3 smoke peers');
	    await cleanupLocalSmokeProcesses({ allP43: true });
	    log('cleaning stale remote p4.3 smoke peers');
	    await cleanupRemoteSmokeProcesses(remote, passFile, { allP43: true });
	  }
	  const remoteRun = path.posix.join(remoteRoot, '.mayhem-local/p4.3-provider-kill', tag);
  const remoteLogs = path.posix.join(remoteRun, 'logs');
  cleanupState.remoteRun = remoteRun;
  const remoteCatalog = path.posix.join(remoteRun, 'catalog/models.json');
  const remoteArtifact = providerMode === 'real'
    ? path.posix.join(remoteRoot, '.mayhem-cache/artifacts', path.basename(artifactPath))
    : path.posix.join(remoteRun, 'artifacts', path.basename(artifactPath));
  const remoteDownloadsDir = path.posix.join(remoteRoot, '.mayhem-cache/artifacts');
  const localArtifactDir = path.dirname(artifactPath);
  const artifactSidecars = Object.entries(artifact.sidecars || {}).map(([name, sidecar]) => ({
    name,
    sidecar,
    local_path: path.resolve(localArtifactDir, sidecar.path),
    remote_path: path.posix.join(remoteDownloadsDir, providerSidecarCacheFile(artifactName, name, sidecar)),
  }));
  const remoteMayhem = path.posix.join(remoteRoot, 'target/debug/mayhem');
  const remoteWalletHelper = path.posix.join(remoteRoot, 'crates/mayhem-cli/src/wallet-helper.mjs');
  const remoteRules = path.posix.join(remoteRoot, 'RULES.md');
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
      `mkdir -p ${sh(path.posix.join(remoteRoot, 'target/debug'))} ${sh(path.posix.dirname(remoteArtifact))} ${sh(path.posix.dirname(remoteCatalog))} ${sh(path.posix.dirname(remoteWalletHelper))} ${sh(path.posix.dirname(remoteRules))} ${sh(path.posix.dirname(remoteIntercomMain))} ${sh(path.posix.dirname(remoteScBridgeFeature))} ${sh(path.posix.dirname(remoteDirectSessionFeature))} ${sh(remoteLogs)}`,
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

  log('copying temporary catalog, rules, and Intercom feature patches to Mac mini');
  await scpTo(remote, passFile, tempCatalogPath, remoteCatalog, { logFile: path.join(logsDir, 'scp-catalog.log') });
  const remoteArtifactSha = await remoteSha256(remote, passFile, remoteArtifact);
  if (remoteArtifactSha !== artifactSha256) {
    log(`copying ${providerMode} artifact to Mac mini`);
    await scpTo(remote, passFile, artifactPath, remoteArtifact, {
      logFile: path.join(logsDir, 'scp-artifact.log'),
      timeoutMs: providerMode === 'real' ? 900_000 : 180_000,
    });
  } else {
    log('Mac mini already has matching provider artifact');
  }
  for (const sidecar of artifactSidecars) {
    if (!fs.existsSync(sidecar.local_path)) {
      log(`sidecar ${sidecar.name} not staged locally at ${sidecar.local_path}; provider may download it`);
      continue;
    }
    const localSidecarSha = sha256File(sidecar.local_path);
    const remoteSidecarSha = await remoteSha256(remote, passFile, sidecar.remote_path);
    if (remoteSidecarSha !== localSidecarSha) {
      log(`copying sidecar ${sidecar.name} to Mac mini provider cache`);
      await scpTo(remote, passFile, sidecar.local_path, sidecar.remote_path, {
        logFile: path.join(logsDir, `scp-sidecar-${sidecar.name}.log`),
        timeoutMs: providerMode === 'real' ? 600_000 : 180_000,
      });
    }
  }
  await scpTo(remote, passFile, path.join(ROOT, 'crates/mayhem-cli/src/wallet-helper.mjs'), remoteWalletHelper, { logFile: path.join(logsDir, 'scp-wallet-helper.log') });
  await scpTo(remote, passFile, path.join(ROOT, 'RULES.md'), remoteRules, { logFile: path.join(logsDir, 'scp-rules.log') });
  await scpTo(remote, passFile, path.join(ROOT, 'intercom/src/main.js'), remoteIntercomMain, { logFile: path.join(logsDir, 'scp-intercom-main.log') });
  await scpTo(remote, passFile, path.join(ROOT, 'intercom/features/sc-bridge/index.js'), remoteScBridgeFeature, { logFile: path.join(logsDir, 'scp-sc-bridge.log') });
  await scpTo(remote, passFile, path.join(ROOT, 'intercom/features/direct-session/index.js'), remoteDirectSessionFeature, { logFile: path.join(logsDir, 'scp-direct-session.log') });

  const localNode = process.env.MAYHEM_P43_LOCAL_NODE_BIN || process.execPath;
  const localPear = path.join(os.homedir(), 'Library/Application Support/pear/current/by-arch/darwin-arm64/bin/pear-runtime');
  await preauthorizeLocalApps([localNode, localPear], path.join(logsDir, 'local-firewall.log'));

  const explicitPeerDhtBootstrap = (process.env.MAYHEM_P43_PEER_DHT_BOOTSTRAP || '').trim();
  const useLocalDht = /^(1|true|yes)$/i.test(process.env.MAYHEM_P43_USE_LOCAL_DHT || '');
  const usePublicDht = /^(1|true|yes)$/i.test(process.env.MAYHEM_P43_USE_PUBLIC_DHT || '1');
  const useRemoteDht = !useLocalDht && !usePublicDht && /^(1|true|yes)$/i.test(process.env.MAYHEM_P43_USE_REMOTE_DHT || '');
  let localPeerDht = null;
  let peerDhtBootstrap = explicitPeerDhtBootstrap;
  if (!peerDhtBootstrap && useLocalDht) {
    const peerDhtPort = await freePort();
    const peerDhtHost = localPeerDhtHost(remote.host);
    const localPeerDhtLog = path.join(logsDir, 'local-peer-dht.log');
    peerDhtBootstrap = `${peerDhtHost}:${peerDhtPort}`;
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
    const remotePeerDhtPid = (await ssh(remote, passFile, remotePeerDhtCmd)).stdout.trim();
    cleanupState.remotePids.push(remotePeerDhtPid);
    const remotePeerDhtWait = `for i in $(seq 1 120); do grep -q 'Fully started Hyperswarm DHT bootstrap node' ${sh(remotePeerDhtLog)} && exit 0; kill -0 ${remotePeerDhtPid} >/dev/null 2>&1 || exit 2; sleep 0.5; done; exit 1`;
    await ssh(remote, passFile, remotePeerDhtWait, { timeoutMs: 75_000 });
  } else if (peerDhtBootstrap) {
    log(`using explicit peer DHT bootstrap ${peerDhtBootstrap}`);
  } else {
    log('using built-in public/default HyperDHT bootstrap nodes (no peer DHT override)');
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
        MAYHEM_DEVNET_JOINERS: '1',
        MAYHEM_DEVNET_SUBNET_CHANNEL: subnetChannel,
        ...(peerDhtBootstrap ? { MAYHEM_DEVNET_PEER_DHT_BOOTSTRAP: peerDhtBootstrap } : {}),
        MAYHEM_DEVNET_REPLICATE_FLUSH_TIMEOUT_MS: '5000',
        SESSION_DEBUG: '1',
      },
    }
  );
  cleanupState.localChildren.push(devnet);
  cleanupState.devnetLog = devnetLog;
  await waitForFilePattern(devnetLog, /Mayhem dev-net ready\./, 180_000, 'local dev-net', async () => devnet.exitCode === null);
  const devnetText = fs.readFileSync(devnetLog, 'utf8');
  const adminWs = devnetText.match(/admin:\s+(ws:\/\/[^\s]+).*rpc=(http:\/\/[^\s]+)/);
  const joinerWs = devnetText.match(/joiner-a:\s+(ws:\/\/[^\s]+).*rpc=(http:\/\/[^\s]+)/);
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
  const adminRpcUrl = adminWs[2];
  const adminRpcPort = new URL(adminRpcUrl).port;
  const scToken = token[1];

  const roomNonce = `p4.3-${tag}`;
  const artifactSidecarRoots = Object.fromEntries(
    Object.entries(artifact.sidecars || {}).map(([name, sidecar]) => [name, sidecar.artifact_root])
  );
  const enclaveId = await catalogEnclaveId({
    adminPubkey: adminPubkey[1],
    modelId,
    artifactRoot: artifactMerkle.root,
    artifactSidecarRoots,
    manifestHash,
    binaryHash,
  });
  const roomId = (await b3(Buffer.from(`${enclaveId}${adminPubkey[1]}${roomNonce}`, 'utf8'))).toString('hex').slice(0, 32);
  const secondaryRoomNonce = `p4.3-e13-${tag}`;
  const secondaryEnclaveId = agentLoopMode
    ? await catalogEnclaveId({
      adminPubkey: adminPubkey[1],
      modelId: secondaryModelId,
      artifactRoot: artifactMerkle.root,
      artifactSidecarRoots,
      manifestHash,
      binaryHash,
    })
    : null;
  const secondaryRoomId = agentLoopMode
    ? (await b3(Buffer.from(`${secondaryEnclaveId}${adminPubkey[1]}${secondaryRoomNonce}`, 'utf8'))).toString('hex').slice(0, 32)
    : null;

  log('seeding admin-created enclave, price, and room');
  const adminHome = path.join(runDir, 'admin-home');
  await fsp.mkdir(path.join(adminHome, 'stores'), { recursive: true });
  const adminStore = path.join(ROOT, 'intercom/stores/mayhem-devnet-admin');
  const adminLink = path.join(adminHome, 'stores/admin');
  await fsp.rm(adminLink, { recursive: true, force: true }).catch(() => {});
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
    '--label', 'phase4-provider-kill',
  ]);
  if (agentLoopMode) {
    adminRun('admin-set-model-ref-secondary', [
      'set-model-ref',
      '--model', secondaryModelId,
      '--rate-map-json', rateMapJson,
    ]);
    adminRun('admin-register-enclave-secondary', [
      'register-enclave',
      '--enclave-id', secondaryEnclaveId,
      '--model', secondaryModelId,
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
    adminRun('admin-set-price-secondary', [
      'set-price',
      '--enclave-id', secondaryEnclaveId,
      '--rate-map-json', rateMapJson,
      '--effective-at', '0',
    ]);
    adminRun('admin-open-room-secondary', [
      'open-room',
      '--enclave-id', secondaryEnclaveId,
      '--model', secondaryModelId,
      '--nonce', secondaryRoomNonce,
      '--label', 'phase4-e13-agent-loop-helper',
    ]);
  }
  const gatewayCreditMu = 10_000_000;
  const fiatDepositRef = (await b3(Buffer.from(`phase4-fiat-credit:${tag}:${userPubkey[1]}`, 'utf8'))).toString('hex');
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

  log('opening SSH reverse tunnel for remote provider contract RPC');
  const remoteAdminRpcTunnelPort = await remoteFreePort(remote, passFile);
  const tunnelLog = path.join(logsDir, 'ssh-reverse-tunnel.log');
  const tunnelBase = sshProgram(passFile);
  const tunnel = spawnLogged(
    tunnelBase.command,
    [
      ...tunnelBase.args,
      '-N',
      '-R', `127.0.0.1:${remoteAdminRpcTunnelPort}:127.0.0.1:${adminRpcPort}`,
      `${remote.user}@${remote.host}`,
    ],
    tunnelLog
  );
  cleanupState.localChildren.push(tunnel);
  await sleep(1500);
  if (tunnel.exitCode !== null) fail(`reverse tunnel exited early; see ${tunnelLog}`);

  const providerCommon = {
    remote,
    passFile,
    remoteRoot,
    remoteRun,
    remoteLogs,
    remoteHome,
    remoteNode,
    remotePear,
    remoteMayhem,
    remoteWalletHelper,
    remoteCatalog,
    remoteArtifact,
    remoteDownloadsDir,
    adminRpcTunnelPort: remoteAdminRpcTunnelPort,
    rulesHash,
    channel: channel[1],
    bootstrap: bootstrap[1],
    peerDhtBootstrap,
    scToken,
    enclaveId,
    delayMs,
    providerMode,
  };
  const providers = [
    await startRemoteProvider({ ...providerCommon, label: 'a' }),
    await startRemoteProvider({ ...providerCommon, label: 'b' }),
  ];
  const secondaryProvider = agentLoopMode
    ? await startRemoteProvider({
      ...providerCommon,
      label: 'c',
      enclaveId: secondaryEnclaveId,
      delayMs: 0,
    })
    : null;
  const allProviders = secondaryProvider ? [...providers, secondaryProvider] : providers;
  const providerPubkeys = allProviders.map((provider) => provider.pubkey);
  const primaryProviderPubkeys = providers.map((provider) => provider.pubkey);

  const remoteClientBridge = await startRemoteClientBridge({
    label: 'client',
    remote,
    passFile,
    remoteRoot,
    remoteRun,
    remoteLogs,
    remotePear,
    channel: channel[1],
    bootstrap: bootstrap[1],
    peerDhtBootstrap,
    scToken,
  });
  const remoteClientTunnel = await openRemoteScBridgeTunnel({
    remote,
    passFile,
    remoteScBridgeUrl: remoteClientBridge.scBridgeUrl,
    logsDir,
    label: 'client',
  });

  log('selecting SC-Bridge with direct connectivity to both providers');
  const userScBridge = await pickBridgeForProviders(
    [
      {
        label: 'macmini-client',
        url: remoteClientTunnel.url,
        remote: remoteClientBridge,
        tunnel_log: remoteClientTunnel.tunnel_log,
      },
      { label: 'joiner-a', url: joinerWs[1] },
      { label: 'admin', url: adminWs[1] },
    ],
    scToken,
    providerPubkeys,
    420_000
  );

  log('starting local contract-backed gateway');
  const gatewayPort = await freePort();
  const gatewayUrl = `http://127.0.0.1:${gatewayPort}`;
  const gatewayLog = path.join(logsDir, 'gateway.log');
  const gatewayHome = path.join(runDir, 'gateway-home');
  const receiptCheckpointTokens = envPositiveInt('MAYHEM_P43_RECEIPT_CHECKPOINT_TOKENS', 32);
  const receiptCheckpointMs = envPositiveInt('MAYHEM_P43_RECEIPT_CHECKPOINT_MS', 30000);
  const sessionOpenTimeoutSeconds = envPositiveInt(
    'MAYHEM_P43_SESSION_OPEN_TIMEOUT_SECONDS',
    agentLoopMode ? (providerMode === 'real' ? 60 : 30) : 3
  );
  await fsp.mkdir(path.join(gatewayHome, 'stores'), { recursive: true });
  const gatewayStore = path.join(ROOT, 'intercom/stores/mayhem-devnet-joiner-a');
  const gatewayStoreLink = path.join(gatewayHome, 'stores/main');
  await fsp.rm(gatewayStoreLink, { recursive: true, force: true });
  await fsp.symlink(gatewayStore, gatewayStoreLink);
  const gateway = spawnLogged(
    mayhemBin,
    [
      'use',
      '--home', gatewayHome,
      '--rpc-url', adminRpcUrl,
      '--sc-bridge-url', userScBridge.url,
      '--sc-bridge-token', scToken,
      '--session-open-timeout-seconds', String(sessionOpenTimeoutSeconds),
      '--session-ttft-timeout-seconds', providerMode === 'real' ? '300' : '60',
      '--session-frame-timeout-seconds', '15',
      '--receipt-checkpoint-tokens', String(receiptCheckpointTokens),
      '--receipt-checkpoint-ms', String(receiptCheckpointMs),
      '--dev-catalog-path', tempCatalogPath,
      '--dev-skip-catalog-verify',
      '--bind', `127.0.0.1:${gatewayPort}`,
      '--json',
    ],
    gatewayLog
  );
  cleanupState.localChildren.push(gateway);
  await waitHttp(`${gatewayUrl}/mayhem/status`, 120_000, 'local gateway', async () => gateway.exitCode === null);
  const routeInfo = await waitForGatewayRoutes(gatewayUrl, modelId, primaryProviderPubkeys, 120_000);
  const secondaryRouteInfo = agentLoopMode
    ? await waitForGatewayRoutes(gatewayUrl, secondaryModelId, [secondaryProvider.pubkey], 120_000)
    : null;

  const chatMaxTokens = envPositiveInt('MAYHEM_P43_CHAT_MAX_TOKENS', agentLoopMode ? 32 : 128);
  if (agentLoopMode) {
    log('running E13 multi-model conversation affinity smoke');
    const conversationId = process.env.MAYHEM_P43_CONVERSATION_ID || `e13-agent-loop-${tag}`;
    const agentUser = process.env.MAYHEM_P43_AGENT_USER || `e13-user-${tag}`;
    async function runAgentTurn(name, turnModelId, prompt) {
      const before = await fetchStoredReceipts(gatewayUrl);
      const stream = await runStreamingChatSmoke(gatewayUrl, turnModelId, runDir, {
        timeoutMs: providerMode === 'real' ? 240_000 : 90_000,
        maxTokens: chatMaxTokens,
        prompt,
        fileStem: `e13-${name}`,
        metadata: { conversation_id: conversationId },
        user: agentUser,
      });
      const after = await fetchStoredReceipts(gatewayUrl);
      const { entry, body } = newFinalReceiptForModel(before.length, after, turnModelId);
      return {
        name,
        model_id: turnModelId,
        provider: body.provider,
        enclave_id: body.enclave_id,
        session_id: body.session_id,
        rail: body.rail,
        price_ver: body.price_ver,
        usage: body.usage,
        mu_owed_cum: body.mu_owed_cum,
        receipt_seq: body.seq,
        receipt_index: after.indexOf(entry),
        receipts_before: before.length,
        receipts_after: after.length,
        stream,
      };
    }

    const primaryFirst = await runAgentTurn(
      'primary-1',
      modelId,
      'Agent turn one. Answer with one concise sentence using the word mayhem and the color red.'
    );
    const primarySecond = await runAgentTurn(
      'primary-2',
      modelId,
      'Agent turn two in the same conversation. Answer with one concise sentence using mayhem and the color green.'
    );
    const helperTurn = await runAgentTurn(
      'helper-model',
      secondaryModelId,
      'Switch models for one helper step. Answer with one concise sentence using mayhem and the color yellow.'
    );

    const stickyProvider = allProviders.find((provider) => provider.pubkey === primaryFirst.provider);
    if (!stickyProvider) fail(`sticky provider ${primaryFirst.provider} was not one of the live providers`);
    log(`killing sticky primary provider ${stickyProvider.label} before final primary turn`);
    await ssh(remote, passFile, `kill -9 ${Number(stickyProvider.providerPid)} >/dev/null 2>&1 || true`);
    await sleep(1_000);
    const primaryAfterKill = await runAgentTurn(
      'primary-after-kill',
      modelId,
      'Agent turn three after the sticky provider died. Answer with one concise sentence using mayhem and the color purple.'
    );

    const turns = [primaryFirst, primarySecond, helperTurn, primaryAfterKill];
    const sessionIds = new Set(turns.map((turn) => turn.session_id));
    const finalReceipts = await fetchStoredReceipts(gatewayUrl);
    const assertions = {
      same_model_affinity: primarySecond.provider === primaryFirst.provider,
      helper_uses_secondary_provider: helperTurn.provider === secondaryProvider.pubkey,
      helper_did_not_use_primary_provider: !primaryProviderPubkeys.includes(helperTurn.provider),
      post_kill_switched_provider:
        primaryAfterKill.provider !== primaryFirst.provider &&
        primaryProviderPubkeys.includes(primaryAfterKill.provider),
      all_turns_final_receipted: turns.every((turn) => turn.receipt_seq >= 1),
      sessions_are_distinct: sessionIds.size === turns.length,
      rails_are_fiat: turns.every((turn) => turn.rail === 'fiat'),
      gateway_process_stayed_running: gateway.exitCode === null,
    };
    for (const [name, ok] of Object.entries(assertions)) {
      if (!ok) fail(`E13 assertion failed: ${name}`);
    }

    const report = {
      ok: true,
      mode: 'e13-agent-loop',
      tag,
      run_dir: path.relative(ROOT, runDir),
      local: {
        gateway_url: gatewayUrl,
        admin_rpc_url: adminRpcUrl,
        user_sc_bridge_label: userScBridge.label,
        user_sc_bridge_url: userScBridge.url,
        user_sc_bridge_remote_url: userScBridge.remote?.scBridgeUrl || null,
        user_sc_bridge_tunnel_log: userScBridge.tunnel_log || null,
        subnet_channel: subnetChannel,
        peer_dht_bootstrap: peerDhtBootstrap,
      },
      remote: {
        host: remote.host,
        root: remoteRoot,
        run_dir: remoteRun,
        client_bridge: userScBridge.remote ? {
          label: userScBridge.remote.label,
          pubkey: userScBridge.remote.pubkey,
          peer_pid: userScBridge.remote.peerPid,
          peer_log: userScBridge.remote.peerLog,
        } : null,
      },
      admin: {
        pubkey: adminPubkey[1],
        rules_hash: rulesHash,
        primary: { model_id: modelId, enclave_id: enclaveId, room_nonce: roomNonce, room_id: roomId },
        secondary: {
          model_id: secondaryModelId,
          enclave_id: secondaryEnclaveId,
          room_nonce: secondaryRoomNonce,
          room_id: secondaryRoomId,
        },
        reports: Object.fromEntries(Object.entries(adminReports).map(([key, value]) => [key, {
          submitted: value.submitted,
          tx_type: value.tx_type,
          command: value.command,
        }])),
      },
      enclave: {
        artifact_name: artifactName,
        artifact_root: artifactMerkle.root,
        artifact_chunks: artifactMerkle.chunks,
        artifact_bytes: artifactMerkle.total_bytes,
        artifact_sha256: artifactSha256,
        manifest_hash: manifestHash,
        binary_hash: binaryHash,
        binary_sha256: binarySha256,
      },
      providers: allProviders.map((provider) => ({
        label: provider.label,
        pubkey: provider.pubkey,
        peer_pid: provider.peerPid,
        provider_pid: provider.providerPid,
        provider_log: provider.providerLog,
        self_test: provider.startup.self_test,
        rooms: provider.startup.rooms,
        features: provider.startup.features,
      })),
      route_candidates: {
        primary: routeInfo.routes,
        secondary: secondaryRouteInfo.routes,
      },
      conversation: {
        conversation_id: conversationId,
        user: agentUser,
        sticky_provider: {
          label: stickyProvider.label,
          pubkey: stickyProvider.pubkey,
          provider_pid: stickyProvider.providerPid,
        },
        turns,
      },
      receipts: {
        stored_count: finalReceipts.length,
        final_count: finalReceipts.filter(receiptIsFinal).length,
      },
      assertions,
    };
    const reportPath = path.join(runDir, 'report.json');
    writeJson(reportPath, report);
    console.log(JSON.stringify(report, null, 2));
    await cleanup({
      remote,
      passFile,
      remotePids: cleanupState.remotePids,
      localChildren: cleanupState.localChildren,
      devnetLog,
      remoteRun,
    });
    return;
  }

  log('starting streaming gateway request, then killing the first provider after checkpoint ack');
  const requestStartedAt = Date.now();
  let streamCompleted = false;
  const killPromise = waitAndKillAfterCheckpointAck(
    remote,
    passFile,
    providers,
    providerMode === 'real' ? 240_000 : 45_000
  );
  await sleep(500);
  const streamPromise = runStreamingChatSmoke(gatewayUrl, modelId, runDir, {
    timeoutMs: providerMode === 'real' ? 360_000 : 90_000,
    maxTokens: chatMaxTokens,
  }).finally(() => {
    streamCompleted = true;
  });
  const kill = await Promise.race([
    killPromise,
    streamPromise.then(() => {
      throw new Error('streaming gateway request completed before a checkpointed provider kill was observed');
    }),
  ]);
  const stream = await streamPromise;
  if (!streamCompleted) {
    fail('internal error: stream did not settle after provider kill');
  }
  const recoveryMs = requestStartedAt + stream.timing_ms.total - kill.killed_at_ms;

  const receipts = await (await fetch(`${gatewayUrl}/mayhem/receipts`)).json();
  const receiptsPath = path.join(runDir, 'gateway-receipts.json');
  writeJson(receiptsPath, receipts);
  const storedReceipts = Array.isArray(receipts.data) ? receipts.data : [];
  if (storedReceipts.length < 2) {
    fail(`expected checkpoint + final receipts after failover, got ${storedReceipts.length}`);
  }
  const killedReceipts = storedReceipts.filter((receipt) => storedReceiptBody(receipt).provider === kill.provider);
  const killedFinalReceipts = killedReceipts.filter(receiptIsFinal);
  if (killedReceipts.length !== 1) {
    fail(`expected exactly one killed-provider checkpoint receipt, got ${killedReceipts.length}`);
  }
  if (killedFinalReceipts.length !== 0) {
    fail(`killed provider produced ${killedFinalReceipts.length} final receipt(s)`);
  }
  const killedReceiptBody = storedReceiptBody(killedReceipts[0]);
  const killedReceiptSeq = Number(killedReceiptBody.seq ?? 0);
  if (!Number.isSafeInteger(killedReceiptSeq) || killedReceiptSeq < 1) {
    fail(`killed provider checkpoint has invalid seq ${killedReceiptBody.seq}`);
  }
  const killedOutputTokens = Number(killedReceiptBody.usage?.output_token ?? killedReceiptBody.usage?.completion_tokens ?? 0);
  const killedProviderLogText = (await ssh(remote, passFile, `cat ${sh(kill.provider_log)}`)).stdout;
  const killedProviderTail = providerLogOutputTailProof(killedProviderLogText, killedOutputTokens);
  if (killedProviderTail.observed_output_tokens === null) {
    fail(`could not prove killed-provider output tail from ${kill.provider_log}`);
  }
  if (killedOutputTokens > killedProviderTail.observed_output_tokens) {
    fail(`killed provider checkpoint output ${killedOutputTokens} exceeds provider-observed output ${killedProviderTail.observed_output_tokens}`);
  }
  if (killedProviderTail.uncheckpointed_output_tokens > receiptCheckpointTokens) {
    fail(`killed provider uncheckpointed output tail ${killedProviderTail.uncheckpointed_output_tokens} exceeds checkpoint window ${receiptCheckpointTokens}`);
  }
  const finalReceipts = storedReceipts.filter(receiptIsFinal);
  if (finalReceipts.length !== 1) {
    fail(`expected exactly one final fallback receipt, got ${finalReceipts.length}`);
  }
  const receiptBody = storedReceiptBody(finalReceipts[0]);
  if (receiptBody.provider === kill.provider) {
    fail('final receipt belongs to killed provider; failover did not switch routes');
  }
  if (!providerPubkeys.includes(receiptBody.provider)) {
    fail(`final receipt provider ${receiptBody.provider} is not one of the two canonical providers`);
  }

  const report = {
    ok: true,
    tag,
    run_dir: path.relative(ROOT, runDir),
    local: {
      gateway_url: gatewayUrl,
        admin_rpc_url: adminRpcUrl,
        user_sc_bridge_label: userScBridge.label,
        user_sc_bridge_url: userScBridge.url,
        user_sc_bridge_remote_url: userScBridge.remote?.scBridgeUrl || null,
        user_sc_bridge_tunnel_log: userScBridge.tunnel_log || null,
        subnet_channel: subnetChannel,
        peer_dht_bootstrap: peerDhtBootstrap,
      },
      remote: {
        host: remote.host,
        root: remoteRoot,
        run_dir: remoteRun,
        client_bridge: userScBridge.remote ? {
          label: userScBridge.remote.label,
          pubkey: userScBridge.remote.pubkey,
          peer_pid: userScBridge.remote.peerPid,
          peer_log: userScBridge.remote.peerLog,
        } : null,
      },
    admin: {
      pubkey: adminPubkey[1],
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
    providers: providers.map((provider) => ({
      label: provider.label,
      pubkey: provider.pubkey,
      peer_pid: provider.peerPid,
      provider_pid: provider.providerPid,
      provider_log: provider.providerLog,
      self_test: provider.startup.self_test,
      rooms: provider.startup.rooms,
      features: provider.startup.features,
    })),
    route_candidates: routeInfo.routes,
      kill,
      request: {
        started_at_ms: requestStartedAt,
        completed_at_ms: requestStartedAt + stream.timing_ms.total,
        recovery_ms: recoveryMs,
        stream,
        response_provider: receiptBody.provider,
        response_session: receiptBody.session_id || null,
      },
      receipts: {
        path: receiptsPath,
        stored_count: storedReceipts.length,
        killed_provider_receipts: killedReceipts.length,
        killed_provider_final_receipts: killedFinalReceipts.length,
        killed_checkpoint: killedReceiptBody,
        killed_checkpoint_seq: killedReceiptSeq,
        killed_checkpoint_output_tokens: killedOutputTokens,
        killed_provider_tail: killedProviderTail,
        final: receiptBody,
        checkpoint_every:
          storedReceipts[0]?.voucher?.checkpoint_every ||
          storedReceipts[0]?.voucher?.body?.checkpoint_every ||
          null,
      },
      assertions: {
        provider_killed_with_sigkill: true,
        stream_done_seen: stream.done_seen === true,
        checkpointed_killed_receipt: killedReceipts.length === 1 && killedFinalReceipts.length === 0,
        killed_uncheckpointed_tail_within_window:
          killedProviderTail.uncheckpointed_output_tokens !== null &&
          killedProviderTail.uncheckpointed_output_tokens <= receiptCheckpointTokens,
        exactly_one_final_receipt: finalReceipts.length === 1,
        fallback_provider_receipted: receiptBody.provider !== kill.provider,
      },
    };
  const reportPath = path.join(runDir, 'report.json');
  writeJson(reportPath, report);
  console.log(JSON.stringify(report, null, 2));

    await cleanup({
      remote,
      passFile,
      remotePids: cleanupState.remotePids,
      localChildren: cleanupState.localChildren,
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
	        await ssh(remote, passFile, `pkill -TERM -P ${Number(pid)} >/dev/null 2>&1 || true`);
	        await ssh(remote, passFile, `kill ${Number(pid)} >/dev/null 2>&1 || true`);
	      } catch {}
	    }
	  }
	  await cleanupRemoteSmokeProcesses(remote, passFile, { remoteRun });
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
  if (process.env.MAYHEM_P43_KEEP_REMOTE === '0' && remote && passFile && fs.existsSync(passFile) && remoteRun) {
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
  console.error(`[p4.3] ERROR: ${err.stack || err.message}`);
  await cleanup(cleanupState);
  process.exit(1);
});

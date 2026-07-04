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
const CHUNK_SIZE = 8 * 1024 * 1024;
const SSH_OPTS = [
  '-F', '/dev/null',
  '-o', 'IdentitiesOnly=yes',
  '-o', 'PreferredAuthentications=password,keyboard-interactive',
  '-o', 'PubkeyAuthentication=no',
  '-o', 'PasswordAuthentication=yes',
  '-o', 'KbdInteractiveAuthentication=yes',
  '-o', 'NumberOfPasswordPrompts=3',
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
  MAYHEM_P43_DELTA_DELAY_MS  Delay after first content delta in each provider (default: 2500)
  MAYHEM_P43_LOCAL_DHT_HOST  Local LAN IP advertised to the Mac mini DHT peer
  MAYHEM_P43_PEER_DHT_BOOTSTRAP Explicit peer DHT bootstrap list
  MAYHEM_P43_USE_LOCAL_DHT   Start/use a temporary local HyperDHT bootstrap (default: 0)
  MAYHEM_P43_USE_REMOTE_DHT  Start/use a temporary Mac mini HyperDHT bootstrap (default: 1)
  MAYHEM_P43_USE_PUBLIC_DHT  Use default/public peer DHT bootstrap instead of private DHT (default: 0)
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

function writeJson(file, value) {
  fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`);
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

async function catalogEnclaveId({ adminPubkey, modelId, artifactRoot, manifestHash, binaryHash }) {
  return (await b3(Buffer.from(`${adminPubkey}${modelId}${artifactRoot}${manifestHash}${binaryHash}`, 'utf8'))).toString('hex');
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
  while (Date.now() < deadline) {
    for (const candidate of candidates) {
      try {
        const ready = [];
        for (const provider of providers) {
          ready.push(await waitBridgePeerConnect(
            candidate.url,
            token,
            provider,
            Math.min(20_000, Math.max(1_000, deadline - Date.now())),
            candidate.label
          ));
          ready.push(await waitBridgeSessionOpen(candidate.url, token, provider, 10_000, candidate.label));
        }
        return { ...candidate, ready };
      } catch (err) {
        errors.push(`${candidate.label}: ${err?.message || err}`);
      }
    }
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

async function rsyncTo(remote, passFile, localPath, remotePath, options = {}) {
  const rsh = ['sshpass', '-f', passFile, 'ssh', ...SSH_OPTS].map(sh).join(' ');
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

async function waitAndKillFirstDelta(remote, passFile, providers, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  const pattern = /delaying after content s\.delta #0|sending content s\.delta #0/;
  let last = {};
  while (Date.now() < deadline) {
    for (const provider of providers) {
      const text = (await ssh(
        remote,
        passFile,
        `test -f ${sh(provider.providerLog)} && tail -n 80 ${sh(provider.providerLog)} || true`
      )).stdout;
      last[provider.label] = text.split('\n').slice(-10).join('\n');
      if (pattern.test(text)) {
        const killedAt = Date.now();
        await ssh(remote, passFile, `kill -9 ${Number(provider.providerPid)} >/dev/null 2>&1 || true`);
        return {
          label: provider.label,
          provider: provider.pubkey,
          provider_pid: provider.providerPid,
          provider_log: provider.providerLog,
          killed_at_ms: killedAt,
          trigger: 'first_content_delta',
        };
      }
    }
    await sleep(100);
  }
  throw new Error(`no provider emitted first content delta before timeout; last tails=${JSON.stringify(last, null, 2)}`);
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
  adminRpcTunnelPort,
  rulesHash,
  channel,
  bootstrap,
  peerDhtBootstrap,
  scToken,
  enclaveId,
  delayMs,
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
  const remoteWaitCmd = `for i in $(seq 1 900); do grep -q 'Sidechannel: ready' ${sh(remotePeerLog)} && exit 0; kill -0 ${remotePeerPid} >/dev/null 2>&1 || exit 2; sleep 0.5; done; exit 1`;
  await ssh(remote, passFile, remoteWaitCmd, { timeoutMs: 480_000 });

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
    '--engine-backend llama.cpp',
    '--skip-disk-bench',
    `--chunk-size ${CHUNK_SIZE}`,
    '--serve-sessions',
    '--serve-sessions-seconds 900',
    '--print-json',
    '--dev-skip-catalog-verify',
    '--dev-session-shim',
  ];
  const providerCmd = [
    prepareHome,
    `cd ${sh(remoteRoot)}`,
    [
      `MAYHEM_PROVIDER_SESSION_DEBUG=1`,
      `MAYHEM_PROVIDER_SESSION_DELTA_DELAY_MS=${Number(delayMs)}`,
      `MAYHEM_PROVIDER_SESSION_DELTA_DELAY_COUNT=1`,
      `MAYHEM_WALLET_HELPER=${sh(remoteWalletHelper)}`,
      `MAYHEM_NODE_BIN=${sh(remoteNode)}`,
      `nohup ${sh(remoteMayhem)} provider start ${providerFlags.join(' ')} > ${sh(providerLog)} 2>&1 & echo $!`,
    ].join(' '),
  ].join(' && ');
  const providerPid = (await ssh(remote, passFile, providerCmd)).stdout.trim();
  cleanupState.remotePids.push(providerPid);
  const providerWaitCmd = `for i in $(seq 1 1200); do grep -q '"self_test"' ${sh(providerLog)} && exit 0; kill -0 ${providerPid} >/dev/null 2>&1 || exit 2; sleep 0.5; done; exit 1`;
  await ssh(remote, passFile, providerWaitCmd, { timeoutMs: 610_000 });
  const startupText = (await ssh(remote, passFile, `sed -n '1,260p' ${sh(providerLog)}`)).stdout;
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
  const delayMs = Number.parseInt(process.env.MAYHEM_P43_DELTA_DELAY_MS || '2500', 10);
  if (!Number.isSafeInteger(delayMs) || delayMs < 0) fail('MAYHEM_P43_DELTA_DELAY_MS must be a non-negative integer');
  const artifactPath = path.join(runDir, 'p43-shim-artifact.bin');
  fs.writeFileSync(
    artifactPath,
    `mayhem phase4 provider-kill deterministic artifact\n${tag}\n`,
    { mode: 0o644 }
  );
  const macminiFile = path.resolve(ROOT, process.env.MAYHEM_P43_MACMINI_FILE || '../gpd/macmini.txt');

  log(`run dir: ${path.relative(ROOT, runDir)}`);
  log('building mayhem CLI/gateway/enclave crates');
  runSync('cargo', ['build', '-q', '-p', 'mayhem-cli', '-p', 'mayhem-gateway', '-p', 'mayhem-enclave']);
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
  const passFile = path.join(localTmpDir, `macmini-p43-${tag}-${process.pid}.pass`);
  fs.writeFileSync(passFile, remote.pass, { mode: 0o600 });
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
  const remoteRun = path.posix.join(remoteRoot, '.mayhem-local/p4.3-provider-kill', tag);
  const remoteLogs = path.posix.join(remoteRun, 'logs');
  cleanupState.remoteRun = remoteRun;
  const remoteCatalog = path.posix.join(remoteRun, 'catalog/models.json');
  const remoteArtifact = path.posix.join(remoteRun, 'artifacts', path.basename(artifactPath));
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

  log('copying temporary catalog, artifact, and Intercom feature patches to Mac mini');
  await scpTo(remote, passFile, tempCatalogPath, remoteCatalog, { logFile: path.join(logsDir, 'scp-catalog.log') });
  await scpTo(remote, passFile, artifactPath, remoteArtifact, { logFile: path.join(logsDir, 'scp-artifact.log') });
  await scpTo(remote, passFile, path.join(ROOT, 'crates/mayhem-cli/src/wallet-helper.mjs'), remoteWalletHelper, { logFile: path.join(logsDir, 'scp-wallet-helper.log') });
  await scpTo(remote, passFile, path.join(ROOT, 'intercom/src/main.js'), remoteIntercomMain, { logFile: path.join(logsDir, 'scp-intercom-main.log') });
  await scpTo(remote, passFile, path.join(ROOT, 'intercom/features/sc-bridge/index.js'), remoteScBridgeFeature, { logFile: path.join(logsDir, 'scp-sc-bridge.log') });
  await scpTo(remote, passFile, path.join(ROOT, 'intercom/features/direct-session/index.js'), remoteDirectSessionFeature, { logFile: path.join(logsDir, 'scp-direct-session.log') });

  const localNode = process.env.MAYHEM_P43_LOCAL_NODE_BIN || process.execPath;
  const localPear = path.join(os.homedir(), 'Library/Application Support/pear/current/by-arch/darwin-arm64/bin/pear-runtime');
  await preauthorizeLocalApps([localNode, localPear], path.join(logsDir, 'local-firewall.log'));

  const explicitPeerDhtBootstrap = (process.env.MAYHEM_P43_PEER_DHT_BOOTSTRAP || '').trim();
  const useLocalDht = /^(1|true|yes)$/i.test(process.env.MAYHEM_P43_USE_LOCAL_DHT || '');
  const usePublicDht = /^(1|true|yes)$/i.test(process.env.MAYHEM_P43_USE_PUBLIC_DHT || '');
  const useRemoteDht = !useLocalDht && !usePublicDht && /^(1|true|yes)?$/i.test(process.env.MAYHEM_P43_USE_REMOTE_DHT || '1');
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
    log(`using default peer DHT bootstrap from ${localPeerDhtHost(remote.host)}`);
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
  const adminPubkey = fs.readFileSync(adminPeerLog, 'utf8').match(/Peer pubkey \(hex\):\s+([0-9a-f]{64})/);
  if (!adminPubkey) fail(`could not parse admin pubkey from ${adminPeerLog}`);
  const adminRpcUrl = adminWs[2];
  const adminRpcPort = new URL(adminRpcUrl).port;
  const scToken = token[1];

  const roomNonce = `p4.3-${tag}`;
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
    '--in-per-1k-mu', String(model.price_ref_mu?.in_per_1k || 18),
    '--out-per-1k-mu', String(model.price_ref_mu?.out_per_1k || 55),
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
    '--manifest-hash', manifestHash,
    '--binary-hash', binaryHash,
    '--caps-json', '{"chat":true,"tools":true,"json":true,"ctx":8192}',
  ]);
  adminRun('admin-set-price', [
    'set-price',
    '--enclave-id', enclaveId,
    '--in-per-1k-mu', String(model.price_ref_mu?.in_per_1k || 18),
    '--out-per-1k-mu', String(model.price_ref_mu?.out_per_1k || 55),
    '--effective-at', '0',
  ]);
  adminRun('admin-open-room', [
    'open-room',
    '--enclave-id', enclaveId,
    '--model', modelId,
    '--nonce', roomNonce,
    '--label', 'phase4-provider-kill',
  ]);

  log('opening SSH reverse tunnel for remote provider contract RPC');
  const remoteAdminRpcTunnelPort = await remoteFreePort(remote, passFile);
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
    adminRpcTunnelPort: remoteAdminRpcTunnelPort,
    rulesHash,
    channel: channel[1],
    bootstrap: bootstrap[1],
    peerDhtBootstrap,
    scToken,
    enclaveId,
    delayMs,
  };
  const providers = [
    await startRemoteProvider({ ...providerCommon, label: 'a' }),
    await startRemoteProvider({ ...providerCommon, label: 'b' }),
  ];
  const providerPubkeys = providers.map((provider) => provider.pubkey);

  log('selecting local SC-Bridge with direct connectivity to both providers');
  const userScBridge = await pickBridgeForProviders(
    [
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
  await fsp.mkdir(gatewayHome, { recursive: true });
  const gateway = spawnLogged(
    mayhemBin,
    [
      'use',
      '--home', gatewayHome,
      '--rpc-url', adminRpcUrl,
      '--sc-bridge-url', userScBridge.url,
      '--sc-bridge-token', scToken,
      '--session-open-timeout-seconds', '3',
      '--session-frame-timeout-seconds', '15',
      '--bind', `127.0.0.1:${gatewayPort}`,
      '--json',
    ],
    gatewayLog
  );
  cleanupState.localChildren.push(gateway);
  await waitHttp(`${gatewayUrl}/mayhem/status`, 120_000, 'local gateway', async () => gateway.exitCode === null);
  const routeInfo = await waitForGatewayRoutes(gatewayUrl, modelId, providerPubkeys, 120_000);

  log('starting gateway request, then killing the first provider that emits a content delta');
  const requestBody = {
    model: modelId,
    messages: [{ role: 'user', content: 'p4 recovery' }],
    max_tokens: 16,
    stream: false,
  };
  const requestStartedAt = Date.now();
  const responsePromise = fetch(`${gatewayUrl}/v1/chat/completions`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(requestBody),
  }).then(async (response) => {
    const text = await response.text();
    let body = null;
    try {
      body = JSON.parse(text);
    } catch {}
    return {
      status: response.status,
      ok: response.ok,
      body,
      text,
      completed_at_ms: Date.now(),
    };
  });
  const kill = await waitAndKillFirstDelta(remote, passFile, providers, 25_000);
  const response = await Promise.race([
    responsePromise,
    sleep(45_000).then(() => {
      throw new Error('gateway request timed out after provider kill');
    }),
  ]);
  if (!response.ok) {
    fail(`gateway request failed after provider kill: status=${response.status} body=${response.text}`);
  }
  const recoveryMs = response.completed_at_ms - kill.killed_at_ms;
  if (recoveryMs >= 20_000) {
    fail(`P4.3 recovery took ${recoveryMs}ms, expected < 20000ms`);
  }

  const receipts = await (await fetch(`${gatewayUrl}/mayhem/receipts`)).json();
  const receiptsPath = path.join(runDir, 'gateway-receipts.json');
  writeJson(receiptsPath, receipts);
  const storedReceipts = Array.isArray(receipts.data) ? receipts.data : [];
  if (storedReceipts.length !== 1) {
    fail(`expected exactly one stored receipt after failover, got ${storedReceipts.length}`);
  }
  const receiptBody = storedReceipts[0]?.receipt?.body || storedReceipts[0]?.receipt || {};
  if (receiptBody.provider === kill.provider) {
    fail('stored receipt belongs to killed provider; failover double-billed the dead route');
  }
  if (!providerPubkeys.includes(receiptBody.provider)) {
    fail(`stored receipt provider ${receiptBody.provider} is not one of the two canonical providers`);
  }
  const killedReceiptCount = storedReceipts.filter((receipt) => {
    const body = receipt?.receipt?.body || receipt?.receipt || {};
    return body.provider === kill.provider;
  }).length;
  if (killedReceiptCount !== 0) {
    fail(`killed provider produced ${killedReceiptCount} stored receipt(s)`);
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
      subnet_channel: subnetChannel,
      peer_dht_bootstrap: peerDhtBootstrap,
    },
    remote: {
      host: remote.host,
      root: remoteRoot,
      run_dir: remoteRun,
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
      completed_at_ms: response.completed_at_ms,
      recovery_ms: recoveryMs,
      response_status: response.status,
      response_id: response.body?.id || null,
      response_provider: receiptBody.provider,
      response_session: receiptBody.session_id || null,
    },
    receipts: {
      path: receiptsPath,
      stored_count: storedReceipts.length,
      killed_provider_receipts: killedReceiptCount,
      latest: receiptBody,
      checkpoint_every: storedReceipts[0]?.voucher?.body?.checkpoint_every || null,
    },
    assertions: {
      provider_killed_with_sigkill: true,
      recovery_under_20s: recoveryMs < 20_000,
      one_stored_receipt: storedReceipts.length === 1,
      killed_provider_receipts_zero: killedReceiptCount === 0,
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

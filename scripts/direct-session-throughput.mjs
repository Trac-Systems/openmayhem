#!/usr/bin/env node
import crypto from 'node:crypto';
import process from 'node:process';
import { setTimeout as sleep } from 'node:timers/promises';

const DEFAULT_TOKEN_FRAMES = 180;
const DEFAULT_PAYLOAD_BYTES = 4096;
const DEFAULT_MIN_TOKEN_FRAMES_PER_SECOND = 30;
const DEFAULT_MIN_BYTES_PER_SECOND = 128 * 1024;
const DEFAULT_WINDOW = 1;
const DEFAULT_TIMEOUT_SECONDS = 60;
const DEFAULT_WAIT_CONNECTIONS_SECONDS = 120;

function usage() {
  console.log(`Usage:
  node scripts/direct-session-throughput.mjs \\
    --provider-url ws://HOST:PORT --provider-token TOKEN \\
    --user-url ws://HOST:PORT --user-token TOKEN [options]

Measures delivered direct mx/s session throughput between two running
SC-Bridge peers. It establishes the targeted peer connection before opening the
session. Tokens are synthetic session delta frames, not model output.

Required:
  --provider-url URL       Provider SC-Bridge WebSocket URL
  --provider-token TOKEN   Provider SC-Bridge auth token
  --user-url URL           User SC-Bridge WebSocket URL
  --user-token TOKEN       User SC-Bridge auth token

Options:
  --token-frames N         Session delta frames to send (default: ${DEFAULT_TOKEN_FRAMES})
  --payload-bytes N        Bytes in each synthetic token payload (default: ${DEFAULT_PAYLOAD_BYTES})
  --min-token-frames-per-second N
                            Minimum delivered token-frame rate (default: ${DEFAULT_MIN_TOKEN_FRAMES_PER_SECOND})
  --min-bytes-per-second N Minimum delivered JSON frame bytes/sec (default: ${DEFAULT_MIN_BYTES_PER_SECOND})
  --window N               In-flight session_send request window (default: ${DEFAULT_WINDOW})
  --session-id HEX         32-byte hex session id (default: random)
  --timeout-seconds N      Receive/send timeout (default: ${DEFAULT_TIMEOUT_SECONDS})
  --wait-connections-seconds N
                            Wait for direct peer connections (default: ${DEFAULT_WAIT_CONNECTIONS_SECONDS})
  --json                   Print only machine-readable JSON
`);
}

function parseArgs(argv) {
  const args = {};
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--help' || arg === '-h') {
      args.help = true;
      continue;
    }
    if (arg === '--json') {
      args.json = true;
      continue;
    }
    if (!arg.startsWith('--')) throw new Error(`unexpected argument: ${arg}`);
    const key = arg.slice(2);
    const value = argv[i + 1];
    if (!value || value.startsWith('--')) throw new Error(`${arg} requires a value`);
    args[key] = value;
    i += 1;
  }
  return args;
}

function positiveInt(value, name, fallback) {
  if (value === undefined || value === null || value === '') return fallback;
  const parsed = Number.parseInt(String(value), 10);
  if (!Number.isSafeInteger(parsed) || parsed <= 0) {
    throw new Error(`${name} must be a positive integer`);
  }
  return parsed;
}

function positiveNumber(value, name, fallback) {
  if (value === undefined || value === null || value === '') return fallback;
  const parsed = Number(String(value));
  if (!Number.isFinite(parsed) || parsed <= 0) throw new Error(`${name} must be positive`);
  return parsed;
}

function required(args, key) {
  const value = args[key];
  if (typeof value !== 'string' || value.trim() === '') throw new Error(`--${key} is required`);
  return value.trim();
}

function normalizeWebSocketUrl(raw, name) {
  const url = new URL(raw);
  if (url.protocol !== 'ws:' && url.protocol !== 'wss:') {
    throw new Error(`${name} must use ws:// or wss://`);
  }
  if (url.username || url.password) throw new Error(`${name} must not contain credentials`);
  return url.toString();
}

function frameBytes(frame) {
  return Buffer.byteLength(JSON.stringify(frame), 'utf8');
}

function makeSessionId(raw) {
  if (!raw) return crypto.randomBytes(32).toString('hex');
  const normalized = String(raw).trim().toLowerCase();
  if (!/^[0-9a-f]{64}$/.test(normalized)) {
    throw new Error('--session-id must be 32 bytes of hex');
  }
  return normalized;
}

function normalizeData(data) {
  if (typeof data === 'string') return data;
  if (data instanceof ArrayBuffer) return Buffer.from(data).toString('utf8');
  if (ArrayBuffer.isView(data)) return Buffer.from(data.buffer, data.byteOffset, data.byteLength).toString('utf8');
  return String(data);
}

class BridgeClient {
  constructor(label, url, token) {
    this.label = label;
    this.url = url;
    this.token = token;
    this.nextId = 1;
    this.pending = new Map();
    this.events = [];
    this.waiters = [];
    this.socket = null;
  }

  async connect(timeoutMs) {
    this.socket = new WebSocket(this.url);
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`${this.label} SC-Bridge open timed out`)), timeoutMs);
      this.socket.addEventListener('open', () => {
        clearTimeout(timer);
        resolve();
      }, { once: true });
      this.socket.addEventListener('error', () => {
        clearTimeout(timer);
        reject(new Error(`${this.label} SC-Bridge websocket error`));
      }, { once: true });
    });
    this.socket.addEventListener('message', (event) => this.handleMessage(event.data));
    this.socket.addEventListener('close', () => this.rejectAll(new Error(`${this.label} SC-Bridge closed`)));
    await this.request({ type: 'auth', token: this.token }, 'auth_ok', timeoutMs);
  }

  close() {
    try {
      this.socket?.close();
    } catch (_err) {}
  }

  rejectAll(err) {
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(err);
    }
    this.pending.clear();
    for (const waiter of this.waiters) {
      clearTimeout(waiter.timer);
      waiter.reject(err);
    }
    this.waiters = [];
  }

  handleMessage(data) {
    let message;
    try {
      message = JSON.parse(normalizeData(data));
    } catch (err) {
      this.rejectAll(new Error(`${this.label} returned invalid JSON: ${err.message}`));
      return;
    }

    if (Number.isInteger(message.id) && this.pending.has(message.id)) {
      const pending = this.pending.get(message.id);
      this.pending.delete(message.id);
      clearTimeout(pending.timer);
      if (message.type === 'error') {
        pending.reject(new Error(`${this.label} SC-Bridge error: ${message.error || 'unknown error'}`));
        return;
      }
      if (pending.expectedType && message.type !== pending.expectedType) {
        pending.reject(new Error(
          `${this.label} expected ${pending.expectedType}, got ${message.type || 'missing type'}`
        ));
        return;
      }
      pending.resolve(message);
      return;
    }

    this.events.push(message);
    this.drainWaiters();
  }

  drainWaiters() {
    for (let i = 0; i < this.waiters.length;) {
      const waiter = this.waiters[i];
      const idx = this.events.findIndex(waiter.predicate);
      if (idx < 0) {
        i += 1;
        continue;
      }
      const [event] = this.events.splice(idx, 1);
      clearTimeout(waiter.timer);
      this.waiters.splice(i, 1);
      waiter.resolve(event);
    }
  }

  request(payload, expectedType, timeoutMs) {
    const id = this.nextId;
    this.nextId += 1;
    const body = { id, ...payload };
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`${this.label} ${payload.type} timed out`));
      }, timeoutMs);
      this.pending.set(id, { expectedType, resolve, reject, timer });
      this.socket.send(JSON.stringify(body));
    });
  }

  waitForEvent(predicate, timeoutMs, description) {
    const idx = this.events.findIndex(predicate);
    if (idx >= 0) {
      const [event] = this.events.splice(idx, 1);
      return Promise.resolve(event);
    }
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        const idx = this.waiters.findIndex((waiter) => waiter.resolve === resolve);
        if (idx >= 0) this.waiters.splice(idx, 1);
        reject(new Error(`${this.label} timed out waiting for ${description}`));
      }, timeoutMs);
      this.waiters.push({ predicate, resolve, reject, timer });
    });
  }
}

async function waitForDirectConnections(provider, user, waitMs) {
  const deadline = Date.now() + waitMs;
  let lastProviderStats = null;
  let lastUserStats = null;
  while (Date.now() < deadline) {
    [lastProviderStats, lastUserStats] = await Promise.all([
      provider.request({ type: 'stats' }, 'stats', 5000),
      user.request({ type: 'stats' }, 'stats', 5000),
    ]);
    if (
      Number(lastProviderStats.connectionCount || 0) > 0 &&
      Number(lastUserStats.connectionCount || 0) > 0
    ) {
      return { provider: lastProviderStats, user: lastUserStats };
    }
    await sleep(500);
  }
  throw new Error(
    `timed out waiting for direct peer connections; provider=${JSON.stringify(lastProviderStats)} user=${JSON.stringify(lastUserStats)}`
  );
}

function peerPubkey(info, label) {
  const value = info?.info?.peerPubkey;
  if (typeof value !== 'string' || !/^[0-9a-f]{64}$/.test(value)) {
    throw new Error(`${label} SC-Bridge info missing peerPubkey`);
  }
  return value;
}

async function sendTokenFrames({
  provider,
  user,
  providerPubkey: _providerPubkey,
  userPubkey,
  sessionId,
  tokenFrames,
  payloadBytes,
  window,
  timeoutMs,
}) {
  const probeId = crypto.randomBytes(16).toString('hex');
  const payload = 'x'.repeat(payloadBytes);
  const seen = new Set();
  let deliveredFrameBytes = 0;
  let lastDeliveredAt = null;
  let nonDirect = 0;
  let relayed = 0;
  const start = performance.now();

  const receiver = (async () => {
    while (seen.size < tokenFrames) {
      const event = await user.waitForEvent(
        (candidate) =>
          candidate?.type === 'session_frame' &&
          candidate?.session_id === sessionId &&
          candidate?.frame?.t === 's.delta' &&
          candidate?.frame?.probe_id === probeId,
        timeoutMs,
        `session_frame ${seen.size + 1}/${tokenFrames}`
      );
      if (event.direct !== true) nonDirect += 1;
      if (event.relayed === true) relayed += 1;
      const idx = Number(event.frame.i);
      if (!Number.isSafeInteger(idx) || idx < 0 || idx >= tokenFrames || seen.has(idx)) continue;
      seen.add(idx);
      deliveredFrameBytes += frameBytes(event.frame);
      lastDeliveredAt = performance.now();
    }
  })();

  try {
    const inFlight = new Set();
    for (let i = 0; i < tokenFrames; i += 1) {
      const frame = {
        t: 's.delta',
        v: 1,
        session_id: sessionId,
        probe_id: probeId,
        i,
        d: payload,
        fin: i === tokenFrames - 1 ? 'stop' : null,
      };
      const pending = provider
        .request({
          type: 'session_send',
          remote: userPubkey,
          session_id: sessionId,
          frame,
        }, 'session_sent', timeoutMs)
        .finally(() => inFlight.delete(pending));
      inFlight.add(pending);
      if (inFlight.size >= window) await Promise.race(inFlight);
    }
    await Promise.all(inFlight);
    await receiver;
  } catch (err) {
    receiver.catch(() => {});
    throw err;
  }

  const elapsedMs = Math.max(1, (lastDeliveredAt ?? performance.now()) - start);
  return {
    probe_id: probeId,
    delivered_frames: seen.size,
    delivered_json_frame_bytes: deliveredFrameBytes,
    elapsed_ms: elapsedMs,
    token_frames_per_second: (seen.size * 1000) / elapsedMs,
    bytes_per_second: (deliveredFrameBytes * 1000) / elapsedMs,
    non_direct_frames: nonDirect,
    relayed_frames: relayed,
  };
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.help) {
    usage();
    return;
  }

  const providerUrl = normalizeWebSocketUrl(required(args, 'provider-url'), '--provider-url');
  const userUrl = normalizeWebSocketUrl(required(args, 'user-url'), '--user-url');
  const providerToken = required(args, 'provider-token');
  const userToken = required(args, 'user-token');
  const tokenFrames = positiveInt(args['token-frames'] ?? args.frames, '--token-frames', DEFAULT_TOKEN_FRAMES);
  const payloadBytes = positiveInt(args['payload-bytes'], '--payload-bytes', DEFAULT_PAYLOAD_BYTES);
  const minTokenFramesPerSecond = positiveNumber(
    args['min-token-frames-per-second'],
    '--min-token-frames-per-second',
    DEFAULT_MIN_TOKEN_FRAMES_PER_SECOND
  );
  const minBytesPerSecond = positiveNumber(
    args['min-bytes-per-second'],
    '--min-bytes-per-second',
    DEFAULT_MIN_BYTES_PER_SECOND
  );
  const window = positiveInt(args.window, '--window', DEFAULT_WINDOW);
  const timeoutMs = positiveInt(args['timeout-seconds'], '--timeout-seconds', DEFAULT_TIMEOUT_SECONDS) * 1000;
  const waitConnectionsMs = positiveInt(
    args['wait-connections-seconds'],
    '--wait-connections-seconds',
    DEFAULT_WAIT_CONNECTIONS_SECONDS
  ) * 1000;
  const sessionId = makeSessionId(args['session-id']);

  const provider = new BridgeClient('provider', providerUrl, providerToken);
  const user = new BridgeClient('user', userUrl, userToken);
  try {
    await Promise.all([provider.connect(timeoutMs), user.connect(timeoutMs)]);
    const [providerInfo, userInfo] = await Promise.all([
      provider.request({ type: 'info' }, 'info', timeoutMs),
      user.request({ type: 'info' }, 'info', timeoutMs),
    ]);
    const providerPubkey = peerPubkey(providerInfo, 'provider');
    const userPubkey = peerPubkey(userInfo, 'user');
    const targetedConnections = await Promise.all([
      provider.request({
        type: 'peer_connect',
        remote: userPubkey,
        wait_ms: waitConnectionsMs,
      }, 'peer_connected', waitConnectionsMs + 5000),
      user.request({
        type: 'peer_connect',
        remote: providerPubkey,
        wait_ms: waitConnectionsMs,
      }, 'peer_connected', waitConnectionsMs + 5000),
    ]);
    const connectionStats = await waitForDirectConnections(provider, user, waitConnectionsMs);
    await Promise.all([
      provider.request({ type: 'session_subscribe', session_ids: [sessionId] }, 'session_subscribed', timeoutMs),
      user.request({ type: 'session_subscribe', session_ids: [sessionId] }, 'session_subscribed', timeoutMs),
    ]);
    const userOpen = await user.request({
      type: 'session_open',
      remote: providerPubkey,
      session_id: sessionId,
    }, 'session_opened', timeoutMs);
    const providerOpen = await provider.request({
      type: 'session_open',
      remote: userPubkey,
      session_id: sessionId,
    }, 'session_opened', timeoutMs);
    const [providerSessionStats, userSessionStats] = await Promise.all([
      provider.request({ type: 'session_stats' }, 'session_stats', timeoutMs),
      user.request({ type: 'session_stats' }, 'session_stats', timeoutMs),
    ]);
    const sampleFrameBytes = frameBytes({
      t: 's.delta',
      v: 1,
      session_id: sessionId,
      probe_id: '0'.repeat(32),
      i: 0,
      d: 'x'.repeat(payloadBytes),
      fin: null,
    });
    if (Number(providerSessionStats.maxFrameBytes || 0) < sampleFrameBytes) {
      throw new Error(`provider maxFrameBytes ${providerSessionStats.maxFrameBytes} is below sample frame ${sampleFrameBytes}`);
    }
    if (Number(userSessionStats.maxFrameBytes || 0) < sampleFrameBytes) {
      throw new Error(`user maxFrameBytes ${userSessionStats.maxFrameBytes} is below sample frame ${sampleFrameBytes}`);
    }

    const measurement = await sendTokenFrames({
      provider,
      user,
      providerPubkey,
      userPubkey,
      sessionId,
      tokenFrames,
      payloadBytes,
      window,
      timeoutMs,
    });
    const ok =
      measurement.delivered_frames === tokenFrames &&
      measurement.non_direct_frames === 0 &&
      measurement.relayed_frames === 0 &&
      measurement.token_frames_per_second >= minTokenFramesPerSecond &&
      measurement.bytes_per_second >= minBytesPerSecond;
    const report = {
      ok,
      session: {
        session_id: sessionId,
        channel: `mx/s/${sessionId}`,
        direct: true,
        relayed: false,
      },
      peers: {
        provider_pubkey: providerPubkey,
        user_pubkey: userPubkey,
      },
      thresholds: {
        min_token_frames_per_second: minTokenFramesPerSecond,
        min_bytes_per_second: minBytesPerSecond,
      },
      config: {
        token_frames: tokenFrames,
        payload_bytes: payloadBytes,
        sample_json_frame_bytes: sampleFrameBytes,
        window,
      },
      measurement,
      session_stats: {
        provider: providerSessionStats,
        user: userSessionStats,
      },
      connection_stats: connectionStats,
      targeted_connections: {
        provider: targetedConnections[0],
        user: targetedConnections[1],
      },
      opened: {
        provider: providerOpen,
        user: userOpen,
      },
    };

    if (args.json) {
      console.log(JSON.stringify(report, null, 2));
    } else {
      console.log(`P3.3 direct session throughput: ${ok ? 'PASS' : 'FAIL'}`);
      console.log(`session: mx/s/${sessionId}`);
      console.log(`delivered: ${measurement.delivered_frames}/${tokenFrames} frames`);
      console.log(`token frames/sec: ${measurement.token_frames_per_second.toFixed(2)}`);
      console.log(`bytes/sec: ${measurement.bytes_per_second.toFixed(0)}`);
      console.log(`direct frames: ${measurement.non_direct_frames === 0 && measurement.relayed_frames === 0 ? 'yes' : 'no'}`);
      console.log('JSON report:');
      console.log(JSON.stringify(report, null, 2));
    }
    if (!ok) process.exitCode = 1;
  } finally {
    provider.close();
    user.close();
  }
}

main().catch((err) => {
  console.error(`direct-session-throughput failed: ${err.message}`);
  process.exitCode = 1;
});

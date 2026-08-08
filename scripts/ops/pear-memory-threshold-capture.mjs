#!/usr/bin/env node
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { execFileSync, spawnSync } from 'node:child_process';

const DEFAULT_THRESHOLDS = '4GiB,8GiB,16GiB,32GiB,48GiB,60GiB';
const DEFAULT_INTERVAL_MS = 15_000;
const DEFAULT_COOLDOWN_MS = 10 * 60_000;
const DEFAULT_HEAP_TIMEOUT_MS = 180_000;
const DEFAULT_MAX_CAPTURES = 8;
const DEFAULT_MAX_BYTES = 8 * 1024 ** 3;
const DEFAULT_BRIDGE_URL = 'ws://127.0.0.1:49222';
const DEFAULT_SERVICE = 'mayhem-stack.service';

const defaultDiagnosticRoot = () => path.join(os.homedir(), 'mayhem-diagnostics');
const defaultArtifactRoot = () => path.join(defaultDiagnosticRoot(), 'threshold-captures');
const defaultBridgeTokenFile = () => path.join(os.homedir(), '.mayhem-mainnet-provider', 'sc-bridge-token');

const redactKey = /TOKEN|SECRET|PASSWORD|BEARER|AUTH|PRIVATE|KEY/i;

const usage = () => `Usage: pear-memory-threshold-capture.mjs [options]

Options:
  --service <name>           systemd user service that owns pear-runtime
  --diagnostic-root <path>   root for external diagnostic files
  --artifact-root <path>     directory for captures
  --state-file <path>        threshold state file
  --thresholds <list>        comma list, e.g. 4GiB,8GiB,16GiB
  --interval-ms <n>          loop interval
  --cooldown-ms <n>          minimum time between captures for one PID
  --bridge-url <url>         local SC-Bridge URL
  --bridge-token-file <path> SC-Bridge token file
  --heap-timeout-ms <n>      heap snapshot request timeout
  --max-captures <n>         retention cap for capture directories
  --max-bytes <size>         retention cap including referenced heap snapshots
  --once                    run one sample and exit
  --force                   capture regardless of thresholds
  --no-heap                 skip heap snapshot, keep proc/bridge artifacts
  --json                    print JSON events
`;

const parseArgs = (argv) => {
  const out = {};
  for (let i = 0; i < argv.length; i += 1) {
    const raw = argv[i];
    if (!raw.startsWith('--')) throw new Error(`unexpected argument ${raw}`);
    const eq = raw.indexOf('=');
    if (eq !== -1) {
      out[raw.slice(2, eq)] = raw.slice(eq + 1);
      continue;
    }
    const key = raw.slice(2);
    const next = argv[i + 1];
    if (next !== undefined && !String(next).startsWith('--')) {
      out[key] = next;
      i += 1;
    } else {
      out[key] = true;
    }
  }
  return out;
};

const parseByteSize = (raw) => {
  const value = String(raw ?? '').trim();
  const match = value.match(/^([0-9]+(?:\.[0-9]+)?)(?:\s*)(b|kb|kib|mb|mib|gb|gib)?$/i);
  if (!match) throw new Error(`invalid byte size ${value}`);
  const number = Number(match[1]);
  if (!Number.isFinite(number) || number <= 0) throw new Error(`invalid byte size ${value}`);
  const unit = (match[2] || 'b').toLowerCase();
  const multipliers = {
    b: 1,
    kb: 1000,
    mb: 1000 ** 2,
    gb: 1000 ** 3,
    kib: 1024,
    mib: 1024 ** 2,
    gib: 1024 ** 3,
  };
  return Math.round(number * multipliers[unit]);
};

const parseThresholds = (raw = DEFAULT_THRESHOLDS) => {
  const thresholds = String(raw)
    .split(',')
    .map((part) => part.trim())
    .filter(Boolean)
    .map(parseByteSize)
    .sort((left, right) => left - right);
  if (thresholds.length === 0) throw new Error('at least one threshold is required');
  return [...new Set(thresholds)];
};

const parsePositiveInteger = (raw, fallback, name) => {
  if (raw === undefined || raw === null || raw === '') return fallback;
  const value = Number(raw);
  if (!Number.isSafeInteger(value) || value <= 0) {
    throw new Error(`${name} must be a positive safe integer`);
  }
  return value;
};

const compactIso = (now = new Date()) => now.toISOString().replace(/[:.]/g, '');

const thresholdLabel = (bytes) => {
  const gib = bytes / (1024 ** 3);
  return Number.isInteger(gib) ? `${gib}GiB` : `${bytes}B`;
};

const readText = (file) => {
  try {
    return fs.readFileSync(file, 'utf8');
  } catch (_error) {
    return '';
  }
};

const writePrivateFile = (file, value) => {
  fs.mkdirSync(path.dirname(file), { recursive: true, mode: 0o700 });
  fs.writeFileSync(file, value, { mode: 0o600 });
  try {
    fs.chmodSync(file, 0o600);
  } catch (_error) {}
};

const readJsonFile = (file, fallback) => {
  try {
    return JSON.parse(fs.readFileSync(file, 'utf8'));
  } catch (_error) {
    return fallback;
  }
};

const writeJsonFile = (file, value) => {
  writePrivateFile(file, `${JSON.stringify(value, null, 2)}\n`);
};

const pathContains = (parent, child) => {
  const relative = path.relative(parent, child);
  return relative === '' || (!!relative && !relative.startsWith('..') && !path.isAbsolute(relative));
};

const execText = (command, args, options = {}) => {
  try {
    return execFileSync(command, args, {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'pipe'],
      timeout: options.timeout ?? 10_000,
    }).trim();
  } catch (_error) {
    return '';
  }
};

const systemdMainPid = (service) => {
  const text = execText('systemctl', ['--user', 'show', '-p', 'MainPID', '--value', service]);
  const pid = Number(text);
  return Number.isSafeInteger(pid) && pid > 1 ? pid : null;
};

const pearChildPid = (service = DEFAULT_SERVICE) => {
  const parent = systemdMainPid(service);
  if (!parent) return null;
  const text = execText('pgrep', ['-P', String(parent), 'pear-runtime']);
  const pid = Number(String(text).split(/\s+/)[0]);
  return Number.isSafeInteger(pid) && pid > 1 ? { parent, pid } : { parent, pid: null };
};

const parseProcStatus = (text) => {
  const out = {};
  for (const line of text.split('\n')) {
    const match = line.match(/^([^:]+):\s+([0-9]+)\s+kB$/);
    if (match) out[match[1]] = Number(match[2]) * 1024;
  }
  return out;
};

const parseSmapsRollup = (text) => {
  const out = {};
  for (const line of text.split('\n')) {
    const match = line.match(/^([^:]+):\s+([0-9]+)\s+kB$/);
    if (match) out[match[1].toLowerCase()] = Number(match[2]) * 1024;
  }
  return out;
};

const samplePearMemory = (pid) => {
  const statusText = readText(`/proc/${pid}/status`);
  const smapsText = readText(`/proc/${pid}/smaps_rollup`);
  const status = parseProcStatus(statusText);
  const smaps = parseSmapsRollup(smapsText);
  const rssBytes = smaps.rss ?? status.VmRSS ?? null;
  return {
    pid,
    rss_bytes: rssBytes,
    pss_bytes: smaps.pss ?? null,
    anonymous_bytes: smaps.anonymous ?? null,
    swap_bytes: smaps.swap ?? status.VmSwap ?? null,
    status,
    smaps,
  };
};

const safeEnvDump = (pid) => {
  const raw = readText(`/proc/${pid}/environ`);
  return raw
    .split('\0')
    .filter(Boolean)
    .map((entry) => {
      const eq = entry.indexOf('=');
      const key = eq === -1 ? entry : entry.slice(0, eq);
      if (redactKey.test(key)) return `${key}=<redacted>`;
      return entry;
    })
    .join('\n');
};

const commandLine = (pid) => readText(`/proc/${pid}/cmdline`).split('\0').filter(Boolean).join(' ');

const shouldCapture = (state, pid, rssBytes, thresholds, now, cooldownMs, force = false) => {
  if (!Number.isSafeInteger(rssBytes) || rssBytes <= 0) {
    return { capture: false, reason: 'rss_unavailable', thresholds: [] };
  }
  const active = state.pid === pid ? state : { pid, fired: {}, last_capture_at: 0 };
  const last = Number(active.last_capture_at) || 0;
  const crossed = thresholds.filter((threshold) => rssBytes >= threshold);
  const fresh = crossed.filter((threshold) => !active.fired?.[String(threshold)]);
  if (!force && fresh.length === 0) {
    return { capture: false, reason: crossed.length ? 'threshold_already_captured' : 'below_threshold', thresholds: crossed };
  }
  if (!force && now - last < cooldownMs) {
    return { capture: false, reason: 'cooldown', thresholds: fresh };
  }
  return {
    capture: true,
    reason: force ? 'force' : 'threshold',
    thresholds: force && fresh.length === 0 ? crossed : fresh,
  };
};

const applyCaptureState = (state, pid, thresholds, now) => {
  const next = state.pid === pid ? { ...state, fired: { ...(state.fired ?? {}) } } : { pid, fired: {} };
  for (const threshold of thresholds) next.fired[String(threshold)] = now;
  next.last_capture_at = now;
  return next;
};

const bridgeRequest = ({ bridgeUrl, token, message, timeoutMs }) => new Promise((resolve, reject) => {
  if (typeof WebSocket !== 'function') {
    reject(new Error('global WebSocket is unavailable in this Node runtime'));
    return;
  }
  const ws = new WebSocket(bridgeUrl);
  const timer = setTimeout(() => {
    try {
      ws.close();
    } catch (_error) {}
    reject(new Error('SC-Bridge request timed out'));
  }, timeoutMs);
  const cleanup = () => clearTimeout(timer);
  ws.addEventListener('error', (event) => {
    cleanup();
    reject(new Error(event?.message || 'SC-Bridge websocket error'));
  });
  ws.addEventListener('message', (event) => {
    let payload = null;
    try {
      payload = JSON.parse(event.data);
    } catch (error) {
      cleanup();
      reject(error);
      return;
    }
    if (payload.type === 'hello') {
      ws.send(JSON.stringify({ id: 1, type: 'auth', token }));
      return;
    }
    if (payload.id === 1 && payload.type === 'auth_ok') {
      ws.send(JSON.stringify({ id: 2, ...message }));
      return;
    }
    if (payload.id === 2) {
      cleanup();
      try {
        ws.close();
      } catch (_error) {}
      resolve(payload);
    }
  });
});

const collectBridgeState = async ({ bridgeUrl, token, timeoutMs }) => {
  const requests = [
    ['bridge-info.json', { type: 'info' }],
    ['bridge-stats.json', { type: 'stats' }],
    ['bridge-session-stats.json', { type: 'session_stats' }],
  ];
  const out = {};
  for (const [name, message] of requests) {
    try {
      out[name] = await bridgeRequest({ bridgeUrl, token, message, timeoutMs: Math.min(timeoutMs, 15_000) });
    } catch (error) {
      out[name] = { ok: false, error: error?.message ?? String(error) };
    }
  }
  return out;
};

const heapSnapshotSummary = (file) => {
  const snapshot = JSON.parse(fs.readFileSync(file, 'utf8'));
  const nodeFields = snapshot.snapshot.meta.node_fields;
  const edgeFields = snapshot.snapshot.meta.edge_fields;
  const nodeTypes = snapshot.snapshot.meta.node_types[nodeFields.indexOf('type')];
  const edgeTypes = snapshot.snapshot.meta.edge_types[edgeFields.indexOf('type')];
  const nodeFieldCount = nodeFields.length;
  const edgeFieldCount = edgeFields.length;
  const nodeTypeIndex = nodeFields.indexOf('type');
  const nodeNameIndex = nodeFields.indexOf('name');
  const nodeSizeIndex = nodeFields.indexOf('self_size');
  const nodeEdgeCountIndex = nodeFields.indexOf('edge_count');
  const edgeTypeIndex = edgeFields.indexOf('type');
  const edgeNameIndex = edgeFields.indexOf('name_or_index');
  const edgeToNodeIndex = edgeFields.indexOf('to_node');
  const strings = snapshot.strings;
  const nodes = snapshot.nodes;
  const edges = snapshot.edges;
  const byKind = new Map();
  const incomingArrayBuffer = new Map();
  const nodeCount = nodes.length / nodeFieldCount;
  const nodeMeta = (nodeOffset) => {
    const type = nodeTypes[nodes[nodeOffset + nodeTypeIndex]];
    const rawName = strings[nodes[nodeOffset + nodeNameIndex]] || '';
    const name = type === 'string'
      ? `<string:${rawName.length}>`
      : rawName.slice(0, 160);
    return { type, name, size: nodes[nodeOffset + nodeSizeIndex] || 0 };
  };

  let total = 0;
  let nativeArrayBufferBytes = 0;
  let edgeOffset = 0;
  for (let nodeOffset = 0; nodeOffset < nodes.length; nodeOffset += nodeFieldCount) {
    const meta = nodeMeta(nodeOffset);
    total += meta.size;
    byKind.set(meta.type, (byKind.get(meta.type) || 0) + meta.size);
    const edgeCount = nodes[nodeOffset + nodeEdgeCountIndex] || 0;
    for (let index = 0; index < edgeCount; index += 1) {
      const edgeType = edgeTypes[edges[edgeOffset + edgeTypeIndex]];
      const edgeNameRaw = edges[edgeOffset + edgeNameIndex];
      const targetOffset = edges[edgeOffset + edgeToNodeIndex];
      const target = nodeMeta(targetOffset);
      if (target.type === 'native' && target.name === 'system / JSArrayBufferData') {
        nativeArrayBufferBytes += target.size;
        const edgeName = edgeType === 'element'
          ? '<element>'
          : String(strings[edgeNameRaw] ?? edgeNameRaw).slice(0, 80);
        const key = `${meta.type}:${meta.name} via ${edgeType}:${edgeName}`;
        const current = incomingArrayBuffer.get(key) ?? { count: 0, bytes: 0 };
        current.count += 1;
        current.bytes += target.size;
        incomingArrayBuffer.set(key, current);
      }
      edgeOffset += edgeFieldCount;
    }
  }

  const top = (map, count) => [...map.entries()]
    .sort((left, right) => {
      const lb = typeof left[1] === 'number' ? left[1] : left[1].bytes;
      const rb = typeof right[1] === 'number' ? right[1] : right[1].bytes;
      return rb - lb;
    })
    .slice(0, count)
    .map(([name, value]) => (
      typeof value === 'number' ? { name, bytes: value } : { name, ...value }
    ));

  return {
    file,
    file_bytes: fs.statSync(file).size,
    node_count: nodeCount,
    total_self_size_bytes: total,
    native_arraybuffer_bytes: nativeArrayBufferBytes,
    by_kind: top(byKind, 16),
    incoming_arraybuffer_retainers: top(incomingArrayBuffer, 40),
  };
};

const copyIfExists = (source, target) => {
  if (!fs.existsSync(source)) return false;
  try {
    writePrivateFile(target, readText(source));
    return true;
  } catch (_error) {
    return false;
  }
};

const entrySize = (target) => {
  let stat = null;
  try {
    stat = fs.lstatSync(target);
  } catch (_error) {
    return 0;
  }
  if (stat.isSymbolicLink()) return 0;
  if (!stat.isDirectory()) return stat.size;
  let total = stat.size;
  for (const name of fs.readdirSync(target)) {
    total += entrySize(path.join(target, name));
  }
  return total;
};

const captureDirs = (artifactRoot) => {
  try {
    return fs.readdirSync(artifactRoot, { withFileTypes: true })
      .filter((entry) => entry.isDirectory() && entry.name.startsWith('pear-'))
      .map((entry) => {
        const dir = path.join(artifactRoot, entry.name);
        let mtimeMs = 0;
        try {
          mtimeMs = fs.statSync(dir).mtimeMs;
        } catch (_error) {}
        const manifest = readJsonFile(path.join(dir, 'manifest.json'), null);
        const createdAt = Date.parse(manifest?.started_at ?? '') || mtimeMs || 0;
        const heapSnapshot = manifest?.files?.heap_snapshot
          ?? readJsonFile(path.join(dir, 'heap-snapshot-result.json'), null)?.path
          ?? null;
        const heapSnapshotSize = typeof heapSnapshot === 'string' && path.isAbsolute(heapSnapshot)
          ? entrySize(heapSnapshot)
          : 0;
        return {
          dir,
          createdAt,
          heapSnapshot,
          size: entrySize(dir) + heapSnapshotSize,
        };
      })
      .sort((left, right) => left.createdAt - right.createdAt);
  } catch (_error) {
    return [];
  }
};

const removeCaptureDir = (captureDir, options) => {
  const root = path.resolve(options.artifactRoot);
  const target = path.resolve(captureDir.dir);
  if (!pathContains(root, target) || path.basename(target).startsWith('pear-') === false) {
    throw new Error(`refusing to prune unexpected capture path ${captureDir.dir}`);
  }
  const heapSnapshot = typeof captureDir.heapSnapshot === 'string'
    ? path.resolve(captureDir.heapSnapshot)
    : null;
  fs.rmSync(target, { recursive: true, force: true });
  if (heapSnapshot && path.basename(heapSnapshot).startsWith('mayhem-peer-')) {
    const diagnosticRoot = path.resolve(options.diagnosticRoot);
    if (pathContains(diagnosticRoot, heapSnapshot)) {
      fs.rmSync(heapSnapshot, { force: true });
    }
  }
};

const enforceRetention = (options) => {
  const maxCaptures = options.maxCaptures;
  const maxBytes = options.maxBytes;
  let dirs = captureDirs(options.artifactRoot);
  let totalBytes = dirs.reduce((sum, entry) => sum + entry.size, 0);
  const removed = [];
  while (dirs.length > maxCaptures || totalBytes > maxBytes) {
    const oldest = dirs.shift();
    if (!oldest) break;
    removeCaptureDir(oldest, options);
    removed.push({ dir: oldest.dir, bytes: oldest.size });
    totalBytes -= oldest.size;
  }
  return {
    removed,
    remaining_captures: dirs.length,
    remaining_bytes: Math.max(0, totalBytes),
  };
};

const capture = async (options, sample, trigger) => {
  const started = Date.now();
  const captureDir = path.join(
    options.artifactRoot,
    `pear-${sample.pid}-${compactIso(new Date(started))}-${trigger.reason}`
  );
  fs.mkdirSync(captureDir, { recursive: true, mode: 0o700 });

  const manifest = {
    started_at: new Date(started).toISOString(),
    host: execText('hostname', []),
    service: options.service,
    parent_pid: sample.parent,
    pear_pid: sample.pid,
    rss_bytes: sample.rss_bytes,
    trigger,
    bridge_url: options.bridgeUrl,
    files: {},
  };
  writeJsonFile(path.join(captureDir, 'manifest.json'), manifest);
  writePrivateFile(path.join(captureDir, 'pear-cmdline.txt'), `${commandLine(sample.pid)}\n`);
  writePrivateFile(path.join(captureDir, 'pear-env-filtered.txt'), `${safeEnvDump(sample.pid)}\n`);
  copyIfExists(`/proc/${sample.pid}/status`, path.join(captureDir, 'pear-status.txt'));
  copyIfExists(`/proc/${sample.pid}/smaps_rollup`, path.join(captureDir, 'pear-smaps-rollup.txt'));
  copyIfExists(`/proc/${sample.pid}/limits`, path.join(captureDir, 'pear-limits.txt'));
  writePrivateFile(
    path.join(captureDir, 'listeners.txt'),
    execText('ss', ['-ltnp'], { timeout: 10_000 })
      .split('\n')
      .filter((line) => line.includes(`pid=${sample.pid},`) || line.includes(':49222') || line.includes(':49223'))
      .join('\n')
  );
  const pmap = spawnSync('pmap', ['-x', String(sample.pid)], {
    encoding: 'utf8',
    timeout: 30_000,
  });
  writePrivateFile(path.join(captureDir, 'pear-pmap.txt'), pmap.stdout || pmap.stderr || '');

  const token = readText(options.bridgeTokenFile).trim();
  if (token) {
    const bridgeState = await collectBridgeState({
      bridgeUrl: options.bridgeUrl,
      token,
      timeoutMs: options.heapTimeoutMs,
    });
    for (const [name, value] of Object.entries(bridgeState)) {
      writeJsonFile(path.join(captureDir, name), value);
    }
    if (options.heap !== false) {
      const label = trigger.thresholds?.map(thresholdLabel).join('-') || trigger.reason;
      try {
        const result = await bridgeRequest({
          bridgeUrl: options.bridgeUrl,
          token,
          timeoutMs: options.heapTimeoutMs,
          message: { type: 'take_heap_snapshot', label },
        });
        writeJsonFile(path.join(captureDir, 'heap-snapshot-result.json'), result);
        if (result?.type === 'heap_snapshot' && result.path) {
          manifest.files.heap_snapshot = result.path;
          try {
            const summary = heapSnapshotSummary(result.path);
            writeJsonFile(path.join(captureDir, 'heap-summary.json'), summary);
            manifest.files.heap_summary = path.join(captureDir, 'heap-summary.json');
          } catch (error) {
            writeJsonFile(path.join(captureDir, 'heap-summary-error.json'), {
              error: error?.message ?? String(error),
            });
          }
        }
      } catch (error) {
        writeJsonFile(path.join(captureDir, 'heap-snapshot-error.json'), {
          error: error?.message ?? String(error),
        });
      }
    }
  } else {
    writeJsonFile(path.join(captureDir, 'bridge-token-error.json'), {
      error: `SC-Bridge token file missing or empty: ${options.bridgeTokenFile}`,
    });
  }
  manifest.completed_at = new Date().toISOString();
  manifest.duration_ms = Date.now() - started;
  writeJsonFile(path.join(captureDir, 'manifest.json'), manifest);
  return captureDir;
};

const defaultStateFile = (artifactRoot) => path.join(artifactRoot, 'state.json');

const optionsFromArgs = (args) => {
  const diagnosticRoot = String(args['diagnostic-root'] ?? defaultDiagnosticRoot());
  const artifactRoot = String(args['artifact-root'] ?? path.join(diagnosticRoot, 'threshold-captures'));
  return {
    service: String(args.service ?? DEFAULT_SERVICE),
    diagnosticRoot,
    artifactRoot,
    stateFile: String(args['state-file'] ?? defaultStateFile(artifactRoot)),
    thresholds: parseThresholds(args.thresholds ?? DEFAULT_THRESHOLDS),
    intervalMs: parsePositiveInteger(args['interval-ms'], DEFAULT_INTERVAL_MS, 'interval-ms'),
    cooldownMs: parsePositiveInteger(args['cooldown-ms'], DEFAULT_COOLDOWN_MS, 'cooldown-ms'),
    maxCaptures: parsePositiveInteger(args['max-captures'], DEFAULT_MAX_CAPTURES, 'max-captures'),
    maxBytes: parsePositiveInteger(
      args['max-bytes'] === undefined ? DEFAULT_MAX_BYTES : parseByteSize(args['max-bytes']),
      DEFAULT_MAX_BYTES,
      'max-bytes'
    ),
    bridgeUrl: String(args['bridge-url'] ?? DEFAULT_BRIDGE_URL),
    bridgeTokenFile: String(args['bridge-token-file'] ?? defaultBridgeTokenFile()),
    heapTimeoutMs: parsePositiveInteger(args['heap-timeout-ms'], DEFAULT_HEAP_TIMEOUT_MS, 'heap-timeout-ms'),
    once: args.once === true,
    force: args.force === true,
    heap: args.heap === false ? false : args['no-heap'] !== true,
    json: args.json === true,
  };
};

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const logEvent = (options, event) => {
  if (options.json) console.log(JSON.stringify(event));
  else console.log(`[${new Date().toISOString()}] ${event.type}: ${event.message ?? ''}`);
};

const tick = async (options) => {
  const child = pearChildPid(options.service);
  if (!child?.pid) {
    return { event: { type: 'skip', message: 'pear-runtime child not found', parent_pid: child?.parent ?? null } };
  }
  const memory = samplePearMemory(child.pid);
  const state = readJsonFile(options.stateFile, {});
  const decision = shouldCapture(
    state,
    child.pid,
    memory.rss_bytes,
    options.thresholds,
    Date.now(),
    options.cooldownMs,
    options.force
  );
  if (!decision.capture) {
    return {
      event: {
        type: 'sample',
        message: decision.reason,
        parent_pid: child.parent,
        pear_pid: child.pid,
        rss_bytes: memory.rss_bytes,
        reason: decision.reason,
      },
    };
  }
  const trigger = {
    reason: decision.reason,
    thresholds: decision.thresholds,
    threshold_labels: decision.thresholds.map(thresholdLabel),
  };
  const captureDir = await capture(options, { ...memory, parent: child.parent }, trigger);
  const nextState = applyCaptureState(state, child.pid, decision.thresholds, Date.now());
  writeJsonFile(options.stateFile, nextState);
  const retention = enforceRetention(options);
  return {
    event: {
      type: 'capture',
      message: captureDir,
      parent_pid: child.parent,
      pear_pid: child.pid,
      rss_bytes: memory.rss_bytes,
      trigger,
      capture_dir: captureDir,
      retention,
    },
  };
};

const main = async () => {
  const args = parseArgs(process.argv.slice(2));
  if (args.help) {
    console.log(usage());
    return;
  }
  const options = optionsFromArgs(args);
  fs.mkdirSync(options.artifactRoot, { recursive: true, mode: 0o700 });
  while (true) {
    try {
      const { event } = await tick(options);
      logEvent(options, event);
    } catch (error) {
      logEvent(options, {
        type: 'error',
        message: error?.message ?? String(error),
      });
    }
    if (options.once) break;
    await sleep(options.intervalMs);
  }
};

if (import.meta.url === `file://${process.argv[1]}`) {
  main().catch((error) => {
    console.error(error?.stack ?? error?.message ?? String(error));
    process.exit(1);
  });
}

export {
  applyCaptureState,
  captureDirs,
  enforceRetention,
  entrySize,
  heapSnapshotSummary,
  optionsFromArgs,
  parseByteSize,
  parseThresholds,
  shouldCapture,
  thresholdLabel,
};

#!/usr/bin/env node

import { spawn } from 'node:child_process';
import { createServer } from 'node:net';
import { request as httpRequest } from 'node:http';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const BINARY_NAME = process.platform === 'win32'
  ? 'mayhem-dashboard-workbench.exe'
  : 'mayhem-dashboard-workbench';
const BINARY_PATH = path.join(ROOT, 'target', 'debug', BINARY_NAME);
const DEFAULT_STARTUP_TIMEOUT_MS = 20_000;
const SCALE_MODEL_COUNT = 96;
const SCALE_ROUTE_COUNT = 128;
const SCALE_RECEIPT_COUNT = 96;
const SCALE_TOKEN_COUNT = 64;
const SCALE_PROBE_COUNT = 64;
const MODEL_ROW_CAP = 25;
const PROVIDER_ROW_CAP = 25;
const ACTIVITY_ROW_CAP = 25;
const EVIDENCE_ROW_CAP = 25;
const TOKEN_ROW_CAP = 25;
const lastPage = (total, cap) => Math.ceil(total / cap);
const lastPageStart = (total, cap) => (lastPage(total, cap) - 1) * cap + 1;
const lastPageRows = (total, cap) => total - (lastPage(total, cap) - 1) * cap;

const EXPECTED_SCENARIOS = [
  'showcase',
  'auth-required',
  'empty',
  'loading',
  'failure',
  'offline',
  'source-update',
  'signed-update',
  'update-required',
  'scale',
];

const SCENARIO_TITLES = new Map([
  ['showcase', 'Showcase'],
  ['auth-required', 'Credential required'],
  ['empty', 'Empty state'],
  ['loading', 'Provider loading'],
  ['failure', 'Provider failure'],
  ['offline', 'Routes offline'],
  ['source-update', 'Source update'],
  ['signed-update', 'Signed update'],
  ['update-required', 'Update required'],
  ['scale', 'Scale and overflow'],
]);

const PRODUCT_ROUTES = [
  {
    id: 'home',
    path: '/mayhem/dashboard',
    title: 'Mayhem Home',
    pageLabel: 'Home',
    heading: 'Overview',
    primaryPath: '/mayhem/dashboard',
    markers: ['aria-label="Current gateway summary"', 'aria-label="Next actions"'],
  },
  {
    id: 'playground',
    path: '/mayhem/dashboard/playground',
    title: 'Mayhem Playground',
    pageLabel: 'Playground',
    heading: 'Playground',
    markers: ['href="/mayhem/dashboard/activity"'],
  },
  {
    id: 'models',
    path: '/mayhem/dashboard/models',
    title: 'Mayhem Models',
    pageLabel: 'Model catalog',
    heading: 'Model catalog',
    primaryPath: '/mayhem/dashboard/models',
    markers: ['id="models-table"', '<caption class="sr-only">Models in this gateway catalog</caption>'],
  },
  {
    id: 'activity',
    path: '/mayhem/dashboard/activity',
    title: 'Mayhem Activity',
    pageLabel: 'Activity',
    heading: 'Requests and receipts',
    primaryPath: '/mayhem/dashboard/activity',
    markers: ['id="activity-table"', '<caption class="sr-only">Prioritized incomplete records, final receipts, and retained pause records from this gateway process</caption>'],
  },
  {
    id: 'wallet',
    path: '/mayhem/dashboard/wallet',
    title: 'Mayhem Wallet',
    pageLabel: 'Billing',
    heading: 'Billing',
    primaryPath: '/mayhem/dashboard/wallet',
    markers: ['id="wallet-backup-command"'],
  },
  {
    id: 'connect',
    path: '/mayhem/dashboard/connect',
    title: 'Mayhem Connect',
    pageLabel: 'Integrations',
    heading: 'Connect another AI app',
    primaryPath: '/mayhem/dashboard/connect',
    markers: ['id="gateway-base-url"', 'id="connection-result"', 'data-connection-test'],
  },
  {
    id: 'earn',
    path: '/mayhem/dashboard/earn',
    title: 'Mayhem Earn',
    pageLabel: 'Earn',
    primaryPath: '/mayhem/dashboard/earn',
    context: 'Earn',
    markers: ['data-money', 'id="earn-routes-table"'],
  },
  {
    id: 'earn-jobs',
    path: '/mayhem/dashboard/earn/jobs',
    title: 'Mayhem Jobs',
    pageLabel: 'Earn',
    heading: 'Jobs',
    primaryPath: '/mayhem/dashboard/earn',
    context: 'Earn',
    markers: ['id="provider-jobs-table"', 'Gateway-observed provider jobs'],
  },
  {
    id: 'earn-machines',
    path: '/mayhem/dashboard/earn/machines',
    title: 'Mayhem Machines',
    pageLabel: 'Earn',
    heading: 'Machines and serving routes',
    primaryPath: '/mayhem/dashboard/earn',
    context: 'Earn',
    markers: ['id="machine-routes-table"'],
  },
  {
    id: 'earn-opportunities',
    path: '/mayhem/dashboard/earn/opportunities',
    title: 'Mayhem Model opportunities',
    pageLabel: 'Earn',
    heading: 'Model opportunities',
    primaryPath: '/mayhem/dashboard/earn',
    context: 'Earn',
    markers: ['id="model-fit-table"'],
  },
  {
    id: 'earn-earnings',
    path: '/mayhem/dashboard/earn/earnings',
    title: 'Mayhem Earnings and payouts',
    pageLabel: 'Earn',
    heading: 'Earnings and payouts',
    primaryPath: '/mayhem/dashboard/earn',
    context: 'Earn',
    markers: ['<table class="data-table">'],
  },
  {
    id: 'earn-reliability',
    path: '/mayhem/dashboard/earn/reliability',
    title: 'Mayhem Reliability',
    pageLabel: 'Earn',
    heading: 'Reliability',
    primaryPath: '/mayhem/dashboard/earn',
    context: 'Earn',
    markers: ['id="reliability-routes-table"'],
  },
  {
    id: 'network',
    path: '/mayhem/dashboard/network',
    title: 'Mayhem Network',
    pageLabel: 'Network',
    heading: 'Network health',
    primaryPath: '/mayhem/dashboard/network',
    context: 'Network',
    markers: ['<table class="data-table">'],
  },
  {
    id: 'network-models',
    path: '/mayhem/dashboard/network/models',
    title: 'Mayhem Network models',
    pageLabel: 'Network',
    heading: 'Models',
    primaryPath: '/mayhem/dashboard/network',
    context: 'Network',
    markers: ['id="network-models-table"'],
  },
  {
    id: 'network-providers',
    path: '/mayhem/dashboard/network/providers',
    title: 'Mayhem Network providers',
    pageLabel: 'Network',
    heading: 'Providers',
    primaryPath: '/mayhem/dashboard/network',
    context: 'Network',
    markers: ['id="provider-table"'],
  },
  {
    id: 'network-markets',
    path: '/mayhem/dashboard/network/markets',
    title: 'Mayhem Network markets',
    pageLabel: 'Network',
    heading: 'Markets',
    primaryPath: '/mayhem/dashboard/network',
    context: 'Network',
    markers: ['id="market-table"'],
  },
  {
    id: 'network-activity',
    path: '/mayhem/dashboard/network/activity',
    title: 'Mayhem Network activity',
    pageLabel: 'Network',
    heading: 'Route status',
    primaryPath: '/mayhem/dashboard/network',
    context: 'Network',
    markers: ['id="network-activity-table"'],
  },
  {
    id: 'network-evidence',
    path: '/mayhem/dashboard/network/evidence',
    title: 'Mayhem Network evidence',
    pageLabel: 'Network',
    heading: 'Evidence',
    primaryPath: '/mayhem/dashboard/network',
    context: 'Network',
    markers: ['id="network-evidence-table"'],
  },
  {
    id: 'help',
    path: '/mayhem/dashboard/help',
    title: 'Mayhem Help',
    pageLabel: 'Help',
    heading: 'Help',
    primaryPath: '/mayhem/dashboard/help',
    markers: ['Get started', 'Common problems', 'What dashboard data means', 'Advanced verification'],
  },
  {
    id: 'settings',
    path: '/mayhem/dashboard/settings',
    title: 'Mayhem Settings',
    pageLabel: 'Settings',
    heading: 'Settings',
    primaryPath: '/mayhem/dashboard/settings',
    markers: [
      'data-preference="amounts"',
      'data-preference="motion"',
      'data-preference="density"',
      'data-clear-preferences',
    ],
  },
];

const SCENARIO_ROUTE_IDS = {
  showcase: new Set(PRODUCT_ROUTES.map((route) => route.id)),
  'auth-required': new Set(PRODUCT_ROUTES.map((route) => route.id)),
  empty: new Set(PRODUCT_ROUTES.map((route) => route.id)),
  loading: new Set([
    'home', 'playground', 'models', 'earn', 'earn-machines',
    'earn-opportunities', 'network', 'network-models', 'network-providers',
  ]),
  failure: new Set([
    'home', 'earn', 'earn-machines', 'earn-reliability',
    'network', 'network-evidence',
  ]),
  offline: new Set([
    'home', 'playground', 'models', 'earn', 'earn-machines',
    'earn-opportunities', 'earn-reliability', 'network', 'network-models',
    'network-providers', 'network-markets', 'network-activity', 'network-evidence',
  ]),
  'source-update': new Set(PRODUCT_ROUTES.map((route) => route.id)),
  'signed-update': new Set(PRODUCT_ROUTES.map((route) => route.id)),
  'update-required': new Set(PRODUCT_ROUTES.map((route) => route.id)),
  scale: new Set([
    'home', 'models', 'activity', 'connect', 'earn', 'earn-machines',
    'earn-opportunities', 'earn-reliability', 'network',
    'network-models', 'network-providers', 'network-markets',
    'network-activity', 'network-evidence',
  ]),
};

const DENSITY_ROUTE_IDS = new Set([
  'models',
  'activity',
  'earn',
  'earn-machines',
  'earn-opportunities',
  'earn-reliability',
  'network-models',
  'network-providers',
  'network-markets',
  'network-activity',
  'network-evidence',
]);

const PRIMARY_NAV_PATHS = [
  '/mayhem/dashboard',
  '/mayhem/dashboard/playground',
  '/mayhem/dashboard/models',
  '/mayhem/dashboard/activity',
  '/mayhem/dashboard/wallet',
  '/mayhem/dashboard/connect',
  '/mayhem/dashboard/earn',
  '/mayhem/dashboard/network',
  '/mayhem/dashboard/settings',
];

const EMPTY_FILTER_ROUTE_IDS = new Set([
  'models',
  'activity',
  'connect',
  'earn',
  'earn-machines',
  'earn-reliability',
  'network-models',
  'network-providers',
  'network-markets',
  'network-evidence',
]);

const SHOWCASE_FILTER_ROUTE_IDS = new Set([
  'models',
  'activity',
  'connect',
  'earn',
  'earn-machines',
  'earn-reliability',
  'network-models',
  'network-providers',
  'network-evidence',
]);

const EVIDENCE_ROUTE_IDS = new Set([
  'models',
  'activity',
  'earn-machines',
  'earn-earnings',
  'earn-reliability',
  'network-models',
  'network-providers',
  'network-evidence',
]);

const MOBILE_MORE_ROUTE_IDS = new Set([]);

function usage() {
  console.log(`Usage:
  node scripts/dashboard-workbench-smoke.mjs [options]

Options:
  --base-url URL       Test an already running loopback workbench instead of
                       building and launching an isolated one.
  --no-build           Launch the existing workbench binary without rebuilding.
  --timeout-ms N       Startup/request timeout in milliseconds (default: 20000).
  --verbose            Print every successful assertion.
  --help, -h           Show this help.

The default mode builds the feature-gated workbench, starts it on an available
loopback port, exercises every product route across the scenarios that matter
to it, and then stops the isolated process. No browser or third-party package
is required.`);
}

function parseArgs(argv) {
  const options = {
    baseUrl: process.env.MAYHEM_DASHBOARD_WORKBENCH_SMOKE_BASE_URL ?? null,
    build: true,
    timeoutMs: DEFAULT_STARTUP_TIMEOUT_MS,
    verbose: false,
  };

  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    switch (arg) {
      case '--base-url':
        options.baseUrl = argv[index + 1] ?? null;
        if (options.baseUrl === null) throw new Error('--base-url requires a URL');
        index += 1;
        break;
      case '--no-build':
        options.build = false;
        break;
      case '--timeout-ms': {
        const value = Number(argv[index + 1]);
        if (!Number.isSafeInteger(value) || value < 250) {
          throw new Error('--timeout-ms requires an integer of at least 250');
        }
        options.timeoutMs = value;
        index += 1;
        break;
      }
      case '--verbose':
        options.verbose = true;
        break;
      case '--help':
      case '-h':
        usage();
        process.exit(0);
        break;
      default:
        throw new Error(`unknown argument: ${arg}`);
    }
  }
  return options;
}

function normalizeBaseUrl(value) {
  const url = new URL(value);
  const loopbackHosts = new Set(['127.0.0.1', 'localhost', '[::1]']);
  if (url.protocol !== 'http:' || !loopbackHosts.has(url.hostname)) {
    throw new Error(`--base-url must be an HTTP loopback URL, received ${value}`);
  }
  if (url.username || url.password || url.search || url.hash) {
    throw new Error('--base-url cannot contain credentials, a query, or a fragment');
  }
  return url.href.replace(/\/$/, '');
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

async function availableLoopbackPort() {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.unref();
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      if (address === null || typeof address === 'string') {
        server.close(() => reject(new Error('could not allocate a loopback port')));
        return;
      }
      server.close((error) => error ? reject(error) : resolve(address.port));
    });
  });
}

async function runCargoBuild() {
  console.log('[smoke] building mayhem-dashboard-workbench');
  await new Promise((resolve, reject) => {
    const child = spawn('cargo', [
      'build',
      '-p', 'mayhem-gateway',
      '--features', 'dashboard-workbench',
      '--bin', 'mayhem-dashboard-workbench',
    ], {
      cwd: ROOT,
      env: process.env,
      stdio: 'inherit',
      windowsHide: true,
    });
    child.once('error', reject);
    child.once('exit', (code, signal) => {
      if (code === 0) resolve();
      else reject(new Error(`cargo build failed (${signal ?? `exit ${code}`})`));
    });
  });
}

function launchWorkbench(port) {
  const logs = [];
  const child = spawn(BINARY_PATH, ['--bind', `127.0.0.1:${port}`], {
    cwd: ROOT,
    env: process.env,
    stdio: ['ignore', 'pipe', 'pipe'],
    windowsHide: true,
  });
  const remember = (chunk) => {
    logs.push(chunk.toString());
    if (logs.length > 80) logs.shift();
  };
  child.stdout.on('data', remember);
  child.stderr.on('data', remember);
  return { child, logs };
}

async function stopWorkbench(child) {
  if (child === null || child.exitCode !== null || child.signalCode !== null) return;
  const exited = new Promise((resolve) => child.once('exit', resolve));
  child.kill('SIGTERM');
  await Promise.race([exited, delay(2_000)]);
  if (child.exitCode === null && child.signalCode === null) {
    child.kill('SIGKILL');
    await Promise.race([exited, delay(1_000)]);
  }
}

function get(baseUrl, pathname, { headers = {}, timeoutMs } = {}) {
  const url = new URL(pathname, `${baseUrl}/`);
  return new Promise((resolve, reject) => {
    const req = httpRequest(url, {
      method: 'GET',
      headers: { Accept: '*/*', ...headers },
    }, (response) => {
      const chunks = [];
      response.on('data', (chunk) => chunks.push(chunk));
      response.once('error', reject);
      response.once('end', () => {
        const body = Buffer.concat(chunks);
        resolve({
          url: url.href,
          status: response.statusCode ?? 0,
          headers: response.headers,
          body,
          text: body.toString('utf8'),
        });
      });
    });
    req.setTimeout(timeoutMs ?? DEFAULT_STARTUP_TIMEOUT_MS, () => {
      req.destroy(new Error(`request timed out after ${timeoutMs ?? DEFAULT_STARTUP_TIMEOUT_MS}ms`));
    });
    req.once('error', reject);
    req.end();
  });
}

function postJson(baseUrl, pathname, payload, { headers = {}, timeoutMs } = {}) {
  const url = new URL(pathname, `${baseUrl}/`);
  const requestBody = Buffer.from(JSON.stringify(payload));
  return new Promise((resolve, reject) => {
    const req = httpRequest(url, {
      method: 'POST',
      headers: {
        Accept: 'text/event-stream, application/json',
        'Content-Type': 'application/json',
        'Content-Length': requestBody.length,
        ...headers,
      },
    }, (response) => {
      const chunks = [];
      response.on('data', (chunk) => chunks.push(chunk));
      response.once('error', reject);
      response.once('end', () => {
        const body = Buffer.concat(chunks);
        resolve({
          url: url.href,
          status: response.statusCode ?? 0,
          headers: response.headers,
          body,
          text: body.toString('utf8'),
        });
      });
    });
    req.setTimeout(timeoutMs ?? DEFAULT_STARTUP_TIMEOUT_MS, () => {
      req.destroy(new Error(`request timed out after ${timeoutMs ?? DEFAULT_STARTUP_TIMEOUT_MS}ms`));
    });
    req.once('error', reject);
    req.end(requestBody);
  });
}

async function waitForWorkbench(baseUrl, child, logs, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  let lastError = null;
  while (Date.now() < deadline) {
    if (child !== null && (child.exitCode !== null || child.signalCode !== null)) {
      const output = logs.join('').trim();
      throw new Error(`workbench exited before it became ready\n${output}`);
    }
    try {
      const response = await get(baseUrl, '/__workbench/health', { timeoutMs: 1_000 });
      if (response.status === 200) return;
      lastError = new Error(`health endpoint returned ${response.status}`);
    } catch (error) {
      lastError = error;
    }
    await delay(100);
  }
  const output = logs.join('').trim();
  throw new Error(
    `workbench was not ready within ${timeoutMs}ms: ${lastError?.message ?? 'unknown error'}`
      + (output ? `\n${output}` : ''),
  );
}

class CheckReport {
  constructor(verbose) {
    this.verbose = verbose;
    this.count = 0;
    this.failures = [];
  }

  check(condition, scope, assertion, detail = '') {
    this.count += 1;
    if (condition) {
      if (this.verbose) console.log(`  [ok] ${scope}: ${assertion}`);
      return;
    }
    this.failures.push({ scope, assertion, detail });
  }

  equal(actual, expected, scope, assertion) {
    this.check(
      actual === expected,
      scope,
      assertion,
      `expected ${JSON.stringify(expected)}, received ${JSON.stringify(actual)}`,
    );
  }

  includes(haystack, needle, scope, assertion = `contains ${JSON.stringify(needle)}`) {
    this.check(
      haystack.includes(needle),
      scope,
      assertion,
      `missing ${JSON.stringify(needle)} (body length ${haystack.length})`,
    );
  }

  finish(routeCount) {
    if (this.failures.length === 0) {
      console.log(
        `[smoke] PASS: ${this.count} assertions across ${routeCount} product route/scenario combinations`,
      );
      return true;
    }

    console.error(
      `[smoke] FAIL: ${this.failures.length} of ${this.count} assertions failed across ${routeCount} product route/scenario combinations`,
    );
    for (const [index, failure] of this.failures.entries()) {
      console.error(`\n${index + 1}) ${failure.scope}\n   ${failure.assertion}`);
      if (failure.detail) console.error(`   ${failure.detail}`);
    }
    return false;
  }
}

function headerValue(headers, name) {
  const value = headers[name.toLowerCase()];
  return Array.isArray(value) ? value.join(', ') : (value ?? '');
}

function setCookies(headers) {
  const value = headers['set-cookie'];
  if (Array.isArray(value)) return value;
  return value === undefined ? [] : [value];
}

function selectedPlaygroundModel(html) {
  const select = html.match(/<select\b[^>]*data-playground-model[^>]*>([\s\S]*?)<\/select>/i)?.[1];
  if (select === undefined) return null;
  const encoded = select.match(/<option\b[^>]*value="([^"]+)"[^>]*\sselected(?:\s|>)/i)?.[1];
  if (encoded === undefined) return null;
  return encoded
    .replaceAll('&amp;', '&')
    .replaceAll('&quot;', '"')
    .replaceAll('&#39;', "'")
    .replaceAll('&lt;', '<')
    .replaceAll('&gt;', '>');
}

function firstSsePayload(text) {
  return ssePayloads(text)[0] ?? null;
}

function ssePayloads(text) {
  const payloads = [];
  for (const line of text.split(/\r?\n/)) {
    if (!line.startsWith('data: ')) continue;
    const data = line.slice('data: '.length);
    if (data === '[DONE]') continue;
    try {
      payloads.push(JSON.parse(data));
    } catch {
      return [];
    }
  }
  return payloads;
}

function embeddedArrowFunction(source, name) {
  const marker = `const ${name} =`;
  const start = source.indexOf(marker);
  const endMarker = '\n    };';
  const end = source.indexOf(endMarker, start);
  if (start < 0 || end < 0) throw new Error(`cannot locate embedded function ${name}`);
  return new Function(`"use strict";${source.slice(start, end + endMarker.length)};return ${name};`)();
}

function countOccurrences(text, needle) {
  if (needle.length === 0) return 0;
  let count = 0;
  let from = 0;
  while (true) {
    const index = text.indexOf(needle, from);
    if (index === -1) return count;
    count += 1;
    from = index + needle.length;
  }
}

function tableBodyRowCount(html, captionText) {
  const caption = `<caption class="sr-only">${captionText}</caption>`;
  const captionIndex = html.indexOf(caption);
  if (captionIndex === -1) return -1;
  const bodyStart = html.indexOf('<tbody>', captionIndex + caption.length);
  if (bodyStart === -1) return -1;
  const bodyEnd = html.indexOf('</tbody>', bodyStart + '<tbody>'.length);
  if (bodyEnd === -1) return -1;
  return countOccurrences(html.slice(bodyStart, bodyEnd), '<tr');
}

function firstEvidenceHref(html) {
  const match = html.match(/href="([^"\s]*\/mayhem\/dashboard\/evidence\?[^"\s]+)" data-evidence-url/);
  return match?.[1].replaceAll('&amp;', '&') ?? null;
}

function containsOnlyLoopbackHttpUrls(html) {
  let from = 0;
  while (true) {
    const index = html.indexOf('http://', from);
    if (index === -1) return true;
    if (!html.startsWith('http://127.0.0.1', index)
        && !html.startsWith('http://localhost', index)
        && !html.startsWith('http://[::1]', index)) return false;
    from = index + 'http://'.length;
  }
}

function containsOnlySafeHttpsLinks(html) {
  const withoutSafeGithubLinks = html.replaceAll(
    /href="https:\/\/github\.com\/[^"\s]+" target="_blank" rel="noopener noreferrer"/g,
    '',
  );
  return !withoutSafeGithubLinks.includes('https://');
}

function checkHtmlResponse(report, response, scope) {
  report.equal(response.status, 200, scope, 'returns HTTP 200');
  report.check(
    headerValue(response.headers, 'content-type').startsWith('text/html'),
    scope,
    'serves HTML content type',
    `received ${JSON.stringify(headerValue(response.headers, 'content-type'))}`,
  );
  report.equal(headerValue(response.headers, 'cache-control'), 'no-store', scope, 'disables caching');
  const csp = headerValue(response.headers, 'content-security-policy');
  report.includes(csp, "default-src 'self'", scope, 'CSP defaults to self');
  report.includes(csp, "frame-ancestors 'none'", scope, 'CSP forbids framing');
  report.equal(headerValue(response.headers, 'x-frame-options'), 'DENY', scope, 'sets X-Frame-Options');
  report.equal(headerValue(response.headers, 'referrer-policy'), 'no-referrer', scope, 'sets no-referrer policy');
  report.check(
    /^<!doctype html>/i.test(response.text),
    scope,
    'starts with an HTML5 doctype',
    `first bytes: ${JSON.stringify(response.text.slice(0, 40))}`,
  );
  report.includes(response.text, '<html lang="en">', scope, 'declares the document language');
  report.includes(response.text, 'name="viewport"', scope, 'declares a responsive viewport');
  report.includes(response.text, '</html>', scope, 'closes the HTML document');
  report.check(!response.text.includes('\uFFFD'), scope, 'contains no UTF-8 replacement characters');
  report.check(
    containsOnlySafeHttpsLinks(response.text),
    scope,
    'loads no external HTTPS resources and allows only hardened GitHub links',
  );
  report.check(
    containsOnlyLoopbackHttpUrls(response.text),
    scope,
    'contains only loopback HTTP URLs',
  );
}

function checkScenarioSemantics(report, scenario, route, html) {
  const scope = `${scenario}/${route.id}`;

  if (scenario === 'showcase') {
    if (route.id === 'home') {
      report.includes(html, 'href="/mayhem/dashboard/playground"', scope, 'offers the primary Playground action');
      report.includes(html, 'data-money', scope, 'marks monetary values for the privacy preference');
    }
    if (route.id === 'playground') {
      report.includes(html, 'data-playground-form', scope, 'renders the interactive prompt form');
      report.includes(html, 'aria-label="Playground mode"', scope, 'renders the Text, Image, and Speech mode switcher');
      report.includes(html, 'data-playground-mode-panel="image"', scope, 'renders the image workspace');
      report.includes(html, 'data-playground-mode-panel="speech"', scope, 'renders the speech workspace');
      report.includes(html, 'data-playground-max-tokens', scope, 'offers an enforceable output limit');
      report.includes(html, 'data-playground-max-price', scope, 'offers the selected model price ceiling');
      report.includes(html, 'data-price-mode="rate"', scope, 'exposes the fixture model price basis');
      report.includes(html, 'data-price-mode="fixed"', scope, 'includes a fixed-only model price basis fixture');
      report.includes(html, 'data-money-input', scope, 'includes the entered ceiling in amount hiding');
      report.includes(html, 'data-playground-min-att-tier', scope, 'offers the minimum attestation tier');
      report.includes(
        html,
        'drafts and history stay in this browser tab',
        scope,
        'explains the scope of draft persistence',
      );
      report.includes(html, 'access tokens are never saved', scope, 'excludes the access token from draft persistence');
      report.includes(html, 'data-playground-reset-draft', scope, 'offers an explicit low-noise draft reset');
      report.includes(
        html,
        'Numeric identity tier does not promise confidential compute.',
        scope,
        'does not overstate attestation as a privacy guarantee',
      );
      report.includes(
        html,
        '<div class="playground-interactive js-only">',
        scope,
        'keeps the enhanced Playground inert when JavaScript is unavailable',
      );
      report.check(
        html.indexOf('<noscript>') < html.indexOf('data-playground-form'),
        scope,
        'places the no-JavaScript explanation before unusable prompt fields',
      );
    }
    if (route.id === 'models') {
      report.includes(html, 'data-model-detail-open', scope, 'opens model details from the model identity');
      report.includes(html, 'aria-label="Use ', scope, 'gives repeated Use links model-specific names');
    }
    if (route.id === 'earn-reliability') {
      report.includes(html, '7 / 25 successful sessions', scope, 'shows protocol-reported probation progress');
      report.includes(
        html,
        'aria-label="Probation successful-session requirement: 7 of 25"',
        scope,
        'labels the determinate probation requirement',
      );
      report.includes(html, 'Provider identity:', scope, 'keeps configured identity as compact context');
      report.check(
        !html.includes('Configured gateway identity'),
        scope,
        'does not elevate a stable provider identity into persistent success attention',
      );
    }
    if (route.id === 'wallet') {
      report.includes(html, 'class="money-value', scope, 'renders the available ledger balance as a monetary value');
      report.includes(html, 'mayhem deposit status --rail fiat', scope, 'checks the configured receipt rail');
    }
    if (SHOWCASE_FILTER_ROUTE_IDS.has(route.id)) {
      report.includes(html, 'data-filter-row', scope, 'renders filterable fixture records');
    }
    if (EVIDENCE_ROUTE_IDS.has(route.id)) {
      report.includes(html, 'data-evidence-url', scope, 'offers evidence on demand for fixture records');
    }
  }

  if (scenario === 'auth-required') {
    if (route.id === 'home') {
      report.includes(html, '<h1>Overview</h1>', scope, 'keeps a stable page title when a credential is missing');
      report.includes(html, 'Credential needed', scope, 'surfaces the missing credential as the current status');
      report.includes(
        html,
        'href="/mayhem/dashboard/connect" data-product-event="use_ai_path_opened">Set up access',
        scope,
        'makes credential setup the Use AI launch action',
      );
      report.includes(
        html,
        '<h2>Your provider</h2>',
        scope,
        'keeps provider evidence visible while credential setup remains the page-level action',
      );
    }
    if (route.id === 'playground') {
      report.includes(html, 'Create an access token first', scope, 'blocks an inference flow that cannot succeed');
      report.includes(html, 'href="/mayhem/dashboard/connect"', scope, 'routes the user to credential setup');
      report.check(
        !html.includes('data-playground-form'),
        scope,
        'does not render an unusable prompt composer',
      );
    }
    if (route.id === 'connect') {
      report.includes(html, 'Credential needed', scope, 'reports the connection blocker');
      report.includes(html, 'No gateway tokens', scope, 'explains how to create the first token');
      report.includes(html, 'class="primary-button" href="#access-tokens">Set up credential', scope, 'keeps the primary action on credential setup');
      report.includes(html, 'Available after an API key is configured.', scope, 'keeps inference pending behind the credential step');
    }
  }

  if (scenario === 'empty') {
    if (route.id === 'home') {
      report.includes(html, '<h1>Overview</h1>', scope, 'keeps a stable page title for the empty catalog');
      report.includes(html, 'No models yet', scope, 'identifies the empty catalog in the page status');
    }
    if (route.id === 'playground') {
      report.includes(html, 'class="empty-block"', scope, 'renders a purposeful empty state');
      report.check(
        !html.includes('data-playground-form'),
        scope,
        'does not offer a prompt form without a model route',
      );
    }
    if (route.id === 'models') {
      report.includes(html, 'No catalog models', scope, 'names the empty catalog state');
    }
    if (route.id === 'wallet') {
      report.includes(
        html,
        'Ledger balance</span><span class="metric-state">FIAT</span></div><div class="metric-status"><span class="status-badge warn">Unavailable</span></div>',
        scope,
        'marks the unavailable ledger balance explicitly without fabricating a balance',
      );
      report.includes(html, 'data-hide-amounts', scope, 'can still mask the funding example amount');
    }
    if (EMPTY_FILTER_ROUTE_IDS.has(route.id)) {
      report.includes(html, 'class="empty-block"', scope, 'renders a purposeful empty state');
      report.check(
        !html.includes('data-filter-row'),
        scope,
        'does not fabricate filterable records',
      );
      report.check(
        !html.includes('data-table-filter'),
        scope,
        'omits a filter when there are no displayed rows',
      );
    }
  }

  if (scenario === 'loading') {
    if (route.id === 'home') {
      report.includes(html, '<h1>Overview</h1>', scope, 'keeps a stable page title while routes load');
      report.includes(html, 'No advertised capacity', scope, 'keeps route unavailability explicit while loading');
      report.includes(html, 'No provider advertises accepting capacity', scope, 'does not treat preparation progress as available capacity');
    }
    if (route.id === 'earn') {
      report.includes(html, '<h1>Provider overview</h1>', scope, 'keeps a stable provider page title during preparation');
      report.includes(html, 'Preparing a model', scope, 'names the active preparation state');
      report.includes(html, 'Download is 68% complete', scope, 'exposes deterministic preparation progress');
    }
  }

  if (scenario === 'failure' && route.id === 'earn') {
    report.includes(html, '<h1>Provider overview</h1>', scope, 'keeps a stable provider page title after a setup failure');
    report.includes(html, 'Setup blocked by model failure', scope, 'names the failed preparation state');
    report.includes(html, ': verify catalog artifact', scope, 'qualifies the failure with its model');
    report.includes(html, 'artifact signature mismatch', scope, 'surfaces the actionable failure reason');
  }
  if (scenario === 'failure' && route.id === 'earn-machines') {
    report.includes(html, 'Recover model preparation', scope, 'provides a finite recovery path');
    report.includes(html, 'rerun the same mayhem provider start command', scope, 'names the honest host-side action');
    report.includes(html, 'Refresh snapshot', scope, 'offers a read-only snapshot refresh');
    report.includes(html, 'does not change provider state', scope, 'does not imply browser-side recovery mutation');
  }

  if (scenario === 'offline') {
    if (route.id === 'home') {
      report.includes(html, '<h1>Overview</h1>', scope, 'keeps a stable page title while routes are offline');
      report.includes(html, 'No advertised capacity', scope, 'surfaces that routes are unavailable');
      report.includes(html, 'No provider advertises accepting capacity', scope, 'does not treat offline routes as accepting capacity');
    }
    if (route.id === 'network') {
      report.includes(html, 'Supply exceptions', scope, 'summarizes offline supply exceptions');
    }
    if (route.id === 'network-providers') {
      report.includes(html, 'Waiting for heartbeat', scope, 'explains why provider routes are unavailable');
      report.check(
        !html.includes('Routes accepting'),
        scope,
        'does not claim accepting routes while offline',
      );
    }
    if (route.id === 'earn-machines') {
      report.includes(html, 'Restore the provider route', scope, 'provides a finite offline recovery path');
      report.includes(html, 'cannot publish a route or start a worker', scope, 'keeps host-side authority explicit');
      report.includes(html, 'Refresh snapshot', scope, 'offers a read-only snapshot refresh');
    }
  }

  if (scenario === 'update-required') {
    if (route.id !== 'settings') {
      report.includes(
        html,
        '<section class="attention-card danger" role="status">',
        scope,
        'renders a prominent update status',
      );
    }
    report.includes(html, 'Update', scope, 'names the update condition');
    if (route.id !== 'settings' && route.id !== 'home') {
      report.includes(
        html,
        '<a class="soft-button" href="/mayhem/dashboard/settings">',
        scope,
        'links the update action to Settings',
      );
    } else if (route.id === 'home') {
      report.includes(
        html,
        '<a class="soft-button" href="/mayhem/dashboard/settings">Review update</a>',
        scope,
        'keeps one update action with the blocking attention state',
      );
      report.equal(
        countOccurrences(html, '<a class="soft-button" href="/mayhem/dashboard/settings">Review update</a>'),
        1,
        scope,
        'does not duplicate the Home update action',
      );
    } else {
      report.includes(html, 'id="mayhem-update-stage-command"', scope, 'shows the verified staging step directly');
      report.includes(html, 'id="mayhem-update-apply-command"', scope, 'shows the separate apply step directly');
      report.includes(html, 'mayhem update --apply-staged', scope, 'does not present staging alone as a completed update');
    }
  }

  if (scenario === 'source-update' || scenario === 'signed-update') {
    report.includes(html, 'data-update-notice', scope, 'renders the compact topbar update notice');
    report.includes(html, 'nav-update-badge info', scope, 'keeps a persistent update marker beside Settings');
    report.check(
      !html.includes('<section class="attention-card warn" role="status"><span class="attention-icon" aria-hidden="true">!</span><div class="attention-copy"><strong>Update available</strong>'),
      scope,
      'does not render a large optional update banner',
    );
    if (route.id === 'settings' && scenario === 'source-update') {
      report.includes(html, 'Source update only.', scope, 'explains that source changes are not an executable update');
      report.includes(html, 'View changes on GitHub', scope, 'links to the GitHub source comparison');
      report.check(!html.includes('id="mayhem-update-stage-command"'), scope, 'does not offer an updater command without signed assets');
    }
    if (route.id === 'settings' && scenario === 'signed-update') {
      report.includes(html, 'Agent-guided update recommended.', scope, 'recommends guided updating while preserving user review');
      report.includes(html, 'View release notes', scope, 'links to the signed release notes');
      report.includes(html, 'id="mayhem-update-stage-command"', scope, 'offers the updater only when the complete signed asset set exists');
      report.includes(html, 'id="mayhem-update-apply-command"', scope, 'keeps staging and application separate');
    }
  }
}

async function exerciseWorkbench(baseUrl, options) {
  const report = new CheckReport(options.verbose);
  const requestOptions = { timeoutMs: options.timeoutMs };

  const health = await get(baseUrl, '/__workbench/health', requestOptions);
  const healthScope = '/__workbench/health';
  report.equal(health.status, 200, healthScope, 'returns HTTP 200');
  report.check(
    headerValue(health.headers, 'content-type').startsWith('application/json'),
    healthScope,
    'serves JSON',
    `received ${JSON.stringify(headerValue(health.headers, 'content-type'))}`,
  );
  let manifest = null;
  try {
    manifest = JSON.parse(health.text);
  } catch (error) {
    report.check(false, healthScope, 'contains valid JSON', error.message);
  }
  report.equal(manifest?.ok, true, healthScope, 'reports healthy status');
  report.equal(manifest?.fixture_only, true, healthScope, 'reports fixture-only mode');
  report.check(Array.isArray(manifest?.scenarios), healthScope, 'reports a scenario list');
  const scenarios = Array.isArray(manifest?.scenarios) ? manifest.scenarios : [];
  report.equal(new Set(scenarios).size, scenarios.length, healthScope, 'reports unique scenario IDs');
  report.equal(
    JSON.stringify(scenarios),
    JSON.stringify(EXPECTED_SCENARIOS),
    healthScope,
    'reports the complete supported scenario sequence',
  );

  const index = await get(baseUrl, '/', requestOptions);
  checkHtmlResponse(report, index, '/');
  report.includes(index.text, '<title>Mayhem Workbench</title>', '/', 'uses the workbench title');
  report.includes(index.text, 'Dashboard workbench', '/', 'identifies the workbench');
  report.includes(index.text, 'fixture states', '/', 'labels fixture-backed states');
  for (const scenario of EXPECTED_SCENARIOS) {
    report.includes(
      index.text,
      `href="/mayhem/dashboard?scenario=${scenario}"`,
      '/',
      `links ${scenario} to the product Home`,
    );
  }

  const versionA = await get(baseUrl, '/__workbench/version', requestOptions);
  const versionB = await get(baseUrl, '/__workbench/version', requestOptions);
  report.equal(versionA.status, 200, '/__workbench/version', 'returns HTTP 200');
  report.check(versionA.text.trim().length > 0, '/__workbench/version', 'returns a non-empty version');
  report.equal(versionA.text, versionB.text, '/__workbench/version', 'is stable for the running process');
  report.equal(headerValue(versionA.headers, 'cache-control'), 'no-store', '/__workbench/version', 'disables caching');

  const reload = await get(baseUrl, '/__workbench/reload.js', requestOptions);
  report.equal(reload.status, 200, '/__workbench/reload.js', 'returns HTTP 200');
  report.check(
    headerValue(reload.headers, 'content-type').startsWith('text/javascript'),
    '/__workbench/reload.js',
    'serves JavaScript',
    `received ${JSON.stringify(headerValue(reload.headers, 'content-type'))}`,
  );
  report.includes(reload.text, '/__workbench/version', '/__workbench/reload.js', 'polls the version endpoint');
  report.includes(reload.text, 'window.location.reload()', '/__workbench/reload.js', 'reloads after a version change');

  const font = await get(baseUrl, '/mayhem/dashboard/assets/exo-latin.woff2', requestOptions);
  report.equal(font.status, 200, 'font asset', 'returns HTTP 200');
  report.equal(headerValue(font.headers, 'content-type'), 'font/woff2', 'font asset', 'uses WOFF2 content type');
  report.check(font.body.length > 10_000, 'font asset', 'contains a non-trivial embedded font', `received ${font.body.length} bytes`);
  report.equal(font.body.subarray(0, 4).toString('ascii'), 'wOF2', 'font asset', 'has a WOFF2 signature');

  const qwenLogo = await get(baseUrl, '/mayhem/dashboard/assets/brand/qwen.svg', requestOptions);
  report.equal(qwenLogo.status, 200, 'Qwen logo asset', 'returns HTTP 200');
  report.equal(headerValue(qwenLogo.headers, 'content-type'), 'image/svg+xml', 'Qwen logo asset', 'uses SVG content type');
  report.includes(qwenLogo.text, '<svg', 'Qwen logo asset', 'serves the materialized Qwen vector file');

  const appCss = await get(baseUrl, '/mayhem/dashboard/assets/app.css', requestOptions);
  report.equal(appCss.status, 200, 'app stylesheet', 'returns HTTP 200');
  report.check(
    headerValue(appCss.headers, 'content-type').startsWith('text/css'),
    'app stylesheet',
    'serves CSS',
    `received ${JSON.stringify(headerValue(appCss.headers, 'content-type'))}`,
  );
  report.equal(headerValue(appCss.headers, 'cache-control'), 'no-store', 'app stylesheet', 'disables caching');
  report.check(appCss.body.length > 5_000, 'app stylesheet', 'contains the full product stylesheet', `received ${appCss.body.length} bytes`);
  report.includes(appCss.text, '.app-shell', 'app stylesheet', 'styles the responsive application shell');
  report.includes(appCss.text, '@media(prefers-reduced-motion:reduce)', 'app stylesheet', 'honors the system motion preference');
  report.includes(appCss.text, '.motion-reduced', 'app stylesheet', 'supports an explicit reduced-motion preference');
  report.includes(appCss.text, '.amounts-hidden', 'app stylesheet', 'supports amount privacy mode');
  report.includes(
    appCss.text,
    '.data-table-wrap:focus-visible',
    'app stylesheet',
    'keeps keyboard-focused horizontal tables visibly located',
  );
  report.includes(
    appCss.text,
    '.subnav a{min-height:44px',
    'app stylesheet',
    'gives contextual navigation a 44px target',
  );
  report.includes(
    appCss.text,
    '.message-details>summary,.playground-composer details.field>summary{min-height:44px',
    'app stylesheet',
    'gives request and technical-detail disclosures a 44px target',
  );
  report.includes(
    appCss.text,
    '.topbar-context .topbar-status{display:inline-flex',
    'app stylesheet',
    'keeps compact critical status visible on mobile',
  );
  report.includes(
    appCss.text,
    'html.js-ready .playground-interactive.js-only{display:flex!important;flex-direction:column}',
    'app stylesheet',
    'reveals the Playground interaction layer only after JavaScript is ready',
  );
  report.includes(
    appCss.text,
    'html.js-ready .icon-button.mobile-menu-button.js-only,html.js-ready .nav-scrim{display:none!important}',
    'app stylesheet',
    'keeps mobile navigation controls hidden on desktop',
  );
  report.includes(
    appCss.text,
    '[hidden],html.js-ready .js-only[hidden]{display:none!important}',
    'app stylesheet',
    'keeps enhanced controls hidden when their state requires it',
  );
  report.check(
    !appCss.text.includes('html{min-width:320px'),
    'app stylesheet',
    'does not force document overflow in a 320px viewport with a scrollbar',
  );

  const appJs = await get(baseUrl, '/mayhem/dashboard/assets/app.js', requestOptions);
  report.equal(appJs.status, 200, 'app script', 'returns HTTP 200');
  report.check(
    headerValue(appJs.headers, 'content-type').startsWith('text/javascript'),
    'app script',
    'serves JavaScript',
    `received ${JSON.stringify(headerValue(appJs.headers, 'content-type'))}`,
  );
  report.equal(headerValue(appJs.headers, 'cache-control'), 'no-store', 'app script', 'disables caching');
  report.check(appJs.body.length > 5_000, 'app script', 'contains the full interaction layer', `received ${appJs.body.length} bytes`);
  report.includes(appJs.text, '[data-evidence-url]', 'app script', 'wires on-demand evidence links');
  report.includes(appJs.text, "cache: 'no-store'", 'app script', 'fetches evidence without caching it');
  report.includes(appJs.text, '[data-playground-form]', 'app script', 'wires the Playground flow');
  report.includes(
    appJs.text,
    "headers['x-mayhem-max-price-au']",
    'app script',
    'sends the supported per-request route-rate ceiling header',
  );
  report.includes(
    appJs.text,
    "headers['x-mayhem-min-att-tier']",
    'app script',
    'sends the supported minimum-attestation header',
  );
  report.includes(
    appJs.text,
    '1000000000000000n',
    'app script',
    'converts dollars per 1M-unit basket to the gateway 1K-basket atto-USD rate basis',
  );
  report.includes(
    appJs.text,
    '1000000000000000000n',
    'app script',
    'converts fixed-charge dollars to canonical atto-USD',
  );
  report.includes(appJs.text, 'outputLimitReached', 'app script', 'distinguishes output truncation from completion');
  report.includes(appJs.text, 'reportedFinishReason', 'app script', 'retains the streamed finish reason');
  report.includes(appJs.text, 'Incomplete response · connection ended before a finish reason', 'app script', 'labels preserved partial output as incomplete');
  report.includes(appJs.text, 'data-playground-reset-draft', 'app script', 'wires the explicit saved-draft reset');
  report.includes(appJs.text, 'Stream ended before the provider reported a finish reason.', 'app script', 'routes cleanly truncated SSE through incomplete-response recovery');
  report.includes(appJs.text, "input.type = hidden ? 'password' : 'text'", 'app script', 'masks the entered price ceiling with hidden amounts');
  report.includes(appJs.text, 'price ceiling hidden', 'app script', 'redacts the price ceiling from the preflight summary');
  report.includes(appJs.text, 'maxPriceMode', 'app script', 'persists the price unit basis beside the draft value');
  report.includes(appJs.text, 'Continue with ${nextOutputLimit.toLocaleString()}-token limit', 'app script', 'offers a higher-limit continuation after truncation');
  report.includes(
    appJs.text,
    'status === 429 && !namesCapacityBlocker',
    'app script',
    'routes token rate limits separately from provider-capacity failures',
  );
  report.includes(
    appJs.text,
    'updateFilter(target, true)',
    'app script',
    'clears persisted filter and pagination state when Escape clears the field',
  );
  try {
    const convertPrice = embeddedArrowFunction(appJs.text, 'maxPriceAuFromUsd');
    const priceInput = {
      value: '0.50',
      validationMessage: '',
      setCustomValidity(message) { this.validationMessage = message; },
    };
    report.equal(
      convertPrice(priceInput, 'rate'),
      '500000000000000',
      'app script',
      'converts a $0.50 per 1M-unit composite ceiling to the exact gateway rate basis',
    );
    priceInput.value = '0.50';
    priceInput.validationMessage = '';
    report.equal(
      convertPrice(priceInput, 'fixed'),
      '500000000000000000',
      'app script',
      'converts a $0.50 fixed route-charge ceiling to exact atto-USD',
    );
    const classifyFailure = embeddedArrowFunction(appJs.text, 'classifyPlaygroundFailure');
    report.equal(classifyFailure(409, 'receipt co-signing refused; session paused').actionHref, '/mayhem/dashboard/activity', 'app script', 'routes paused-session receipt recovery to Activity');
    report.equal(classifyFailure(429, 'bearer token rate limit reached').actionHref, '/mayhem/dashboard/connect', 'app script', 'routes token rate limits to access recovery');
    report.equal(classifyFailure(429, 'no route has capacity').actionHref, '/mayhem/dashboard/models', 'app script', 'keeps explicit capacity failures on model recovery');
  } catch (error) {
    report.check(false, 'app script', 'executes focused Playground contract helpers', error.message);
  }
  report.includes(appJs.text, '[data-session-seconds]', 'app script', 'wires the live session timer');
  report.includes(appJs.text, 'Dashboard session ends soon', 'app script', 'announces session expiry before access is lost');
  report.includes(appJs.text, "extend.dataset.sessionExtend = ''", 'app script', 'offers a low-noise session extension action');
  report.includes(
    appJs.text,
    'recordDashboardSessionActivity(initial);',
    'app script',
    'renews the fixture session in place so the warning path is behaviorally testable',
  );
  report.includes(
    appJs.text,
    'this page, draft, filters, and scroll position stay in place',
    'app script',
    'explains that extension preserves task context',
  );
  report.includes(appJs.text, 'Export shown page', 'app script', 'labels CSV export as page-local');
  report.includes(appJs.text, 'tableToolParameter', 'app script', 'isolates query state for multiple tables on one page');
  report.includes(
    appJs.text,
    'recordDashboardSessionActivity',
    'app script',
    'synchronizes the timer after user-triggered authenticated requests',
  );
  report.check(
    !appJs.text.includes("document.addEventListener('visibilitychange'"),
    'app script',
    'does not keep an idle browser session alive through passive visibility polling',
  );
  report.includes(appJs.text, '[data-preference]', 'app script', 'wires saved presentation preferences');

  const rendered = new Map();
  let routeCount = 0;
  for (const scenario of EXPECTED_SCENARIOS) {
    const coveredRouteIds = SCENARIO_ROUTE_IDS[scenario];
    const coveredRoutes = PRODUCT_ROUTES.filter((route) => coveredRouteIds.has(route.id));
    const groupFailureCount = report.failures.length;
    for (const route of coveredRoutes) {
      routeCount += 1;
      const pathname = `${route.path}?scenario=${encodeURIComponent(scenario)}`;
      const scope = `${scenario}/${route.id}`;
      let response;
      try {
        response = await get(baseUrl, pathname, requestOptions);
      } catch (error) {
        report.check(false, scope, 'request completes', error.message);
        continue;
      }
      if (scenario === 'showcase' || scenario === 'scale') {
        rendered.set(`${scenario}/${route.id}`, response.text);
      }
      checkHtmlResponse(report, response, scope);
      const ids = [...response.text.matchAll(/\sid="([^"]+)"/g)].map((match) => match[1]);
      report.equal(new Set(ids).size, ids.length, scope, 'uses unique element IDs');
      report.check(!/\s(?:onclick|onchange|oninput|onsubmit)=/i.test(response.text), scope, 'uses no inline event handlers');
      report.check(!/\sstyle=/i.test(response.text), scope, 'uses no inline style attributes');
      report.equal(
        countOccurrences(response.text, '<caption'),
        countOccurrences(response.text, '<table'),
        scope,
        'gives every data table a programmatic caption',
      );
      report.equal(
        countOccurrences(
          response.text,
          'class="data-table-wrap" role="region" tabindex="0" aria-label="',
        ),
        countOccurrences(response.text, '<table'),
        scope,
        'makes every horizontally scrollable table a named keyboard region',
      );
      if (response.text.includes('<table')) {
        report.includes(
          response.text,
          'Scroll horizontally to view all columns.',
          scope,
          'explains how to reach columns outside the viewport',
        );
      }
      report.includes(response.text, `<title>${route.title}</title>`, scope, 'uses the product document title');
      report.equal(
        (response.text.match(/<h1(?:\s[^>]*)?>/g) ?? []).length,
        1,
        scope,
        'renders one page-level heading',
      );
      if (route.heading !== undefined) {
        report.includes(response.text, `<h1>${route.heading}</h1>`, scope, 'uses the route heading');
      }
      report.includes(response.text, '<body class="has-workbench">', scope, 'marks workbench rendering');
      report.includes(response.text, '<div class="app-shell">', scope, 'renders the shared application shell');
      report.includes(response.text, '<a class="skip-link" href="#main-content">', scope, 'offers a keyboard skip link');
      report.includes(response.text, 'aria-label="Mayhem navigation"', scope, 'labels the application navigation');
      report.includes(response.text, '<nav class="app-nav" aria-label="Primary">', scope, 'renders labeled primary navigation');
      report.includes(response.text, '<main class="app-main', scope, 'provides a focusable main landmark');
      report.includes(response.text, 'id="main-content" tabindex="-1">', scope, 'keeps the main landmark focusable');
      report.includes(response.text, '<header class="app-topbar">', scope, 'renders the shared top bar');
      report.includes(response.text, 'data-page-status-text', scope, 'keeps critical page status in a compact mobile-safe text wrapper');
      const hasMoney = response.text.includes('data-money');
      report.equal(
        countOccurrences(response.text, 'data-hide-amounts'),
        hasMoney ? 1 : 0,
        scope,
        hasMoney
          ? 'offers one amount-visibility control when monetary values are present'
          : 'omits the no-op amount-visibility control when no monetary values are present',
      );
      report.includes(response.text, `<strong>${route.pageLabel}</strong>`, scope, 'labels the current product area');
      report.includes(response.text, 'data-session-seconds', scope, 'renders the session timer hook');
      report.includes(response.text, 'data-session-status', scope, 'renders the session status hook');
      report.includes(response.text, 'aria-label="Mobile primary"', scope, 'renders labeled mobile navigation');
      if (MOBILE_MORE_ROUTE_IDS.has(route.id)) {
        report.includes(
          response.text,
          'aria-expanded="false" aria-current="page">More</button>',
          scope,
          'marks More as the current mobile destination for a secondary route',
        );
      }
      report.includes(
        response.text,
        'data-toast-region role="status" aria-live="polite" aria-atomic="true"',
        scope,
        'provides an accessible live toast region',
      );
      report.includes(response.text, 'href="/mayhem/dashboard/assets/app.css"', scope, 'loads the product stylesheet');
      report.includes(response.text, 'src="/mayhem/dashboard/assets/app.js"', scope, 'loads the product interaction layer');
      report.equal(
        countOccurrences(response.text, '<dialog class="verify-dialog"'),
        1,
        scope,
        'renders one reusable evidence dialog',
      );
      report.includes(
        response.text,
        '<pre class="raw-evidence" data-evidence-raw></pre>',
        scope,
        'keeps the reusable evidence payload empty until requested',
      );
      report.check(
        !response.text.includes('id="verify-'),
        scope,
        'does not embed per-row evidence dialogs or payload IDs',
      );
      const evidenceLinkCount = countOccurrences(response.text, 'data-evidence-url');
      report.check(
        countOccurrences(response.text, 'href="/mayhem/dashboard/evidence?') >= evidenceLinkCount,
        scope,
        'backs every evidence trigger with a progressively enhanced link',
      );
      for (const primaryPath of PRIMARY_NAV_PATHS) {
        report.includes(response.text, `href="${primaryPath}"`, scope, `links primary destination ${primaryPath}`);
      }
      if (route.primaryPath !== undefined) {
        const navigationLabel = route.id === 'models'
          ? 'Model catalog'
          : route.id.startsWith('network')
            ? 'Network explorer'
            : route.pageLabel;
        report.includes(
          response.text,
          `<a href="${route.primaryPath}" aria-label="${navigationLabel}" aria-current="page">`,
          scope,
          'marks the current navigation destination',
        );
      }
      if (route.context !== undefined) {
        report.includes(
          response.text,
          `<nav class="subnav" aria-label="${route.context} sections">`,
          scope,
          `renders the ${route.context} contextual navigation`,
        );
        report.includes(
          response.text,
          `<a href="${route.path}" aria-current="page">`,
          scope,
          'marks the current contextual destination',
        );
      }
      for (const marker of route.markers) {
        report.includes(response.text, marker, scope, `retains route invariant ${marker}`);
      }
      report.includes(response.text, 'data-workbench-chrome', scope, 'renders workbench controls');
      report.includes(response.text, '/__workbench/reload.js', scope, 'loads the workbench reload helper');
      const title = SCENARIO_TITLES.get(scenario);
      if (title !== undefined) {
        report.includes(response.text, `Fixture: ${title}`, scope, 'labels the selected fixture');
      }
      report.includes(
        response.text,
        `class="workbench-scenario active" href="${route.path}?scenario=${scenario}"`,
        scope,
        'marks the requested scenario active for this route',
      );
      report.equal(
        countOccurrences(response.text, 'class="workbench-scenario active"'),
        1,
        scope,
        'marks only one scenario active',
      );
      const cookies = setCookies(response.headers);
      report.check(
        cookies.some((cookie) => cookie.includes(`mayhem_dashboard_workbench_scenario=${scenario}`)),
        scope,
        'sets the selected scenario cookie',
        `received ${JSON.stringify(cookies)}`,
      );
      report.check(
        cookies.some((cookie) => cookie.includes('Path=/') && cookie.includes('SameSite=Lax')),
        scope,
        'scopes the scenario cookie safely',
        `received ${JSON.stringify(cookies)}`,
      );
      checkScenarioSemantics(report, scenario, route, response.text);
    }
    const passed = report.failures.length === groupFailureCount;
    console.log(`  [${passed ? 'ok' : 'FAIL'}] ${scenario}: ${coveredRoutes.length} product routes`);
  }

  for (const routeId of DENSITY_ROUTE_IDS) {
    const showcase = rendered.get(`showcase/${routeId}`) ?? '';
    const scale = rendered.get(`scale/${routeId}`) ?? '';
    const scope = `scale/${routeId}`;
    report.check(
      Buffer.byteLength(scale) > Buffer.byteLength(showcase),
      scope,
      'contains more fixture content than showcase',
      `showcase ${Buffer.byteLength(showcase)} bytes, scale ${Buffer.byteLength(scale)} bytes`,
    );
    report.check(
      countOccurrences(scale, '<tr') > countOccurrences(showcase, '<tr'),
      scope,
      'renders more table rows than showcase',
      `showcase ${countOccurrences(showcase, '<tr')} rows, scale ${countOccurrences(scale, '<tr')} rows`,
    );
  }

  const scaleModels = rendered.get('scale/models') ?? '';
  report.equal(
    tableBodyRowCount(scaleModels, 'Models in this gateway catalog'),
    MODEL_ROW_CAP,
    'scale/models',
    'caps the model table at the documented renderer limit',
  );
  report.includes(
    scaleModels,
    `id="models-count">${MODEL_ROW_CAP} shown rows`,
    'scale/models',
    'reports the number of rendered model rows',
  );
  report.includes(
    scaleModels,
    `Showing rows 1&ndash;${MODEL_ROW_CAP} of ${SCALE_MODEL_COUNT} catalog models. Page 1 of ${lastPage(SCALE_MODEL_COUNT, MODEL_ROW_CAP)}.`,
    'scale/models',
    'reports the current model row range and page count',
  );
  report.includes(
    scaleModels,
    'rel="next" href="/mayhem/dashboard/models?page=2"',
    'scale/models',
    'links to the next server-rendered model page',
  );

  const scaleSelection = await get(
    baseUrl,
    '/mayhem/dashboard/models?scenario=scale',
    requestOptions,
  );
  const scaleScenarioCookie = setCookies(scaleSelection.headers)
    .map((cookie) => cookie.split(';', 1)[0])
    .find((cookie) => cookie.startsWith('mayhem_dashboard_workbench_scenario='));
  report.check(
    scaleScenarioCookie !== undefined,
    'scale/pagination',
    'captures the selected scale scenario cookie',
  );
  const scalePageOptions = {
    ...requestOptions,
    headers: { Cookie: scaleScenarioCookie ?? '' },
  };
  const modelLastPage = lastPage(SCALE_MODEL_COUNT, MODEL_ROW_CAP);
  const scaleModelsSecondResponse = await get(
    baseUrl,
    `/mayhem/dashboard/models?page=${modelLastPage}`,
    scalePageOptions,
  );
  const scaleModelsSecond = scaleModelsSecondResponse.text;
  report.equal(scaleModelsSecondResponse.status, 200, 'scale/models last page', 'returns HTTP 200');
  report.equal(
    tableBodyRowCount(scaleModelsSecond, 'Models in this gateway catalog'),
    lastPageRows(SCALE_MODEL_COUNT, MODEL_ROW_CAP),
    'scale/models last page',
    'renders every remaining model row',
  );
  report.includes(
    scaleModelsSecond,
    `Showing rows ${lastPageStart(SCALE_MODEL_COUNT, MODEL_ROW_CAP)}&ndash;${SCALE_MODEL_COUNT} of ${SCALE_MODEL_COUNT} catalog models. Page ${modelLastPage} of ${modelLastPage}.`,
    'scale/models last page',
    'reports the final model row range',
  );
  report.includes(scaleModelsSecond, 'workbench/96-', 'scale/models last page', 'makes the final model reachable');
  report.includes(scaleModelsSecond, 'Fixture: Scale and overflow', 'scale/models last page', 'preserves the fixture through its cookie');
  report.check(
    !scaleModelsSecond.includes('href="/mayhem/dashboard/models?scenario=scale&amp;page=1"'),
    'scale/models last page',
    'keeps pagination URLs product-clean and relies on the local fixture cookie',
  );

  const scaleActivity = rendered.get('scale/activity') ?? '';
  report.equal(
    tableBodyRowCount(
      scaleActivity,
      'Prioritized incomplete records, final receipts, and retained pause records from this gateway process',
    ),
    ACTIVITY_ROW_CAP,
    'scale/activity',
    'caps activity rows at the documented renderer limit',
  );
  report.includes(
    scaleActivity,
    '<span class="metric-label">Final receipts</span>',
    'scale/activity',
    'summarizes final receipts without a raw checkpoint card',
  );
  report.includes(
    scaleActivity,
    `Showing rows 1&ndash;${ACTIVITY_ROW_CAP} of ${SCALE_RECEIPT_COUNT} recorded sessions. Page 1 of ${lastPage(SCALE_RECEIPT_COUNT, ACTIVITY_ROW_CAP)}.`,
    'scale/activity',
    'reports the current activity row range and page count',
  );
  const activityLastPage = lastPage(SCALE_RECEIPT_COUNT, ACTIVITY_ROW_CAP);
  const scaleActivitySecond = (await get(
    baseUrl,
    `/mayhem/dashboard/activity?page=${activityLastPage}`,
    scalePageOptions,
  )).text;
  report.equal(
    tableBodyRowCount(scaleActivitySecond, 'Prioritized incomplete records, final receipts, and retained pause records from this gateway process'),
    lastPageRows(SCALE_RECEIPT_COUNT, ACTIVITY_ROW_CAP),
    'scale/activity last page',
    'renders every remaining activity row',
  );
  report.includes(
    scaleActivitySecond,
    `Showing rows ${lastPageStart(SCALE_RECEIPT_COUNT, ACTIVITY_ROW_CAP)}&ndash;${SCALE_RECEIPT_COUNT} of ${SCALE_RECEIPT_COUNT} recorded sessions. Page ${activityLastPage} of ${activityLastPage}.`,
    'scale/activity last page',
    'reports the final activity row range',
  );
  report.includes(scaleActivitySecond, 'workbench-session-96', 'scale/activity last page', 'makes the final recorded session reachable');

  const scaleConnect = rendered.get('scale/connect') ?? '';
  report.equal(
    tableBodyRowCount(scaleConnect, 'Gateway access tokens, budgets, scopes, and status'),
    TOKEN_ROW_CAP,
    'scale/connect',
    'caps token rows at the documented renderer limit',
  );
  report.includes(scaleConnect, 'id="access-tokens-table"', 'scale/connect', 'gives the access-token table a stable tool target');
  report.includes(scaleConnect, 'data-table-filter="#access-tokens-table"', 'scale/connect', 'filters only access tokens on the shown page');
  report.equal(countOccurrences(scaleConnect, 'data-filter-row'), TOKEN_ROW_CAP, 'scale/connect', 'makes each shown token sortable and exportable');
  report.includes(
    scaleConnect,
    `Showing rows 1&ndash;${TOKEN_ROW_CAP} of ${SCALE_TOKEN_COUNT} access tokens. Page 1 of ${lastPage(SCALE_TOKEN_COUNT, TOKEN_ROW_CAP)}.`,
    'scale/connect',
    'reports the first reachable token page',
  );
  report.includes(
    scaleConnect,
    'Scale active 64',
    'scale/connect',
    'keeps active tokens ahead of older inactive records',
  );
  const tokenLastPage = lastPage(SCALE_TOKEN_COUNT, TOKEN_ROW_CAP);
  const scaleConnectSecond = (await get(
    baseUrl,
    `/mayhem/dashboard/connect?page=${tokenLastPage}`,
    scalePageOptions,
  )).text;
  report.equal(
    tableBodyRowCount(scaleConnectSecond, 'Gateway access tokens, budgets, scopes, and status'),
    lastPageRows(SCALE_TOKEN_COUNT, TOKEN_ROW_CAP),
    'scale/connect last page',
    'renders every remaining access token',
  );
  report.includes(
    scaleConnectSecond,
    `Showing rows ${lastPageStart(SCALE_TOKEN_COUNT, TOKEN_ROW_CAP)}&ndash;${SCALE_TOKEN_COUNT} of ${SCALE_TOKEN_COUNT} access tokens. Page ${tokenLastPage} of ${tokenLastPage}.`,
    'scale/connect last page',
    'reports the final token row range',
  );
  report.includes(scaleConnectSecond, 'Scale inactive 56', 'scale/connect last page', 'makes the final sorted token reachable');

  const scaleEarn = rendered.get('scale/earn') ?? '';
  report.equal(
    tableBodyRowCount(scaleEarn, 'Configured provider serving routes and current capacity'),
    PROVIDER_ROW_CAP,
    'scale/earn',
    'bounds provider overview routes',
  );
  report.includes(scaleEarn, 'data-table-filter="#earn-routes-table"', 'scale/earn', 'adds shown-page tools to provider overview routes');
  const routeLastPage = lastPage(SCALE_ROUTE_COUNT, PROVIDER_ROW_CAP);
  report.includes(
    scaleEarn,
    `Showing rows 1&ndash;${PROVIDER_ROW_CAP} of ${SCALE_ROUTE_COUNT} configured serving routes. Page 1 of ${routeLastPage}.`,
    'scale/earn',
    'reports the provider overview range',
  );
  const scaleEarnLast = (await get(baseUrl, `/mayhem/dashboard/earn?page=${routeLastPage}`, scalePageOptions)).text;
  report.equal(
    tableBodyRowCount(scaleEarnLast, 'Configured provider serving routes and current capacity'),
    lastPageRows(SCALE_ROUTE_COUNT, PROVIDER_ROW_CAP),
    'scale/earn last page',
    'makes every provider overview route reachable',
  );

  const scaleMachines = rendered.get('scale/earn-machines') ?? '';
  report.equal(
    tableBodyRowCount(scaleMachines, 'Machine routes for the configured provider identity'),
    PROVIDER_ROW_CAP,
    'scale/earn-machines',
    'bounds provider machine routes',
  );
  report.includes(scaleMachines, 'data-table-filter="#machine-routes-table"', 'scale/earn-machines', 'adds shown-page tools to machine routes');
  const scaleMachinesLast = (await get(baseUrl, `/mayhem/dashboard/earn/machines?page=${routeLastPage}`, scalePageOptions)).text;
  report.equal(
    tableBodyRowCount(scaleMachinesLast, 'Machine routes for the configured provider identity'),
    lastPageRows(SCALE_ROUTE_COUNT, PROVIDER_ROW_CAP),
    'scale/earn-machines last page',
    'makes every provider machine route reachable',
  );

  const scaleReliability = rendered.get('scale/earn-reliability') ?? '';
  report.equal(
    tableBodyRowCount(scaleReliability, 'Provider route reputation, probation, and gateway observations'),
    PROVIDER_ROW_CAP,
    'scale/earn-reliability',
    'bounds provider reliability rows',
  );
  report.includes(scaleReliability, 'data-table-filter="#reliability-routes-table"', 'scale/earn-reliability', 'adds shown-page tools to reliability routes');
  const scaleReliabilityLast = (await get(baseUrl, `/mayhem/dashboard/earn/reliability?page=${routeLastPage}`, scalePageOptions)).text;
  report.equal(
    tableBodyRowCount(scaleReliabilityLast, 'Provider route reputation, probation, and gateway observations'),
    lastPageRows(SCALE_ROUTE_COUNT, PROVIDER_ROW_CAP),
    'scale/earn-reliability last page',
    'makes every provider reliability route reachable',
  );

  const scaleOpportunities = rendered.get('scale/earn-opportunities') ?? '';
  report.equal(
    tableBodyRowCount(
      scaleOpportunities,
      'Catalog models, gateway-host compatibility, and advertised supply',
    ),
    MODEL_ROW_CAP,
    'scale/earn-opportunities',
    'caps host-fit rows at the model renderer limit',
  );
  report.includes(
    scaleOpportunities,
    `Showing rows 1&ndash;${MODEL_ROW_CAP} of ${SCALE_MODEL_COUNT} catalog models. Page 1 of ${modelLastPage}.`,
    'scale/earn-opportunities',
    'reports the current host-fit row range and page count',
  );
  const scaleOpportunitiesSecond = (await get(
    baseUrl,
    `/mayhem/dashboard/earn/opportunities?page=${modelLastPage}`,
    scalePageOptions,
  )).text;
  report.equal(
    tableBodyRowCount(scaleOpportunitiesSecond, 'Catalog models, gateway-host compatibility, and advertised supply'),
    lastPageRows(SCALE_MODEL_COUNT, MODEL_ROW_CAP),
    'scale/earn-opportunities last page',
    'renders every remaining host-fit row',
  );
  report.includes(scaleOpportunitiesSecond, 'workbench/96-', 'scale/earn-opportunities last page', 'makes the final host-fit row reachable');

  const scaleNetworkModels = rendered.get('scale/network-models') ?? '';
  report.equal(
    tableBodyRowCount(
      scaleNetworkModels,
      'Network models, advertised capacity, capabilities, and price',
    ),
    MODEL_ROW_CAP,
    'scale/network-models',
    'caps network model rows at the documented renderer limit',
  );
  report.includes(
    scaleNetworkModels,
    `Showing rows 1&ndash;${MODEL_ROW_CAP} of ${SCALE_MODEL_COUNT} network models. Page 1 of ${modelLastPage}.`,
    'scale/network-models',
    'reports the current network-model row range and page count',
  );
  const scaleNetworkModelsSecond = (await get(
    baseUrl,
    `/mayhem/dashboard/network/models?page=${modelLastPage}`,
    scalePageOptions,
  )).text;
  report.equal(
    tableBodyRowCount(scaleNetworkModelsSecond, 'Network models, advertised capacity, capabilities, and price'),
    lastPageRows(SCALE_MODEL_COUNT, MODEL_ROW_CAP),
    'scale/network-models last page',
    'renders every remaining network-model row',
  );
  report.includes(scaleNetworkModelsSecond, 'workbench/96-', 'scale/network-models last page', 'makes the final network model reachable');

  const scaleProviders = rendered.get('scale/network-providers') ?? '';
  report.equal(
    tableBodyRowCount(
      scaleProviders,
      'Canonical provider routes and current operational evidence',
    ),
    PROVIDER_ROW_CAP,
    'scale/network-providers',
    'caps provider rows at the documented renderer limit',
  );
  report.includes(
    scaleProviders,
    `Showing rows 1&ndash;${PROVIDER_ROW_CAP} of ${SCALE_ROUTE_COUNT} catalog provider routes. Page 1 of ${routeLastPage}.`,
    'scale/network-providers',
    'reports the current provider-route row range and page count',
  );
  const scaleProvidersLast = (await get(
    baseUrl,
    `/mayhem/dashboard/network/providers?page=${routeLastPage}`,
    scalePageOptions,
  )).text;
  report.equal(
    tableBodyRowCount(scaleProvidersLast, 'Canonical provider routes and current operational evidence'),
    lastPageRows(SCALE_ROUTE_COUNT, PROVIDER_ROW_CAP),
    'scale/network-providers last page',
    'renders every provider route on the final page',
  );
  report.includes(
    scaleProvidersLast,
    `Showing rows ${lastPageStart(SCALE_ROUTE_COUNT, PROVIDER_ROW_CAP)}&ndash;${SCALE_ROUTE_COUNT} of ${SCALE_ROUTE_COUNT} catalog provider routes. Page ${routeLastPage} of ${routeLastPage}.`,
    'scale/network-providers last page',
    'reports the final provider-route range',
  );

  const scaleMarketsSecond = (await get(
    baseUrl,
    '/mayhem/dashboard/network/markets?page=2',
    scalePageOptions,
  )).text;
  report.check(
    tableBodyRowCount(scaleMarketsSecond, 'Catalog markets and reference prices') > 0,
    'scale/network-markets page 2',
    'renders catalog markets beyond the first page',
  );
  report.check(
    new RegExp(`Showing rows ${PROVIDER_ROW_CAP + 1}&ndash;\\d+ of \\d+ catalog markets\\. Page 2 of \\d+\\.`).test(scaleMarketsSecond),
    'scale/network-markets page 2',
    'reports a truthful later market row range',
  );

  const scaleNetworkActivity = rendered.get('scale/network-activity') ?? '';
  report.equal(
    tableBodyRowCount(
      scaleNetworkActivity,
      'Provider route observations ordered by heartbeat freshness',
    ),
    EVIDENCE_ROW_CAP,
    'scale/network-activity',
    'caps current route observations at the evidence limit',
  );
  const evidenceLastPage = lastPage(SCALE_ROUTE_COUNT, EVIDENCE_ROW_CAP);
  report.includes(
    scaleNetworkActivity,
    `Showing rows 1&ndash;${EVIDENCE_ROW_CAP} of ${SCALE_ROUTE_COUNT} route observations. Page 1 of ${evidenceLastPage}.`,
    'scale/network-activity',
    'reports the current route-observation range and page count',
  );
  const scaleNetworkActivityLast = (await get(
    baseUrl,
    `/mayhem/dashboard/network/activity?page=${evidenceLastPage}`,
    scalePageOptions,
  )).text;
  report.equal(
    tableBodyRowCount(scaleNetworkActivityLast, 'Provider route observations ordered by heartbeat freshness'),
    lastPageRows(SCALE_ROUTE_COUNT, EVIDENCE_ROW_CAP),
    'scale/network-activity last page',
    'renders every route observation on the final page',
  );

  const scaleNetworkEvidence = rendered.get('scale/network-evidence') ?? '';
  report.equal(
    tableBodyRowCount(scaleNetworkEvidence, 'Provider route evidence'),
    EVIDENCE_ROW_CAP,
    'scale/network-evidence',
    'caps route-evidence rows at the documented limit',
  );
  report.includes(
    scaleNetworkEvidence,
    `Showing rows 1&ndash;${EVIDENCE_ROW_CAP} of ${SCALE_ROUTE_COUNT} route entries. Page 1 of ${evidenceLastPage}.`,
    'scale/network-evidence',
    'reports the current route-evidence range and page count',
  );
  const scaleNetworkEvidenceLast = (await get(
    baseUrl,
    `/mayhem/dashboard/network/evidence?page=${evidenceLastPage}`,
    scalePageOptions,
  )).text;
  report.equal(
    tableBodyRowCount(scaleNetworkEvidenceLast, 'Provider route evidence'),
    lastPageRows(SCALE_ROUTE_COUNT, EVIDENCE_ROW_CAP),
    'scale/network-evidence last page',
    'renders every route-evidence row on the final page',
  );
  report.equal(
    tableBodyRowCount(scaleNetworkEvidence, 'Verification probe evidence'),
    EVIDENCE_ROW_CAP,
    'scale/network-evidence probes',
    'bounds verification probes independently of route evidence',
  );
  report.includes(scaleNetworkEvidence, 'data-table-filter="#evidence-probes-table"', 'scale/network-evidence probes', 'adds shown-page tools to probes');
  report.includes(scaleNetworkEvidence, 'data-table-query-prefix="probe"', 'scale/network-evidence probes', 'isolates probe filter and sort URL state');
  const probeLastPage = lastPage(SCALE_PROBE_COUNT, EVIDENCE_ROW_CAP);
  report.includes(
    scaleNetworkEvidence,
    `Showing rows 1&ndash;${EVIDENCE_ROW_CAP} of ${SCALE_PROBE_COUNT} probe events. Page 1 of ${probeLastPage}.`,
    'scale/network-evidence probes',
    'reports the first probe range',
  );
  const scaleProbeLast = (await get(
    baseUrl,
    `/mayhem/dashboard/network/evidence?page=2&probe_page=${probeLastPage}`,
    scalePageOptions,
  )).text;
  report.equal(
    tableBodyRowCount(scaleProbeLast, 'Verification probe evidence'),
    lastPageRows(SCALE_PROBE_COUNT, EVIDENCE_ROW_CAP),
    'scale/network-evidence probes last page',
    'makes every probe event reachable',
  );
  report.includes(
    scaleProbeLast,
    `/mayhem/dashboard/network/evidence?page=2&amp;probe_page=${probeLastPage - 1}`,
    'scale/network-evidence probes last page',
    'preserves the route page while paging probes',
  );
  report.includes(
    scaleProbeLast,
    `/mayhem/dashboard/network/evidence?probe_page=${probeLastPage}&amp;page=1`,
    'scale/network-evidence probes last page',
    'preserves the probe page while paging routes',
  );

  const evidenceHref = firstEvidenceHref(scaleModels);
  report.check(evidenceHref !== null, 'on-demand evidence', 'finds an evidence link in the scale Models page');
  if (evidenceHref !== null) {
    const evidence = await get(baseUrl, evidenceHref, {
      ...requestOptions,
      headers: { Accept: 'application/json' },
    });
    report.equal(evidence.status, 200, 'on-demand evidence', 'returns the requested evidence snapshot');
    report.check(
      headerValue(evidence.headers, 'content-type').startsWith('application/json'),
      'on-demand evidence',
      'serves the requested snapshot as JSON',
      `received ${JSON.stringify(headerValue(evidence.headers, 'content-type'))}`,
    );
    report.equal(headerValue(evidence.headers, 'cache-control'), 'no-store', 'on-demand evidence', 'disables evidence caching');
    let payload = null;
    try {
      payload = JSON.parse(evidence.text);
    } catch (error) {
      report.check(false, 'on-demand evidence', 'contains valid JSON', error.message);
    }
    report.equal(payload?.title, 'Model evidence', 'on-demand evidence', 'returns the requested evidence kind');
    report.check(typeof payload?.raw?.id === 'string', 'on-demand evidence', 'loads raw model evidence only after request');
  }

  const failureSelection = await get(
    baseUrl,
    '/mayhem/dashboard/earn?scenario=failure',
    requestOptions,
  );
  const scenarioCookie = setCookies(failureSelection.headers)
    .map((cookie) => cookie.split(';', 1)[0])
    .find((cookie) => cookie.startsWith('mayhem_dashboard_workbench_scenario='));
  report.check(scenarioCookie !== undefined, 'scenario cookie', 'can capture a selected scenario cookie');
  if (scenarioCookie !== undefined) {
    const remembered = await get(baseUrl, '/mayhem/dashboard/earn', {
      ...requestOptions,
      headers: { Cookie: scenarioCookie },
    });
    report.includes(remembered.text, 'Fixture: Provider failure', 'scenario cookie', 'restores the remembered scenario');
    report.includes(remembered.text, 'Setup blocked by model failure', 'scenario cookie', 'restores the remembered fixture data');

    const overridden = await get(baseUrl, '/mayhem/dashboard/earn?scenario=loading', {
      ...requestOptions,
      headers: { Cookie: scenarioCookie },
    });
    report.includes(overridden.text, 'Fixture: Provider loading', 'scenario query', 'query overrides the remembered cookie');
    report.includes(overridden.text, 'Download is 68% complete', 'scenario query', 'query selects the requested fixture data');
  }

  const invalid = await get(baseUrl, '/mayhem/dashboard?scenario=not-a-scenario', requestOptions);
  report.includes(invalid.text, 'Fixture: Showcase', 'invalid scenario', 'falls back to showcase explicitly');
  report.check(
    setCookies(invalid.headers).some((cookie) => cookie.includes('mayhem_dashboard_workbench_scenario=showcase')),
    'invalid scenario',
    'persists the showcase fallback',
    `received ${JSON.stringify(setCookies(invalid.headers))}`,
  );

  const showcasePlayground = await get(
    baseUrl,
    '/mayhem/dashboard/playground?scenario=showcase',
    requestOptions,
  );
  const showcaseCookie = setCookies(showcasePlayground.headers)
    .map((cookie) => cookie.split(';', 1)[0])
    .find((cookie) => cookie.startsWith('mayhem_dashboard_workbench_scenario='));
  const showcaseModel = selectedPlaygroundModel(showcasePlayground.text);
  report.check(showcaseCookie !== undefined, 'playground state slice', 'captures the Showcase scenario cookie');
  report.check(showcaseModel !== null, 'playground state slice', 'finds the selected runnable fixture model');

  if (showcaseCookie !== undefined && showcaseModel !== null) {
    const homeBeforeRequest = await get(baseUrl, '/mayhem/dashboard', {
      ...requestOptions,
      headers: { Cookie: showcaseCookie },
    });
    report.includes(
      homeBeforeRequest.text,
      '4 receipt records',
      'playground state slice',
      'starts from the deterministic Showcase receipt fixture',
    );

    const chatRequest = (prompt) => ({
      model: showcaseModel,
      messages: [{ role: 'user', content: prompt }],
      stream: true,
      stream_options: { include_usage: true },
    });
    const firstChat = await postJson(
      baseUrl,
      '/v1/chat/completions',
      chatRequest('first workbench smoke request'),
      { ...requestOptions, headers: { Cookie: showcaseCookie } },
    );
    const secondChat = await postJson(
      baseUrl,
      '/v1/chat/completions',
      chatRequest('second workbench smoke request'),
      { ...requestOptions, headers: { Cookie: showcaseCookie } },
    );
    report.equal(firstChat.status, 200, 'playground state slice', 'serves the first Showcase request');
    report.equal(secondChat.status, 200, 'playground state slice', 'serves a repeated Showcase request');
    report.check(
      headerValue(firstChat.headers, 'content-type').startsWith('text/event-stream'),
      'playground state slice',
      'streams a successful Showcase response',
      `received ${JSON.stringify(headerValue(firstChat.headers, 'content-type'))}`,
    );
    report.includes(firstChat.text, 'data: [DONE]', 'playground state slice', 'closes the SSE stream explicitly');
    const firstEvent = firstSsePayload(firstChat.text);
    const secondEvent = firstSsePayload(secondChat.text);
    const firstPayloads = ssePayloads(firstChat.text);
    const secondPayloads = ssePayloads(secondChat.text);
    const firstFinish = firstPayloads.find((payload) => payload?.choices?.[0]?.finish_reason);
    const secondFinish = secondPayloads.find((payload) => payload?.choices?.[0]?.finish_reason);
    const firstReceipt = firstPayloads
      .find((payload) => payload?.mayhem?.receipt)?.mayhem?.receipt;
    const secondReceipt = secondPayloads
      .find((payload) => payload?.mayhem?.receipt)?.mayhem?.receipt;
    report.equal(firstEvent?.model, showcaseModel, 'playground state slice', 'reports the selected model in the SSE chunk');
    report.equal(secondEvent?.model, showcaseModel, 'playground state slice', 'keeps the reported model stable');
    report.equal(firstFinish?.choices?.[0]?.finish_reason, 'stop', 'playground state slice', 'reports a normal stop finish reason');
    report.equal(secondFinish?.choices?.[0]?.finish_reason, 'stop', 'playground state slice', 'reports the repeated request finish reason');
    const lengthChat = await postJson(
      baseUrl,
      '/v1/chat/completions',
      { ...chatRequest('exercise the deterministic output limit'), max_tokens: 64 },
      {
        ...requestOptions,
        headers: { Cookie: 'mayhem_dashboard_workbench_scenario=scale' },
      },
    );
    const lengthPayloads = ssePayloads(lengthChat.text);
    report.equal(lengthChat.status, 200, 'playground output limit', 'serves the deterministic length fixture');
    report.equal(
      lengthPayloads.find((payload) => payload?.choices?.[0]?.finish_reason)?.choices?.[0]?.finish_reason,
      'length',
      'playground output limit',
      'reports the production-shaped length finish reason',
    );
    report.equal(
      lengthPayloads.find((payload) => payload?.usage)?.usage?.completion_tokens,
      64,
      'playground output limit',
      'reports usage at the requested output limit',
    );
    report.check(
      lengthPayloads.some((payload) => payload?.choices?.[0]?.delta?.content?.includes('Workbench output-limit fixture')),
      'playground output limit',
      'streams deterministic partial content before the length finish',
    );
    report.check(
      typeof firstEvent?.id === 'string' && firstEvent.id.startsWith('workbench-live-'),
      'playground state slice',
      'returns a fixture session identifier',
      `received ${JSON.stringify(firstEvent?.id)}`,
    );
    report.check(
      typeof secondEvent?.id === 'string' && secondEvent.id !== firstEvent?.id,
      'playground state slice',
      'does not collide across repeated sends',
      `received ${JSON.stringify([firstEvent?.id, secondEvent?.id])}`,
    );
    report.equal(
      firstReceipt?.session_id,
      firstEvent?.id,
      'playground state slice',
      'reports the first final receipt when usage metadata is requested',
    );
    report.equal(
      firstReceipt?.final,
      true,
      'playground state slice',
      'marks the reported receipt final',
    );
    report.check(
      firstReceipt?.au_owed_cum != null,
      'playground state slice',
      'reports the actual cumulative fixture charge',
    );
    report.equal(
      secondReceipt?.session_id,
      secondEvent?.id,
      'playground state slice',
      'associates the repeated request with its own receipt',
    );

    const homeAfterRequest = await get(baseUrl, '/mayhem/dashboard', {
      ...requestOptions,
      headers: { Cookie: showcaseCookie },
    });
    report.includes(
      homeAfterRequest.text,
      '6 receipt records',
      'playground state slice',
      'updates the Home snapshot after both completed requests',
    );
    const activityAfterRequest = await get(baseUrl, '/mayhem/dashboard/activity', {
      ...requestOptions,
      headers: { Cookie: showcaseCookie },
    });
    report.includes(
      activityAfterRequest.text,
      firstEvent?.id ?? '__missing_first_session__',
      'playground state slice',
      'shows the first request in Activity',
    );
    report.includes(
      activityAfterRequest.text,
      secondEvent?.id ?? '__missing_second_session__',
      'playground state slice',
      'shows the repeated request in Activity',
    );
    report.includes(
      activityAfterRequest.text,
      '<span class="metric-label">Final receipts</span><span class="metric-state">Current gateway run</span></div><div class="metric-value">4</div>',
      'playground state slice',
      'updates the Activity receipt count',
    );

    if (typeof secondEvent?.id === 'string') {
      const receiptEvidence = await get(
        baseUrl,
        `/mayhem/dashboard/evidence?kind=receipt&id=${encodeURIComponent(secondEvent.id)}`,
        {
          ...requestOptions,
          headers: { Accept: 'application/json', Cookie: showcaseCookie },
        },
      );
      report.equal(receiptEvidence.status, 200, 'playground state slice', 'opens Verify for the new receipt');
      let evidencePayload = null;
      try {
        evidencePayload = JSON.parse(receiptEvidence.text);
      } catch (error) {
        report.check(false, 'playground state slice', 'returns valid receipt evidence JSON', error.message);
      }
      report.equal(
        evidencePayload?.raw?.receipt?.session_id,
        secondEvent.id,
        'playground state slice',
        'Verify returns the exact new session receipt',
      );
      report.equal(
        evidencePayload?.raw?.receipt?.model_id,
        showcaseModel,
        'playground state slice',
        'Verify preserves the selected model',
      );
      report.equal(
        evidencePayload?.raw?.receipt?.final,
        true,
        'playground state slice',
        'records a final receipt for completed fixture work',
      );
    }

    const blockedScenarios = [
      ['auth-required', 401, 'fixture_credential_required', showcaseModel],
      ['empty', 503, 'fixture_catalog_unavailable', showcaseModel],
      ['loading', 503, 'fixture_route_preparing', showcaseModel],
      ['failure', 503, 'fixture_provider_failure', showcaseModel],
      ['offline', 503, 'fixture_no_fresh_route', showcaseModel],
      ['update-required', 426, 'fixture_update_required', 'workbench/catalog-next'],
    ];
    for (const [scenario, expectedStatus, expectedCode, requestedModel] of blockedScenarios) {
      const response = await postJson(
        baseUrl,
        '/v1/chat/completions',
        {
          model: requestedModel,
          messages: [{ role: 'user', content: 'this fixture must not falsely succeed' }],
          stream: true,
          stream_options: { include_usage: true },
        },
        {
          ...requestOptions,
          headers: { Cookie: `mayhem_dashboard_workbench_scenario=${scenario}` },
        },
      );
      report.equal(response.status, expectedStatus, `playground ${scenario}`, 'returns the truthful failure status');
      report.check(
        headerValue(response.headers, 'content-type').startsWith('application/json'),
        `playground ${scenario}`,
        'returns a JSON error instead of a false SSE success',
        `received ${JSON.stringify(headerValue(response.headers, 'content-type'))}`,
      );
      let errorPayload = null;
      try {
        errorPayload = JSON.parse(response.text);
      } catch (error) {
        report.check(false, `playground ${scenario}`, 'returns valid error JSON', error.message);
      }
      report.equal(errorPayload?.error?.code, expectedCode, `playground ${scenario}`, 'names the fixture blocker');
      report.equal(errorPayload?.error?.scenario, scenario, `playground ${scenario}`, 'reports the selected scenario');
      report.check(!response.text.includes('data:'), `playground ${scenario}`, 'does not emit a completion event');
    }

    const offlineActivity = await get(baseUrl, '/mayhem/dashboard/activity', {
      ...requestOptions,
      headers: { Cookie: 'mayhem_dashboard_workbench_scenario=offline' },
    });
    report.includes(
      offlineActivity.text,
      '<span class="metric-label">Final receipts</span><span class="metric-state">Current gateway run</span></div><div class="metric-value">0</div>',
      'playground offline',
      'does not manufacture receipt evidence for failed work',
    );
  }

  const notFound = await get(
    baseUrl,
    '/mayhem/dashboard/not-a-product-page?scenario=showcase',
    requestOptions,
  );
  const notFoundScope = 'unknown product route';
  report.equal(notFound.status, 404, notFoundScope, 'returns HTTP 404');
  report.check(
    headerValue(notFound.headers, 'content-type').startsWith('text/html'),
    notFoundScope,
    'serves an HTML recovery page',
    `received ${JSON.stringify(headerValue(notFound.headers, 'content-type'))}`,
  );
  report.equal(headerValue(notFound.headers, 'cache-control'), 'no-store', notFoundScope, 'disables caching');
  report.includes(
    headerValue(notFound.headers, 'content-security-policy'),
    "frame-ancestors 'none'",
    notFoundScope,
    'retains the dashboard CSP',
  );
  report.equal(headerValue(notFound.headers, 'x-frame-options'), 'DENY', notFoundScope, 'forbids framing');
  report.includes(notFound.text, '<title>Mayhem Workbench page not found</title>', notFoundScope, 'uses a clear document title');
  report.includes(notFound.text, '<h1>Workbench page not found</h1>', notFoundScope, 'names the missing route');
  report.equal(
    (notFound.text.match(/<h1(?:\s[^>]*)?>/g) ?? []).length,
    1,
    notFoundScope,
    'renders one page-level heading',
  );

  const unsupportedDisputes = await get(
    baseUrl,
    '/mayhem/dashboard/earn/disputes?scenario=showcase',
    requestOptions,
  );
  report.equal(
    unsupportedDisputes.status,
    404,
    'unsupported disputes surface',
    'does not ship an invented workflow without a data source',
  );
  report.includes(notFound.text, 'href="/"', notFoundScope, 'offers a return to the preview index');

  return { passed: report.finish(routeCount), routeCount };
}

async function main() {
  const options = parseArgs(process.argv.slice(2));
  let workbench = null;
  let baseUrl;

  try {
    if (options.baseUrl !== null) {
      baseUrl = normalizeBaseUrl(options.baseUrl);
      console.log(`[smoke] using running workbench at ${baseUrl}`);
    } else {
      if (options.build) await runCargoBuild();
      const port = await availableLoopbackPort();
      baseUrl = `http://127.0.0.1:${port}`;
      workbench = launchWorkbench(port);
      console.log(`[smoke] started isolated workbench at ${baseUrl}`);
    }

    await waitForWorkbench(
      baseUrl,
      workbench?.child ?? null,
      workbench?.logs ?? [],
      options.timeoutMs,
    );
    const result = await exerciseWorkbench(baseUrl, options);
    if (!result.passed) process.exitCode = 1;
  } finally {
    await stopWorkbench(workbench?.child ?? null);
  }
}

main().catch((error) => {
  console.error(`[smoke] fatal: ${error.stack ?? error.message}`);
  process.exitCode = 1;
});

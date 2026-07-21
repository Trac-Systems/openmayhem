#!/usr/bin/env node
import assert from 'node:assert/strict';
import { createHash as createNodeHash, createHmac, randomBytes } from 'node:crypto';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import MayhemContract, {
  SESSION_RECEIPT_SCHEMA_VERSION,
  stripePayoutProcessorRevision,
} from '../intercom/contract/contract.js';
import { opaqueHash, recomputeEpoch } from '../intercom/scripts/recompute-epoch-roots.mjs';
import { createHash, jsonStringify } from '../intercom/trac/trac-peer/src/utils/types.js';
import {
  MemoryStorage,
  execute,
  executeEpochApplyFeature,
  executeFeature,
  seedSpendHoldsForApply,
} from '../intercom/tests/helpers/contract.js';

const SCRIPT_DIR = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(SCRIPT_DIR, '..');
const DEFAULT_STRIPE_ENV_FILE = path.resolve(
  '.mayhem-local',
  'secrets',
  'stripe-test.env'
);
const DEFAULT_MAYHEM_BIN = path.join(
  REPO_ROOT,
  'target',
  'debug',
  process.platform === 'win32' ? 'mayhem.exe' : 'mayhem'
);
const DEFAULT_PAYGATE_BIN = path.join(
  REPO_ROOT,
  'target',
  'debug',
  process.platform === 'win32' ? 'mayhem-paygate.exe' : 'mayhem-paygate'
);
const DEFAULT_AMOUNT_MINOR = 100;
const DEFAULT_FEE_BPS = 1_500;
const EPOCH_SECONDS = 3_600;
const AU_PER_USD_MINOR = 10_000_000_000_000_000n;
const ZERO_HEX = '0'.repeat(64);
const STRIPE_CHECKOUT_API_VERSION = '2025-03-31.basil';
const STRIPE_FX_API_VERSION = '2025-07-30.preview';
const CONNECT_READY_TIMEOUT_MS = 180_000;
const STRIPE_EVENT_TIMEOUT_MS = 60_000;
const STRIPE_BALANCE_TIMEOUT_MS = 60_000;
const COUNTRY_CURRENCY = Object.freeze({ US: 'usd', DE: 'eur', GB: 'gbp' });
const CURRENCY_COUNTRY = Object.freeze({ usd: 'US', eur: 'DE', gbp: 'GB' });

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const usage = () => `Usage: node scripts/stripe-sandbox-payout-smoke.mjs [options]

Runs the complete Stripe test-mode economic path without browser interaction:
Connect onboarding URL -> API-completed test account -> checkout URL -> confirmed
USD deposit -> metered fiat session -> destination-currency FX quote -> provider
transfer -> destination payment -> ledger settlement evidence. It deliberately
takes the admin appender offline after Stripe creates the transfer, then
cold-restores the index and proves recovery creates no duplicate Stripe effect.

Options:
  --stripe-env-file <path>  Test STRIPE_SECRET_KEY + STRIPE_WEBHOOK_SECRET
                            (default: ${DEFAULT_STRIPE_ENV_FILE})
  --provider <64-hex>       Provider identity (default: random)
  --country <US|DE|GB>      Connected-account country (default: DE)
  --destination-currency <usd|eur|gbp>
                            Provider currency (default follows country)
  --amount-minor <integer>  Canonical USD deposit/session minor units
                            (default: ${DEFAULT_AMOUNT_MINOR})
  --fee-bps <integer>       Operator fee, maximum 1500 (default: ${DEFAULT_FEE_BPS})
  --business-url <url>      Test connected-account business URL
  --mayhem-bin <path>       Current mayhem binary (default: ${DEFAULT_MAYHEM_BIN})
  --paygate-bin <path>      Current mayhem-paygate binary (default: ${DEFAULT_PAYGATE_BIN})
  --paygate-port <port>     Local paygate port (default: auto)
  --contract-port <port>    Local contract RPC port (default: auto)
  --keep-temp               Keep ignored smoke artifacts
  --json                    Emit compact JSON
  --help                    Show this help
`;

function parseArgs(argv) {
  const args = {
    stripeEnvFile: DEFAULT_STRIPE_ENV_FILE,
    provider: randomBytes(32).toString('hex'),
    country: null,
    destinationCurrency: null,
    amountMinor: DEFAULT_AMOUNT_MINOR,
    feeBps: DEFAULT_FEE_BPS,
    businessUrl: 'https://trac.network',
    mayhemBin: DEFAULT_MAYHEM_BIN,
    paygateBin: DEFAULT_PAYGATE_BIN,
    paygatePort: null,
    contractPort: null,
    keepTemp: false,
    json: false,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    const next = () => {
      index += 1;
      if (index >= argv.length) throw new Error(`${arg} requires a value`);
      return argv[index];
    };
    if (arg === '--stripe-env-file') args.stripeEnvFile = next();
    else if (arg === '--provider') args.provider = next().trim().toLowerCase();
    else if (arg === '--country') args.country = next().trim().toUpperCase();
    else if (arg === '--destination-currency') {
      args.destinationCurrency = next().trim().toLowerCase();
    } else if (arg === '--amount-minor') args.amountMinor = Number.parseInt(next(), 10);
    else if (arg === '--fee-bps') args.feeBps = Number.parseInt(next(), 10);
    else if (arg === '--business-url') args.businessUrl = next();
    else if (arg === '--mayhem-bin') args.mayhemBin = next();
    else if (arg === '--paygate-bin') args.paygateBin = next();
    else if (arg === '--paygate-port') args.paygatePort = Number.parseInt(next(), 10);
    else if (arg === '--contract-port') args.contractPort = Number.parseInt(next(), 10);
    else if (arg === '--keep-temp') args.keepTemp = true;
    else if (arg === '--json') args.json = true;
    else if (arg === '--help' || arg === '-h') {
      process.stdout.write(usage());
      process.exit(0);
    } else {
      throw new Error(`unknown argument: ${arg}`);
    }
  }

  if (!/^[0-9a-f]{64}$/.test(args.provider)) {
    throw new Error('--provider must be 64 lowercase hexadecimal characters');
  }
  if (args.country !== null && !(args.country in COUNTRY_CURRENCY)) {
    throw new Error('--country must be US, DE, or GB');
  }
  if (
    args.destinationCurrency !== null
    && !(args.destinationCurrency in CURRENCY_COUNTRY)
  ) {
    throw new Error('--destination-currency must be usd, eur, or gbp');
  }
  if (args.country === null && args.destinationCurrency === null) {
    args.country = 'DE';
    args.destinationCurrency = 'eur';
  } else if (args.country === null) {
    args.country = CURRENCY_COUNTRY[args.destinationCurrency];
  } else if (args.destinationCurrency === null) {
    args.destinationCurrency = COUNTRY_CURRENCY[args.country];
  }
  if (COUNTRY_CURRENCY[args.country] !== args.destinationCurrency) {
    throw new Error(
      '--destination-currency must match the test connected account default currency '
      + '(US=usd, DE=eur, GB=gbp)'
    );
  }
  if (!Number.isSafeInteger(args.amountMinor) || args.amountMinor < 100) {
    throw new Error('--amount-minor must be a safe integer >= 100');
  }
  if (!Number.isSafeInteger(args.feeBps) || args.feeBps < 0 || args.feeBps > 1_500) {
    throw new Error('--fee-bps must be an integer from 0 to 1500');
  }
  const businessUrl = new URL(args.businessUrl);
  if (!['http:', 'https:'].includes(businessUrl.protocol)) {
    throw new Error('--business-url must be an http(s) URL');
  }
  for (const [label, port] of [['--paygate-port', args.paygatePort], ['--contract-port', args.contractPort]]) {
    if (port !== null && (!Number.isInteger(port) || port < 1 || port > 65_535)) {
      throw new Error(`${label} must be a TCP port`);
    }
  }
  args.stripeEnvFile = path.resolve(args.stripeEnvFile);
  args.mayhemBin = path.resolve(args.mayhemBin);
  args.paygateBin = path.resolve(args.paygateBin);
  return args;
}

function stripQuotes(value) {
  const trimmed = value.trim();
  if (
    (trimmed.startsWith('"') && trimmed.endsWith('"'))
    || (trimmed.startsWith("'") && trimmed.endsWith("'"))
  ) {
    return trimmed.slice(1, -1);
  }
  return trimmed;
}

async function readEnvFile(file) {
  if (!existsSync(file)) throw new Error(`Stripe test env file does not exist: ${file}`);
  const values = {};
  for (const rawLine of (await readFile(file, 'utf8')).split(/\r?\n/)) {
    let line = rawLine.trim();
    if (!line || line.startsWith('#')) continue;
    if (line.startsWith('export ')) line = line.slice('export '.length).trim();
    const match = line.match(/^([A-Za-z_][A-Za-z0-9_]*)\s*(?:=|:)\s*(.*)$/);
    if (match) values[match[1]] = stripQuotes(match[2]);
  }
  return values;
}

async function loadStripeSecrets(file) {
  const values = await readEnvFile(file);
  const secretKey = process.env.STRIPE_SECRET_KEY
    || process.env.MAYHEM_STRIPE_SECRET_KEY
    || values.STRIPE_SECRET_KEY
    || values.MAYHEM_STRIPE_SECRET_KEY;
  const webhookSecret = process.env.STRIPE_WEBHOOK_SECRET
    || process.env.MAYHEM_STRIPE_WEBHOOK_SECRET
    || values.STRIPE_WEBHOOK_SECRET
    || values.MAYHEM_STRIPE_WEBHOOK_SECRET;
  if (!secretKey?.startsWith('sk_test_')) {
    throw new Error('FO.3 refuses to run without a Stripe test-mode secret key');
  }
  if (!webhookSecret?.startsWith('whsec_')) {
    throw new Error('FO.3 requires a Stripe test webhook secret');
  }
  return { secretKey, webhookSecret };
}

function redactStripeCredentials(input) {
  return String(input).replace(
    /(?:sk|rk|pk)_(?:test|live)_[A-Za-z0-9_.*-]+|whsec_[A-Za-z0-9_.*-]+|ac_[A-Za-z0-9_.*-]+/g,
    '[REDACTED]'
  );
}

async function freePort() {
  const server = createServer();
  server.unref();
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  const { port } = server.address();
  await new Promise((resolve, reject) => server.close((error) => (error ? reject(error) : resolve())));
  return port;
}

function sendJson(res, status, value) {
  const body = `${JSON.stringify(value)}\n`;
  res.writeHead(status, {
    'content-type': 'application/json',
    'content-length': Buffer.byteLength(body),
  });
  res.end(body);
}

async function readJson(req) {
  const chunks = [];
  for await (const chunk of req) chunks.push(chunk);
  return chunks.length === 0 ? {} : JSON.parse(Buffer.concat(chunks).toString('utf8'));
}

function makeContractHarness({ storage = new MemoryStorage(), admin = null } = {}) {
  const contract = new MayhemContract({ peer: { wallet: { verify: () => false } } }, {});
  let txCount = 0;
  let featureOnline = true;
  const featureAttempts = [];

  const server = createServer(async (req, res) => {
    try {
      const url = new URL(req.url, 'http://127.0.0.1');
      if (req.method === 'GET' && url.pathname === '/v1/contract/nonce') {
        return sendJson(res, 200, { nonce: randomBytes(32).toString('hex') });
      }
      if (req.method === 'POST' && url.pathname === '/v1/contract/tx/prepare') {
        const body = await readJson(req);
        const prepared = body.prepared_command;
        if (!prepared || typeof prepared !== 'object') throw new Error('prepared_command missing');
        const commandJson = jsonStringify(prepared);
        if (commandJson == null) throw new Error('prepared_command is not stable-json serializable');
        const commandHash = await createHash(commandJson);
        const tx = await createHash(`${body.address ?? ''}${commandHash}${body.nonce ?? ''}${txCount}`);
        return sendJson(res, 200, { tx, command_hash: commandHash });
      }
      if (req.method === 'POST' && url.pathname === '/v1/contract/tx') {
        const body = await readJson(req);
        const prepared = body.prepared_command;
        const tx = String(body.tx ?? '').toLowerCase();
        const address = String(body.address ?? '').toLowerCase();
        if (!/^[0-9a-f]{64}$/.test(tx)) throw new Error('tx must be 64 hex characters');
        if (!/^[0-9a-f]{64}$/.test(address)) throw new Error('address must be 64 hex characters');
        if (!prepared || typeof prepared !== 'object') throw new Error('prepared_command missing');
        const previousLog = console.log;
        let result;
        try {
          console.log = () => {};
          result = await contract.execute({
            type: 'tx',
            key: tx,
            value: {
              dispatch: { type: prepared.type, value: prepared.value },
              ipk: address,
              wp: ZERO_HEX,
            },
          }, storage);
        } finally {
          console.log = previousLog;
        }
        txCount += 1;
        const normalized = result instanceof Error
          ? { ok: false, message: result.message }
          : result;
        return sendJson(res, 200, { result: normalized });
      }
      if (req.method === 'POST' && url.pathname === '/v1/contract/feature') {
        const body = await readJson(req);
        featureAttempts.push(body);
        if (!featureOnline) {
          return sendJson(res, 503, { error: 'admin appender intentionally offline' });
        }
        if (!admin) throw new Error('contract harness admin is not configured');
        contract._mayhemLastFeatureResult = undefined;
        const previousLog = console.log;
        let direct;
        try {
          console.log = () => {};
          direct = await executeFeature(
            contract,
            storage,
            'mayhem_feature',
            body.key,
            body.value,
            admin
          );
        } finally {
          console.log = previousLog;
        }
        const result = direct ?? contract._mayhemLastFeatureResult;
        const featureHash = await createHash(
          `${body.key ?? ''}:${jsonStringify(body.value ?? null) ?? ''}`
        );
        if (result instanceof Error) {
          return sendJson(res, 200, {
            ok: false,
            accepted: true,
            status: 'rejected',
            feature: 'mayhem',
            key: body.key,
            hash: featureHash,
            message: result.message,
            result_key: `fr/${featureHash}`,
            result: {
              type: 'feature_result',
              feature_key: `mayhem_${body.key}`,
              hash: featureHash,
              status: 'rejected',
              ok: false,
              result: null,
              error: { name: result.name, message: result.message },
            },
          });
        }
        return sendJson(res, 200, {
          ok: result?.ok === true,
          accepted: true,
          status: 'applied',
          feature: 'mayhem',
          key: body.key,
          hash: featureHash,
          message: result?.ok === true ? 'Feature applied.' : 'Feature rejected.',
          result_key: `fr/${featureHash}`,
          result: {
            type: 'feature_result',
            feature_key: `mayhem_${body.key}`,
            hash: featureHash,
            status: result?.ok === true ? 'applied' : 'rejected',
            ok: result?.ok === true,
            result,
            error: result?.ok === true ? null : { name: 'Error', message: 'Feature rejected.' },
          },
        });
      }
      if (req.method === 'GET' && url.pathname === '/v1/state') {
        const prefix = url.searchParams.get('prefix');
        if (prefix !== null) {
          const limit = Number.parseInt(url.searchParams.get('limit') ?? '1000', 10);
          const values = Array.from(storage.values.entries())
            .filter(([key]) => key.startsWith(prefix))
            .sort(([left], [right]) => left.localeCompare(right))
            .slice(0, limit)
            .map(([key, value]) => ({ key, value }));
          return sendJson(res, 200, { values });
        }
        const key = url.searchParams.get('key');
        if (!key) return sendJson(res, 400, { error: 'missing key or prefix' });
        const entry = await storage.get(key);
        return sendJson(res, 200, { key, value: entry?.value ?? null });
      }
      return sendJson(res, 404, { error: 'not found' });
    } catch (error) {
      return sendJson(res, 500, { error: error.message });
    }
  });

  return {
    storage,
    contract,
    featureAttempts,
    setAdmin(value) {
      admin = value;
    },
    setFeatureOnline(value) {
      featureOnline = value;
    },
    async listen(port) {
      await new Promise((resolve, reject) => {
        server.once('error', reject);
        server.listen(port, '127.0.0.1', resolve);
      });
      return server.address().port;
    },
    async close() {
      if (!server.listening) return;
      await new Promise((resolve, reject) => server.close((error) => (error ? reject(error) : resolve())));
    },
  };
}

function tomlString(value) {
  return String(value).replace(/\\/g, '\\\\').replace(/"/g, '\\"');
}

async function writePaygateConfig({
  file,
  bind,
  contractRpc,
  eventStore,
  accountStore,
  consentStore,
  internalAuthSecret,
  oracleKey,
}) {
  await writeFile(file, `[server]
bind = "${tomlString(bind)}"

[contract]
rpc_url = "${tomlString(contractRpc)}"
dry_run = false
epoch_seconds = ${EPOCH_SECONDS}

[oracle]
key_path = "${tomlString(oracleKey)}"

[stripe]
enabled = true
mode = "test"
event_store_path = "${tomlString(eventStore)}"
webhook_tolerance_seconds = 300
backfill_enabled = false
connect_account_type = "custom"
connect_accounts_path = "${tomlString(accountStore)}"
connect_consents_path = "${tomlString(consentStore)}"
connect_return_url = "https://dashboard.stripe.com/"
connect_refresh_url = "https://dashboard.stripe.com/"
internal_auth_secret_path = "${tomlString(internalAuthSecret)}"
`, { mode: 0o600 });
}

function spawnPaygate(binary, configPath, secrets) {
  if (!existsSync(binary)) throw new Error(`mayhem-paygate binary does not exist: ${binary}`);
  const child = spawn(binary, ['--config', configPath], {
    cwd: REPO_ROOT,
    env: {
      ...process.env,
      MAYHEM_STRIPE_MODE: 'test',
      MAYHEM_STRIPE_SECRET_KEY: secrets.secretKey,
      MAYHEM_STRIPE_WEBHOOK_SECRET: secrets.webhookSecret,
    },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  const logs = [];
  const collect = (chunk) => {
    logs.push(chunk.toString('utf8'));
    while (logs.join('').length > 30_000) logs.shift();
  };
  child.stdout.on('data', collect);
  child.stderr.on('data', collect);
  return {
    child,
    logs: () => logs.join(''),
    async stop() {
      if (child.exitCode !== null || child.signalCode !== null) return;
      child.kill('SIGTERM');
      await Promise.race([
        new Promise((resolve) => child.once('exit', resolve)),
        sleep(3_000).then(() => {
          if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
        }),
      ]);
    },
  };
}

async function waitForHealth(baseUrl, paygate) {
  const deadline = Date.now() + 60_000;
  let lastError = null;
  while (Date.now() < deadline) {
    if (paygate.child.exitCode !== null) {
      throw new Error(`paygate exited early:\n${paygate.logs()}`);
    }
    try {
      const response = await fetch(`${baseUrl}/v1/health`);
      if (response.ok) return await response.json();
      lastError = new Error(`health returned ${response.status}`);
    } catch (error) {
      lastError = error;
    }
    await sleep(250);
  }
  throw new Error(`paygate health timeout: ${lastError?.message ?? 'unknown'}\n${paygate.logs()}`);
}

function paygateInternalAuthHeaders(secret, method, pathname, body) {
  const timestamp = Math.floor(Date.now() / 1_000);
  const nonce = randomBytes(32).toString('hex');
  const bodyHash = createNodeHash('sha256').update(body).digest('hex');
  const message = [
    'mayhem-paygate-internal-request-v1',
    timestamp,
    nonce,
    method,
    pathname,
    bodyHash,
  ].join('\n');
  return {
    'x-mayhem-paygate-timestamp': String(timestamp),
    'x-mayhem-paygate-nonce': nonce,
    'x-mayhem-paygate-signature': createHmac('sha256', secret)
      .update(message)
      .digest('hex'),
  };
}

async function requestJson(
  url,
  { method = 'GET', body = null, headers = {}, internalAuthSecret = null } = {}
) {
  const bodyText = body === null
    ? null
    : (typeof body === 'string' ? body : JSON.stringify(body));
  const target = new URL(url);
  const authHeaders = internalAuthSecret === null
    ? {}
    : paygateInternalAuthHeaders(
        internalAuthSecret,
        method,
        target.pathname,
        bodyText ?? ''
      );
  const response = await fetch(url, {
    method,
    headers: {
      ...(body === null ? {} : { 'content-type': 'application/json' }),
      ...headers,
      ...authHeaders,
    },
    ...(bodyText === null ? {} : { body: bodyText }),
  });
  const text = await response.text();
  let value;
  try {
    value = JSON.parse(text);
  } catch {
    value = { raw: text };
  }
  if (!response.ok) {
    throw new Error(`${method} ${url} returned ${response.status}: ${JSON.stringify(value)}`);
  }
  return value;
}

async function stripeRequest(
  secretKey,
  method,
  endpoint,
  params = null,
  {
    idempotencyKey = null,
    stripeAccount = null,
    apiVersion = null,
  } = {}
) {
  const url = new URL(`https://api.stripe.com/v1/${endpoint}`);
  const headers = { authorization: `Bearer ${secretKey}` };
  const init = { method, headers };
  if (idempotencyKey) headers['idempotency-key'] = idempotencyKey;
  if (stripeAccount) headers['stripe-account'] = stripeAccount;
  if (apiVersion) headers['stripe-version'] = apiVersion;
  if (method === 'GET' && params) {
    for (const [key, value] of Object.entries(params)) url.searchParams.append(key, String(value));
  } else if (params) {
    headers['content-type'] = 'application/x-www-form-urlencoded';
    init.body = new URLSearchParams(params);
  }
  const response = await fetch(url, init);
  const text = await response.text();
  let value;
  try {
    value = JSON.parse(text);
  } catch {
    value = { raw: text };
  }
  if (!response.ok) {
    const error = new Error(redactStripeCredentials(
      value.error?.message ?? `${endpoint} returned ${response.status}`
    ));
    error.code = value.error?.code;
    error.type = value.error?.type;
    error.status = response.status;
    throw error;
  }
  return value;
}

const stripeGet = (secret, endpoint, params = null, options = {}) => (
  stripeRequest(secret, 'GET', endpoint, params, options)
);
const stripePost = (secret, endpoint, params, idempotencyKey = null, options = {}) => (
  stripeRequest(secret, 'POST', endpoint, params, { ...options, idempotencyKey })
);

async function completeCustomAccount(secretKey, account, args, tag) {
  if (
    account.details_submitted === true
    && account.payouts_enabled === true
    && account.capabilities?.transfers === 'active'
  ) {
    return account;
  }
  const now = Math.floor(Date.now() / 1_000);
  const common = {
    email: `mayhem-${tag}@example.com`,
    business_type: 'individual',
    'business_profile[mcc]': '5734',
    'business_profile[url]': args.businessUrl,
    'individual[first_name]': 'Mayhem',
    'individual[last_name]': 'Provider',
    'individual[email]': `provider-${tag}@example.com`,
    'individual[dob][day]': '1',
    'individual[dob][month]': '1',
    'individual[dob][year]': '1901',
    'tos_acceptance[date]': String(now),
    'tos_acceptance[ip]': '127.0.0.1',
    'metadata[mayhem_fo3]': tag,
  };
  const countryFields = {
    US: {
        'individual[phone]': '+14155550123',
        'individual[address][line1]': 'address_full_match',
        'individual[address][city]': 'San Francisco',
        'individual[address][state]': 'CA',
        'individual[address][postal_code]': '94107',
        'individual[address][country]': 'US',
        'individual[id_number]': '000000000',
        'individual[ssn_last_4]': '0000',
        external_account: 'btok_us_verified',
    },
    DE: {
        'individual[phone]': '+4915112345678',
        'individual[address][line1]': 'address_full_match',
        'individual[address][city]': 'Berlin',
        'individual[address][postal_code]': '10115',
        'individual[address][country]': 'DE',
        external_account: 'btok_de',
    },
    GB: {
        'individual[phone]': '+447700900123',
        'individual[address][line1]': 'address_full_match',
        'individual[address][city]': 'London',
        'individual[address][postal_code]': 'SW1A 1AA',
        'individual[address][country]': 'GB',
        external_account: 'btok_gb',
    },
  }[args.country];
  assert.ok(countryFields, `unsupported Stripe test country ${args.country}`);
  await stripePost(secretKey, `accounts/${account.id}`, { ...common, ...countryFields });

  const deadline = Date.now() + CONNECT_READY_TIMEOUT_MS;
  let current = null;
  while (Date.now() < deadline) {
    current = await stripeGet(secretKey, `accounts/${account.id}`);
    if (
      current.details_submitted === true
      && current.payouts_enabled === true
      && current.capabilities?.transfers === 'active'
    ) {
      return current;
    }
    await sleep(1_000);
  }
  throw new Error(
    `connected account did not become ready: ${JSON.stringify({
      payouts_enabled: current?.payouts_enabled,
      transfers: current?.capabilities?.transfers,
      currently_due: current?.requirements?.currently_due,
      disabled_reason: current?.requirements?.disabled_reason,
    })}`
  );
}

async function ensureAvailableBalance(secretKey, currency, requiredMinor, tag) {
  const readAvailable = async () => {
    const balance = await stripeGet(secretKey, 'balance');
    return Number(balance.available?.find((entry) => entry.currency === currency)?.amount ?? 0);
  };
  const before = await readAvailable();
  if (before >= requiredMinor) return { before, after: before, topup_id: null };
  const topupAmount = Math.max(requiredMinor - before, 100);
  const source = {
    usd: 'btok_us_verified',
    eur: 'btok_de',
    gbp: 'btok_gb',
  }[currency];
  if (!source) {
    throw new Error(`Stripe smoke cannot fund platform source currency ${currency}`);
  }
  const topup = await stripePost(
    secretKey,
    'topups',
    {
      amount: String(topupAmount),
      currency,
      source,
      description: 'Mayhem FO.3 settlement smoke',
      'metadata[mayhem_fo3]': tag,
    },
    `mayhem-fo3-topup-${tag}`
  );
  const deadline = Date.now() + STRIPE_BALANCE_TIMEOUT_MS;
  let after = before;
  while (Date.now() < deadline) {
    after = await readAvailable();
    if (after >= requiredMinor) return { before, after, topup_id: topup.id };
    const current = await stripeGet(secretKey, `topups/${topup.id}`);
    if (current.status === 'failed' || current.status === 'canceled') {
      throw new Error(`Stripe test top-up ${current.status}: ${current.failure_code ?? 'unknown'}`);
    }
    await sleep(500);
  }
  throw new Error(`Stripe test balance did not become available after top-up ${topup.id}`);
}

async function readyUsdConnectAccount(secretKey, args, tag) {
  let startingAfter = null;
  for (let page = 0; page < 100; page += 1) {
    const listed = await stripeGet(secretKey, 'accounts', {
      limit: 100,
      ...(startingAfter ? { starting_after: startingAfter } : {}),
    });
    const ready = listed.data?.find((account) => (
      account.country === 'US'
      && account.default_currency === 'usd'
      && account.details_submitted === true
      && account.charges_enabled === true
      && account.capabilities?.card_payments === 'active'
      && account.capabilities?.transfers === 'active'
    ));
    if (ready) return ready;
    if (listed.has_more !== true) break;
    startingAfter = listed.data?.at(-1)?.id;
    if (!startingAfter) throw new Error('Stripe Connect account pagination did not advance');
  }
  let account = await stripePost(
    secretKey,
    'accounts',
    {
      type: 'custom',
      country: 'US',
      'capabilities[card_payments][requested]': 'true',
      'capabilities[transfers][requested]': 'true',
      'metadata[mayhem_smoke_role]': 'usd-transfer-balance',
    },
    `mayhem-fo3-usd-funding-account-${tag}`
  );
  account = await completeCustomAccount(
    secretKey,
    account,
    { ...args, country: 'US' },
    `${tag}-usd-funding`
  );
  const deadline = Date.now() + CONNECT_READY_TIMEOUT_MS;
  while (Date.now() < deadline) {
    account = await stripeGet(secretKey, `accounts/${account.id}`);
    if (
      account.country === 'US'
      && account.default_currency === 'usd'
      && account.details_submitted === true
      && account.charges_enabled === true
      && account.capabilities?.card_payments === 'active'
      && account.capabilities?.transfers === 'active'
    ) {
      return account;
    }
    await sleep(1_000);
  }
  throw new Error('Stripe sandbox USD funding account did not become charge-ready');
}

async function ensureUsdTransferBalance(secretKey, requiredMinor, args, tag) {
  const readAvailable = async () => {
    const balance = await stripeGet(secretKey, 'balance');
    return Number(balance.available?.find((entry) => entry.currency === 'usd')?.amount ?? 0);
  };
  const before = await readAvailable();
  if (before >= requiredMinor) {
    return { before, after: before, funding_charge_id: null };
  }
  const fundingAccount = await readyUsdConnectAccount(secretKey, args, tag);
  const deficit = requiredMinor - before;
  const fundingAmount = Math.max(deficit + Math.ceil(deficit * 0.05) + 100, 100);
  const fundingCharge = await stripePost(
    secretKey,
    'charges',
    {
      amount: String(fundingAmount),
      currency: 'usd',
      source: 'tok_bypassPending',
      on_behalf_of: fundingAccount.id,
      description: 'Mayhem Stripe payout smoke USD balance preparation',
      'metadata[mayhem_fo3]': tag,
    },
    `mayhem-fo3-usd-funding-${tag}`
  );
  assert.equal(fundingCharge.currency, 'usd');
  assert.equal(fundingCharge.amount, fundingAmount);
  assert.equal(fundingCharge.paid, true);
  assert.equal(fundingCharge.captured, true);
  const fundingBalanceTransaction = await stripeGet(
    secretKey,
    `balance_transactions/${fundingCharge.balance_transaction}`
  );
  assert.equal(fundingBalanceTransaction.currency, 'usd');
  assert.equal(fundingBalanceTransaction.status, 'available');

  const deadline = Date.now() + STRIPE_BALANCE_TIMEOUT_MS;
  let after = before;
  while (Date.now() < deadline) {
    after = await readAvailable();
    if (after >= requiredMinor) {
      return { before, after, funding_charge_id: fundingCharge.id };
    }
    await sleep(500);
  }
  throw new Error(
    `Stripe USD balance did not become available after funding charge ${fundingCharge.id}`
  );
}

async function confirmPaymentIntent(secretKey, intentId, tag) {
  return stripePost(
    secretKey,
    `payment_intents/${intentId}/confirm`,
    {
      payment_method: 'pm_card_visa',
      return_url: 'https://example.com/mayhem/stripe/return',
    },
    `mayhem-fo3-confirm-${tag}`
  );
}

async function findPaymentIntentEvent(secretKey, intentId, createdAt) {
  const deadline = Date.now() + STRIPE_EVENT_TIMEOUT_MS;
  while (Date.now() < deadline) {
    const events = await stripeGet(secretKey, 'events', {
      type: 'payment_intent.succeeded',
      'created[gte]': createdAt - 5,
      limit: 100,
    });
    const event = events.data?.find((entry) => entry.data?.object?.id === intentId);
    if (event) return event;
    await sleep(500);
  }
  throw new Error(`Stripe payment_intent.succeeded event did not appear for ${intentId}`);
}

function stripeSignatureHeader(secret, payload, timestamp) {
  const signature = createHmac('sha256', secret)
    .update(`${timestamp}.${payload}`)
    .digest('hex');
  return `t=${timestamp},v1=${signature}`;
}

async function seedParam(storage, key, value) {
  await storage.put(`params/${key}`, {
    key,
    current: {
      value,
      ver: 1,
      submitted_at: 0,
      effective_at: 0,
      set_by_role: 'admin',
      set_at: 'stripe-sandbox-payout-smoke',
    },
    pending: null,
  });
}

async function seedSettlementState({ harness, admin, provider, account, args, user, depositRoot, epoch, at }) {
  await harness.storage.put('admin', admin);
  await harness.storage.put('epoch/apply/state', {
    updated_epoch: epoch - 1,
    updated_at: null,
    last_apply_hash: null,
  });
  for (const [key, value] of Object.entries({
    epoch_seconds: EPOCH_SECONDS,
    fee_bps: args.feeBps,
    holdback_epochs: 0,
    new_provider_holdback_epochs: 0,
    challenge_epochs: 0,
    probation_successful_sessions: 0,
    canary_probe_holdback_bps: 0,
    canary_probe_release_min_passes: 0,
  })) {
    await seedParam(harness.storage, key, value);
  }
  await harness.storage.put(`prov/${provider}`, {
    provider,
    status: 'active',
    accepted_rails: ['fiat'],
    probation: { successful_sessions: 1 },
    registered_at: 'stripe-sandbox-payout-smoke',
    updated_at: 'stripe-sandbox-payout-smoke',
  });

  const grossAu = BigInt(args.amountMinor) * AU_PER_USD_MINOR;
  const sessionId = `fo3-${randomBytes(16).toString('hex')}`;
  const receipt = {
    schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
    session_id: sessionId,
    billing_id: randomBytes(32).toString('hex'),
    billing_attempt: 0,
    billing_prior_usage: {},
    billing_prior_au_owed_cum: '0',
    seq: 1,
    final: true,
    rail: 'fiat',
    user,
    provider,
    enclave_id: randomBytes(32).toString('hex'),
    model_id: 'mayhem/fo3-stripe-settlement-smoke',
    price_ver: 1,
    locked_rate_map: [
      { unit: 'input_token', per_unit_au: grossAu.toString(), granularity: 1 },
      { unit: 'output_token', per_unit_au: '0', granularity: 1 },
    ],
    locked_per_req_au: '0',
    locked_min_session_au: '0',
    served_ctx: 4_096,
    ctx_bracket: 'le8k',
    ctx_bracket_table_ver: 1,
    rules_ver: 1,
    usage: { input_token: 1 },
    au_owed_cum: grossAu.toString(),
    prompt_hash: randomBytes(32).toString('hex'),
    ts: at,
    enclave_sig: randomBytes(64).toString('hex'),
    user_sig: randomBytes(64).toString('hex'),
  };
  const roll = await recomputeEpoch({
    epoch,
    deposit_root: depositRoot,
    params: { fee_bps: args.feeBps },
    deposits: [],
    receipts: [receipt],
    payouts: [],
    prior_earnings: {},
    prior_fee_cum_au: '0',
    prior_burn_cum_au: '0',
  });
  const applyValue = {
    op: 'epoch_apply',
    epoch,
    at,
    debits: roll.debits,
    earnings: roll.earnings,
    roots: roll.roots,
    totals: roll.totals,
  };
  await seedSpendHoldsForApply(harness.storage, applyValue);
  const previousLog = console.log;
  let payments;
  let committed;
  let applied;
  try {
    console.log = () => {};
    payments = await execute(
      harness.contract,
      harness.storage,
      'setPayments',
      {
        op: 'set_payments',
        ver: 1,
        fiat: {
          processor: 'stripe',
          integration_currency: 'usd',
          adaptive_pricing: true,
          payout_currencies: ['eur', 'gbp', 'usd'],
          locale: 'en',
        },
        tap: {
          chain_id: 1,
          token_address: `0x${'1'.repeat(40)}`,
          pool_address: `0x${'2'.repeat(40)}`,
        },
        tnk: {
          network: 'testnet1',
          treasury_address: `testtrac1${'1'.repeat(40)}`,
        },
      },
      admin,
      999_999
    );
    if (payments?.ok !== true) {
      throw new Error(`payment config failed: ${payments?.message ?? String(payments)}`);
    }
    committed = await execute(
      harness.contract,
      harness.storage,
      'epochCommit',
      {
        op: 'epoch_commit',
        epoch,
        at,
        roots: roll.roots,
        totals: roll.totals,
      },
      admin,
      1_000_000
    );
    if (committed?.ok !== true) {
      throw new Error(`epoch commit failed: ${committed?.message ?? String(committed)}`);
    }
    applied = await executeEpochApplyFeature(
      harness.contract,
      harness.storage,
      applyValue,
      admin
    );
  } finally {
    console.log = previousLog;
  }
  if (applied?.ok !== true) {
    throw new Error(`epoch apply failed: ${applied?.message ?? String(applied)}`);
  }

  const earningKey = `earn/fiat/${provider}`;
  const earning = (await harness.storage.get(earningKey))?.value;
  if (!earning) throw new Error('epoch apply did not create provider fiat earnings');
  assert.equal(String(earning.total_au), String(applied.earned_au));
  assert.equal(String(earning.paid_cum_au), '0');

  const payoutRevision = randomBytes(32).toString('hex');
  const processorRevision = await stripePayoutProcessorRevision({
    account_id: account.id,
    account_type: account.type,
    country: account.country,
    currency: account.default_currency,
    mode: 'test',
    provider,
  });
  const verificationRevision = await opaqueHash('stripe-sandbox-payout-smoke-verification-v1', {
    provider,
    account_id: account.id,
    processor_revision: processorRevision,
  });
  const verificationKey = `payout/stripe-verified/${provider}/${verificationRevision}`;
  const verification = {
    type: 'stripe_payout_verification',
    revision: verificationRevision,
    provider,
    target: account.id,
    account_type: account.type,
    country: account.country,
    currency: account.default_currency,
    mode: 'test',
    verification_kind: 'status',
    source_provider: null,
    processor_revision: processorRevision,
    previous_verification: null,
    details_submitted: account.details_submitted,
    payouts_enabled: account.payouts_enabled,
    transfers_enabled: account.capabilities?.transfers === 'active',
    ready: true,
    verified_at: verificationKey,
    verified_by: admin,
    verified_by_role: 'admin',
  };
  const verificationPointer = {
    provider,
    revision: verificationRevision,
    record_key: verificationKey,
    target: account.id,
    currency: account.default_currency,
    processor_revision: processorRevision,
    ready: true,
    details_submitted: true,
    payouts_enabled: true,
    transfers_enabled: true,
    updated_at: verificationKey,
    updated_by: admin,
    updated_by_role: 'admin',
  };
  const binding = {
    type: 'provider_payout_binding',
    verified: true,
    provider,
    rail: 'fiat',
    revision: payoutRevision,
    target: account.id,
    currency: args.destinationCurrency,
    chain_id: null,
    stripe_processor_revision: processorRevision,
    activation_epoch: epoch,
  };
  const liability = {
    ...earning,
    type: 'provider_payout_liability',
    provider,
    rail: 'fiat',
    revision: payoutRevision,
    target: account.id,
    currency: args.destinationCurrency,
    chain_id: null,
  };
  await harness.storage.put(verificationKey, verification);
  await harness.storage.put(`payout/stripe-verified/current/${provider}`, verificationPointer);
  await harness.storage.put(
    `payout/stripe-verified/target/${provider}/${account.id}`,
    verificationPointer
  );
  await harness.storage.put(`payout/binding/fiat/${provider}/${payoutRevision}`, binding);
  await harness.storage.put(`payout/liability/fiat/${provider}/${payoutRevision}`, liability);
  await harness.storage.put(`payout/current/fiat/${provider}`, {
    provider,
    rail: 'fiat',
    current_revision: payoutRevision,
    current_activation_epoch: epoch,
    pending_revision: null,
    pending_activation_epoch: null,
  });
  return {
    sessionId,
    receipt,
    roll,
    payments,
    committed,
    applied,
    payoutRevision,
    processorRevision,
    verificationRevision,
  };
}

async function runMayhem(binary, args, { expectFailure = false } = {}) {
  if (!existsSync(binary)) throw new Error(`mayhem binary does not exist: ${binary}`);
  const child = spawn(binary, args, {
    cwd: REPO_ROOT,
    env: {
      ...process.env,
      MAYHEM_NETWORK: 'development',
      MAYHEM_MSB_NETWORK: 'development',
      MAYHEM_STRIPE_MODE: 'test',
    },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let stdout = '';
  let stderr = '';
  child.stdout.on('data', (chunk) => { stdout += chunk.toString('utf8'); });
  child.stderr.on('data', (chunk) => { stderr += chunk.toString('utf8'); });
  const code = await new Promise((resolve, reject) => {
    child.once('error', reject);
    child.once('exit', resolve);
  });
  if (expectFailure) {
    if (code === 0) throw new Error('expected mayhem command to fail while appender was offline');
    return { code, stdout, stderr };
  }
  if (code !== 0) {
    throw new Error(`mayhem command failed (${code}): ${stderr || stdout}`);
  }
  let report;
  try {
    report = JSON.parse(stdout);
  } catch {
    throw new Error(`mayhem command did not emit JSON: ${stdout}\n${stderr}`);
  }
  return { code, stdout, stderr, report };
}

function settlementArgs({ args, tempDir, contractBase, epoch, at }) {
  return [
    'admin',
    'fiat-settlement',
    '--epoch',
    String(epoch),
    '--at',
    String(at),
    '--operator-stripe-account',
    'platform_balance',
    '--submit-transfer',
    '--submit',
    '--home',
    tempDir,
    '--rpc-url',
    contractBase,
    '--stripe-env-file',
    args.stripeEnvFile,
    '--json',
  ];
}

async function exactTransfers(secretKey, { epoch, applyHash, provider, destination }) {
  const transferGroup = `mayhem_fiat_epoch_${epoch}_${applyHash.slice(0, 16)}`;
  const listed = await stripeGet(secretKey, 'transfers', {
    transfer_group: transferGroup,
    destination,
    limit: 100,
  }, { apiVersion: STRIPE_FX_API_VERSION });
  return (listed.data ?? []).filter((transfer) => (
    transfer.destination === destination
    && transfer.transfer_group === transferGroup
    && transfer.metadata?.mayhem_provider === provider
    && transfer.metadata?.mayhem_epoch === String(epoch)
    && transfer.metadata?.mayhem_epoch_apply_hash === applyHash
  ));
}

function stripeObjectId(value, prefix, label) {
  const id = typeof value === 'string' ? value : value?.id;
  assert.match(id ?? '', new RegExp(`^${prefix}_[A-Za-z0-9._-]+$`), label);
  return id;
}

function stripeTimestamp(value, label) {
  const timestamp = Number.parseInt(String(value).split(/[.eE]/, 1)[0], 10);
  assert.ok(Number.isSafeInteger(timestamp) && timestamp > 0, label);
  return timestamp;
}

function stripeFxQuoteEvidence(quote) {
  assert.equal(quote.object, 'fx_quote');
  const rates = Object.fromEntries(
    Object.entries(quote.rates ?? {}).map(([currency, rate]) => {
      assert.match(currency, /^[a-z]{3}$/);
      assert.ok(rate?.exchange_rate !== undefined, `${currency} exchange_rate missing`);
      assert.ok(rate?.rate_details?.base_rate !== undefined, `${currency} base_rate missing`);
      return [currency, {
        exchange_rate: String(rate.exchange_rate),
        base_rate: String(rate.rate_details.base_rate),
      }];
    })
  );
  const usageType = quote.usage?.type;
  const usageDestination = quote.usage?.transfer?.destination
    ?? quote.usage?.payment?.destination
    ?? null;
  return {
    id: stripeObjectId(quote.id, 'fxq', 'Stripe FX quote id is invalid'),
    created: stripeTimestamp(quote.created, 'Stripe FX quote timestamp is invalid'),
    expires_at: quote.lock_expires_at == null
      ? null
      : stripeTimestamp(quote.lock_expires_at, 'Stripe FX quote expiry is invalid'),
    lock_duration: quote.lock_duration,
    lock_status: quote.lock_status,
    to_currency: quote.to_currency,
    usage: {
      type: usageType,
      destination: usageDestination,
    },
    rates,
  };
}

async function verifyStripeTransferEvidence(secretKey, {
  transfer,
  provider,
  payoutRevision,
  destination,
  destinationCurrency,
  epoch,
  applyHash,
}) {
  const transferId = stripeObjectId(transfer.id, 'tr', 'Stripe transfer id is invalid');
  const retrieved = await stripeGet(secretKey, `transfers/${transferId}`, null, {
    apiVersion: STRIPE_FX_API_VERSION,
  });
  assert.equal(retrieved.id, transferId);
  assert.equal(retrieved.destination, destination);
  assert.equal(retrieved.reversed, false);
  assert.equal(retrieved.amount_reversed, 0);
  assert.ok(Number.isSafeInteger(retrieved.amount) && retrieved.amount > 0);
  assert.match(retrieved.currency, /^[a-z]{3}$/);
  assert.equal(retrieved.metadata?.mayhem_schema, 'fiat_fx_v1');
  assert.equal(retrieved.metadata?.mayhem_provider, provider);
  assert.equal(retrieved.metadata?.mayhem_payout_revision, payoutRevision);
  assert.equal(retrieved.metadata?.mayhem_epoch, String(epoch));
  assert.equal(retrieved.metadata?.mayhem_epoch_apply_hash, applyHash);

  const convertsCurrency = retrieved.currency !== destinationCurrency;
  const directUsd = retrieved.currency === 'usd' && destinationCurrency === 'usd';
  let quote = null;
  let quoteEvidence = null;
  let quoteHash = null;
  if (directUsd) {
    assert.equal(retrieved.metadata?.mayhem_fx_quote, 'direct-usd');
    assert.equal(retrieved.fx_quote ?? null, null);
  } else {
    const quoteId = stripeObjectId(
      retrieved.metadata?.mayhem_fx_quote,
      'fxq',
      'Stripe transfer valuation quote id is invalid'
    );
    if (convertsCurrency) {
      assert.equal(
        stripeObjectId(retrieved.fx_quote, 'fxq', 'Stripe applied FX quote id is invalid'),
        quoteId
      );
    } else {
      assert.equal(retrieved.fx_quote ?? null, null);
    }
    quote = await stripeGet(secretKey, `fx_quotes/${quoteId}`, null, {
      apiVersion: STRIPE_FX_API_VERSION,
    });
    quoteEvidence = stripeFxQuoteEvidence(quote);
    assert.equal(quoteEvidence.id, quoteId);
    assert.equal(quoteEvidence.to_currency, destinationCurrency);
    assert.equal(quoteEvidence.usage.type, 'transfer');
    assert.equal(quoteEvidence.usage.destination, destination);
    assert.ok(quoteEvidence.rates.usd, 'Stripe FX quote is missing canonical USD rate');
    if (convertsCurrency) {
      assert.ok(
        quoteEvidence.rates[retrieved.currency],
        'Stripe FX quote is missing the platform source-currency rate'
      );
    }
    quoteHash = await opaqueHash('mayhem-stripe-fx-quote-evidence-v1', quoteEvidence);
  }

  const destinationPaymentId = stripeObjectId(
    retrieved.destination_payment,
    'py',
    'Stripe destination payment id is invalid'
  );
  const destinationPayment = await stripeGet(
    secretKey,
    `charges/${destinationPaymentId}`,
    null,
    {
      apiVersion: STRIPE_FX_API_VERSION,
      stripeAccount: destination,
    }
  );
  assert.equal(destinationPayment.id, destinationPaymentId);
  assert.equal(destinationPayment.source_transfer, transferId);
  assert.equal(destinationPayment.currency, retrieved.currency);
  assert.equal(destinationPayment.amount, retrieved.amount);
  assert.ok(
    Number.isSafeInteger(destinationPayment.amount) && destinationPayment.amount > 0,
    'Stripe destination payment amount is invalid'
  );
  assert.equal(destinationPayment.paid, true);
  assert.equal(destinationPayment.captured, true);

  const destinationBalanceTransactionId = stripeObjectId(
    destinationPayment.balance_transaction,
    'txn',
    'Stripe destination balance transaction id is invalid'
  );
  const destinationBalanceTransaction = await stripeGet(
    secretKey,
    `balance_transactions/${destinationBalanceTransactionId}`,
    null,
    {
      apiVersion: STRIPE_FX_API_VERSION,
      stripeAccount: destination,
    }
  );
  assert.equal(destinationBalanceTransaction.id, destinationBalanceTransactionId);
  assert.equal(destinationBalanceTransaction.object, 'balance_transaction');
  assert.equal(destinationBalanceTransaction.type, 'payment');
  assert.equal(destinationBalanceTransaction.source, destinationPaymentId);
  assert.equal(destinationBalanceTransaction.currency, destinationCurrency);
  assert.ok(
    Number.isSafeInteger(destinationBalanceTransaction.amount)
      && destinationBalanceTransaction.amount > 0,
    'Stripe destination balance amount is invalid'
  );
  assert.ok(
    Number.isSafeInteger(destinationBalanceTransaction.fee)
      && destinationBalanceTransaction.fee >= 0,
    'Stripe destination balance fee is invalid'
  );
  assert.ok(
    Number.isSafeInteger(destinationBalanceTransaction.net)
      && destinationBalanceTransaction.net >= 0,
    'Stripe destination balance net amount is invalid'
  );
  assert.equal(
    destinationBalanceTransaction.net + destinationBalanceTransaction.fee,
    destinationBalanceTransaction.amount
  );
  if (convertsCurrency) {
    assert.equal(
      String(destinationBalanceTransaction.exchange_rate),
      quoteEvidence.rates[retrieved.currency].base_rate
    );
  } else {
    assert.equal(destinationBalanceTransaction.exchange_rate ?? null, null);
  }

  return {
    transfer: retrieved,
    quote,
    quoteEvidence,
    quoteHash,
    destinationPayment,
    destinationBalanceTransaction,
  };
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const secrets = await loadStripeSecrets(args.stripeEnvFile);
  const localRoot = path.join(REPO_ROOT, '.mayhem-local');
  await mkdir(localRoot, { recursive: true });
  const tempDir = await mkdtemp(path.join(localRoot, 'stripe-sandbox-payout-smoke-'));
  const contractPort = args.contractPort ?? await freePort();
  const paygatePort = args.paygatePort ?? await freePort();
  const contractBase = `http://127.0.0.1:${contractPort}/v1`;
  const paygateBase = `http://127.0.0.1:${paygatePort}`;
  const configPath = path.join(tempDir, 'paygate.toml');
  const eventStore = path.join(tempDir, 'stripe-events.jsonl');
  const accountStore = path.join(tempDir, 'stripe-connect-accounts.jsonl');
  const consentStore = path.join(tempDir, 'stripe-connect-consents.jsonl');
  const internalAuthSecretPath = path.join(tempDir, 'internal-auth.secret');
  const internalAuthSecret = randomBytes(32).toString('hex');
  const oracleKey = path.join(tempDir, 'oracle.seed');
  let harness = makeContractHarness();
  let paygate = null;

  try {
    await harness.listen(contractPort);
    await writeFile(internalAuthSecretPath, `${internalAuthSecret}\n`, { mode: 0o600 });
    await writePaygateConfig({
      file: configPath,
      bind: `127.0.0.1:${paygatePort}`,
      contractRpc: contractBase,
      eventStore,
      accountStore,
      consentStore,
      internalAuthSecret: internalAuthSecretPath,
      oracleKey,
    });
    paygate = spawnPaygate(args.paygateBin, configPath, secrets);
    const health = await waitForHealth(paygateBase, paygate);
    const admin = String(health.oracle_pubkey).toLowerCase();
    assert.match(admin, /^[0-9a-f]{64}$/);
    harness.setAdmin(admin);
    await harness.storage.put('admin', admin);
    await seedParam(harness.storage, 'epoch_seconds', EPOCH_SECONDS);

    const tag = `fo3-${Date.now().toString(36)}-${randomBytes(3).toString('hex')}`;
    const nonce = randomBytes(32).toString('hex');
    const onboard = await requestJson(`${paygateBase}/v1/stripe/connect/onboard`, {
      method: 'POST',
      body: { provider: args.provider, country: args.country, request_nonce: nonce },
      internalAuthSecret,
    });
    assert.equal(onboard.ok, true);
    assert.equal(onboard.provider, args.provider);
    assert.equal(onboard.mode, 'test');
    if (onboard.onboarding) {
      assert.equal(new URL(onboard.onboarding.url).hostname, 'connect.stripe.com');
      assert.equal(onboard.copy_paste?.onboarding_url, onboard.onboarding.url);
    }

    let account = await stripeGet(secrets.secretKey, `accounts/${onboard.account.id}`);
    account = await completeCustomAccount(secrets.secretKey, account, args, tag);
    const status = await requestJson(`${paygateBase}/v1/stripe/connect/status`, {
      method: 'POST',
      body: { provider: args.provider, request_nonce: randomBytes(32).toString('hex') },
      internalAuthSecret,
    });
    assert.equal(status.account.id, account.id);
    assert.equal(status.account.ready, true);
    assert.equal(status.account.default_currency, args.destinationCurrency);

    const platformAccount = await stripeGet(secrets.secretKey, 'account', null, {
      apiVersion: STRIPE_FX_API_VERSION,
    });
    assert.match(platformAccount.id, /^acct_[A-Za-z0-9._-]+$/);
    assert.match(platformAccount.default_currency, /^[a-z]{3}$/);
    const platformSourceCurrency = platformAccount.default_currency;

    const user = randomBytes(32).toString('hex');
    const grossAu = BigInt(args.amountMinor) * AU_PER_USD_MINOR;
    const checkout = await requestJson(`${paygateBase}/v1/stripe/checkout-sessions`, {
      method: 'POST',
      body: {
        who: user,
        au: grossAu.toString(),
        currency: 'usd',
        locale: 'en',
        success_url: 'https://stripe.com',
        cancel_url: 'https://stripe.com',
        idempotency_key: `mayhem-fo3-checkout-${tag}`,
      },
      internalAuthSecret,
    });
    assert.equal(new URL(checkout.checkout_session.url).hostname, 'checkout.stripe.com');
    assert.equal(checkout.copy_paste.checkout_url, checkout.checkout_session.url);
    assert.equal(checkout.checkout_session.currency, 'usd');
    assert.equal(checkout.checkout_session.amount_total, args.amountMinor);
    const checkoutSession = await stripeGet(
      secrets.secretKey,
      `checkout/sessions/${checkout.checkout_session.id}`,
      null,
      { apiVersion: STRIPE_CHECKOUT_API_VERSION }
    );
    assert.equal(checkoutSession.currency, 'usd');
    assert.equal(checkoutSession.amount_total, args.amountMinor);
    assert.equal(checkoutSession.adaptive_pricing?.enabled, true);

    const createdAt = Math.floor(Date.now() / 1_000);
    const created = await requestJson(`${paygateBase}/v1/stripe/payment-intents`, {
      method: 'POST',
      body: {
        who: user,
        au: grossAu.toString(),
        currency: 'usd',
        idempotency_key: `mayhem-fo3-deposit-${tag}`,
      },
      internalAuthSecret,
    });
    const confirmed = await confirmPaymentIntent(
      secrets.secretKey,
      created.payment_intent.id,
      tag
    );
    assert.equal(confirmed.status, 'succeeded');
    assert.equal(confirmed.amount_received, args.amountMinor);
    assert.equal(confirmed.currency, 'usd');
    assert.equal(confirmed.metadata?.mayhem_fiat_currency, 'usd');
    assert.equal(confirmed.metadata?.mayhem_fiat_amount_minor, String(args.amountMinor));
    const event = await findPaymentIntentEvent(secrets.secretKey, confirmed.id, createdAt);
    assert.equal(event.data?.object?.currency, 'usd');
    assert.equal(event.data?.object?.amount_received, args.amountMinor);
    const eventPayload = JSON.stringify(event);
    const eventTs = Math.floor(Date.now() / 1_000);
    const stripeSignature = stripeSignatureHeader(secrets.webhookSecret, eventPayload, eventTs);
    const firstWebhook = await requestJson(`${paygateBase}/v1/stripe/webhook`, {
      method: 'POST',
      body: eventPayload,
      headers: {
        'content-type': 'application/json',
        'stripe-signature': stripeSignature,
      },
    });
    const replayWebhook = await requestJson(`${paygateBase}/v1/stripe/webhook`, {
      method: 'POST',
      body: eventPayload,
      headers: {
        'content-type': 'application/json',
        'stripe-signature': stripeSignature,
      },
    });
    assert.equal(firstWebhook.credited, true);
    assert.equal(replayWebhook.duplicate, true);

    const epoch = Math.floor(Number(event.created) / EPOCH_SECONDS) + 1;
    const balance = (await harness.storage.get(`bal/${user}/fiat`))?.value;
    const depositRoot = (await harness.storage.get(`ev/dep/${epoch}`))?.value;
    assert.equal(String(balance?.au), grossAu.toString());
    assert.equal(depositRoot?.count, 1);
    assert.equal(String(depositRoot?.au_total), grossAu.toString());

    const session = await seedSettlementState({
      harness,
      admin,
      provider: args.provider,
      account,
      args,
      user,
      depositRoot,
      epoch,
      at: Number(event.created),
    });
    const applyState = (await harness.storage.get('epoch/apply/state')).value;
    const requiredTransferBalance = Math.max(args.amountMinor * 2, 1_000);
    const usdBalanceSetup = await ensureUsdTransferBalance(
      secrets.secretKey,
      requiredTransferBalance,
      args,
      tag
    );
    const balanceSetup = platformSourceCurrency === 'usd'
      ? usdBalanceSetup
      : await ensureAvailableBalance(
        secrets.secretKey,
        platformSourceCurrency,
        requiredTransferBalance,
        tag
      );

    const commandArgs = settlementArgs({
      args,
      tempDir,
      contractBase,
      epoch,
      at: Number(event.created),
    });
    const preSettlementSnapshot = harness.storage.snapshotBytes();
    harness.setFeatureOnline(false);
    const interrupted = await runMayhem(args.mayhemBin, commandArgs, { expectFailure: true });
    assert.match(
      `${interrupted.stderr}\n${interrupted.stdout}`,
      /admin appender intentionally offline/
    );
    const transfersAfterFailure = await exactTransfers(secrets.secretKey, {
      epoch,
      applyHash: applyState.last_apply_hash,
      provider: args.provider,
      destination: account.id,
    });
    if (transfersAfterFailure.length !== 1) {
      const cliOutput = redactStripeCredentials(
        `${interrupted.stderr}\n${interrupted.stdout}`.trim()
      );
      throw new Error(
        `interrupted settlement created ${transfersAfterFailure.length} matching Stripe transfers, expected 1`
        + (cliOutput ? `; CLI output: ${cliOutput}` : '')
      );
    }
    assert.equal(await harness.storage.get(`settle/targeted/fiat/${epoch}`), null);
    const stripeEvidence = await verifyStripeTransferEvidence(secrets.secretKey, {
      transfer: transfersAfterFailure[0],
      provider: args.provider,
      payoutRevision: session.payoutRevision,
      destination: account.id,
      destinationCurrency: args.destinationCurrency,
      epoch,
      applyHash: applyState.last_apply_hash,
    });
    assert.equal(stripeEvidence.transfer.currency, platformSourceCurrency);

    await harness.close();
    const rebuiltStorage = MemoryStorage.fromSnapshotBytes(preSettlementSnapshot);
    harness = makeContractHarness({ storage: rebuiltStorage, admin });
    await harness.listen(contractPort);

    const recovered = await runMayhem(args.mayhemBin, commandArgs);
    assert.equal(recovered.report.ok, true);
    assert.equal(recovered.report.submitted, true);
    assert.equal(recovered.report.reconciliation?.all_provider_transfers_verified, true);
    const providerTransferReport = recovered.report.stripe_transfers?.find(
      (entry) => entry.transfer?.id === stripeEvidence.transfer.id
    );
    assert.ok(providerTransferReport, 'recovered report is missing provider transfer');
    assert.equal(providerTransferReport.transfer.recovered, true);
    assert.equal(providerTransferReport.transfer.verified, true);
    assert.equal(providerTransferReport.transfer.source_currency, platformSourceCurrency);
    assert.equal(providerTransferReport.transfer.destination, account.id);
    assert.equal(
      providerTransferReport.transfer.fx_quote,
      platformSourceCurrency === args.destinationCurrency
        ? null
        : stripeEvidence.quoteEvidence?.id
    );
    assert.deepEqual(providerTransferReport.quote, stripeEvidence.quoteEvidence);
    assert.equal(
      providerTransferReport.destination_payment.id,
      stripeEvidence.destinationPayment.id
    );
    assert.equal(
      providerTransferReport.destination_payment.currency,
      args.destinationCurrency
    );
    assert.equal(
      providerTransferReport.destination_payment.amount_minor,
      stripeEvidence.destinationBalanceTransaction.net
    );
    assert.equal(
      providerTransferReport.destination_payment.gross_amount_minor,
      stripeEvidence.destinationBalanceTransaction.amount
    );
    assert.equal(
      providerTransferReport.destination_payment.source_currency,
      stripeEvidence.destinationPayment.currency
    );
    assert.equal(
      providerTransferReport.destination_payment.source_amount_minor,
      stripeEvidence.destinationPayment.amount
    );
    assert.equal(
      providerTransferReport.destination_payment.balance_transaction,
      stripeEvidence.destinationBalanceTransaction.id
    );
    assert.equal(
      providerTransferReport.destination_payment.fee_minor,
      stripeEvidence.destinationBalanceTransaction.fee
    );
    assert.equal(
      providerTransferReport.destination_payment.net_minor,
      stripeEvidence.destinationBalanceTransaction.net
    );
    assert.equal(
      providerTransferReport.destination_payment.exchange_rate ?? null,
      stripeEvidence.destinationBalanceTransaction.exchange_rate == null
        ? null
        : String(stripeEvidence.destinationBalanceTransaction.exchange_rate)
    );

    const settlementKey = `settle/targeted/fiat/${epoch}`;
    const settlement = (await harness.storage.get(settlementKey))?.value;
    assert.ok(settlement, 'targeted fiat settlement was not recorded');
    assert.equal(settlement.source_currency, platformSourceCurrency);
    assert.equal(settlement?.epoch_apply_hash, applyState.last_apply_hash);
    const providerOutput = settlement.outputs.find((entry) => entry.role === 'provider');
    const providerTransfer = settlement.stripe_transfers.find(
      (entry) => entry.kind === 'stripe_transfer'
    );
    assert.equal(providerOutput.provider, args.provider);
    assert.equal(providerOutput.payout_revision, session.payoutRevision);
    assert.equal(providerOutput.to, account.id);
    assert.equal(providerOutput.source_currency, platformSourceCurrency);
    assert.equal(
      providerOutput.source_amount_minor,
      String(stripeEvidence.transfer.amount)
    );
    assert.equal(providerOutput.destination_currency, args.destinationCurrency);
    assert.equal(
      providerOutput.destination_amount_minor,
      String(stripeEvidence.destinationBalanceTransaction.net)
    );
    assert.equal(providerOutput.fx_quote_id, stripeEvidence.quoteEvidence?.id ?? null);
    assert.equal(providerOutput.fx_quote_hash, stripeEvidence.quoteHash);
    assert.equal(providerTransfer.ref, stripeEvidence.transfer.id);
    assert.equal(providerTransfer.destination, account.id);
    assert.equal(providerTransfer.source_currency, platformSourceCurrency);
    assert.equal(providerTransfer.source_amount_minor, providerOutput.source_amount_minor);
    assert.equal(providerTransfer.destination_currency, args.destinationCurrency);
    assert.equal(
      providerTransfer.destination_amount_minor,
      providerOutput.destination_amount_minor
    );
    assert.equal(providerTransfer.fx_quote_id, stripeEvidence.quoteEvidence?.id ?? null);
    assert.equal(providerTransfer.fx_quote_hash, stripeEvidence.quoteHash);
    assert.equal(
      providerTransfer.destination_payment,
      stripeEvidence.destinationPayment.id
    );
    assert.equal(
      BigInt(providerOutput.paid_au) + BigInt(providerOutput.dust_au),
      BigInt(providerOutput.liability_au)
    );
    assert.equal(providerOutput.rounding_au, providerOutput.dust_au);
    assert.equal(
      BigInt(settlement.gross_paid_au) + BigInt(settlement.dust_au),
      BigInt(settlement.gross_liability_au)
    );
    assert.equal(settlement.gross_liability_au, grossAu.toString());
    assert.deepEqual(settlement.destination_totals, [{
      currency: args.destinationCurrency,
      amount_minor: String(stripeEvidence.destinationBalanceTransaction.net),
    }]);
    assert.ok(await harness.storage.get(`rail/seen/stripe/${stripeEvidence.transfer.id}`));
    if (stripeEvidence.quoteEvidence) {
      assert.ok(
        await harness.storage.get(`rail/seen/stripe-fx-quote/${stripeEvidence.quoteEvidence.id}`)
      );
    }
    assert.ok(
      await harness.storage.get(
        `rail/seen/stripe-destination-payment/${stripeEvidence.destinationPayment.id}`
      )
    );
    const settledLiability = (
      await harness.storage.get(
        `payout/liability/fiat/${args.provider}/${session.payoutRevision}`
      )
    )?.value;
    assert.equal(settledLiability?.paid_cum_au, providerOutput.paid_au);

    const settledSnapshot = harness.storage.snapshotBytes();
    const replay = await runMayhem(args.mayhemBin, commandArgs);
    assert.ok(replay.report.already_settled);
    assert.equal(replay.report.stripe_transfers.length, 0);
    const transfersAfterReplay = await exactTransfers(secrets.secretKey, {
      epoch,
      applyHash: applyState.last_apply_hash,
      provider: args.provider,
      destination: account.id,
    });
    assert.equal(transfersAfterReplay.length, 1);
    assert.equal(transfersAfterReplay[0].id, stripeEvidence.transfer.id);
    const destinationPayments = await stripeGet(
      secrets.secretKey,
      'charges',
      {
        limit: 100,
        'created[gte]': stripeEvidence.transfer.created - 5,
      },
      {
        apiVersion: STRIPE_FX_API_VERSION,
        stripeAccount: account.id,
      }
    );
    const exactDestinationPayments = (destinationPayments.data ?? []).filter(
      (payment) => payment.source_transfer === stripeEvidence.transfer.id
    );
    assert.equal(exactDestinationPayments.length, 1);
    assert.equal(exactDestinationPayments[0].id, stripeEvidence.destinationPayment.id);
    assert.equal(harness.storage.snapshotBytes(), settledSnapshot);

    const eventLines = (await readFile(eventStore, 'utf8')).trim().split(/\r?\n/).filter(Boolean);
    assert.equal(eventLines.length, 1);
    const report = {
      ok: true,
      mode: 'test',
      provider: args.provider,
      connect: {
        account_id: account.id,
        account_type: account.type,
        country: account.country,
        default_currency: account.default_currency,
        details_submitted: account.details_submitted,
        payouts_enabled: account.payouts_enabled,
        transfers_enabled: account.capabilities?.transfers === 'active',
        onboarding_url_validated: onboard.onboarding
          ? new URL(onboard.onboarding.url).hostname === 'connect.stripe.com'
          : 'account_already_ready',
      },
      deposit: {
        accounting_denomination: 'au_usd',
        integration_currency: 'usd',
        integration_amount_minor: args.amountMinor,
        adaptive_pricing_enabled: checkoutSession.adaptive_pricing?.enabled === true,
        checkout_url_validated: true,
        checkout_no_open: true,
        payment_intent_id: confirmed.id,
        stripe_event_id: event.id,
        webhook_replay_duplicate: replayWebhook.duplicate === true,
        credited_au: grossAu.toString(),
      },
      session: {
        session_id: session.sessionId,
        epoch,
        gross_au: grossAu.toString(),
        provider_au: session.applied.earned_au,
        operator_fee_au: session.applied.fee_au,
      },
      settlement: {
        payout_revision: session.payoutRevision,
        platform_source_currency: platformSourceCurrency,
        source_amount_minor: stripeEvidence.transfer.amount,
        destination_currency: args.destinationCurrency,
        destination_amount_minor: stripeEvidence.destinationBalanceTransaction.net,
        destination_net_minor: stripeEvidence.destinationBalanceTransaction.net,
        destination_fee_minor: stripeEvidence.destinationBalanceTransaction.fee,
        transfer_id: stripeEvidence.transfer.id,
        transfer_destination: stripeEvidence.transfer.destination,
        fx_quote_id: stripeEvidence.quoteEvidence?.id ?? null,
        fx_quote_hash: stripeEvidence.quoteHash,
        destination_payment_id: stripeEvidence.destinationPayment.id,
        destination_balance_transaction_id: stripeEvidence.destinationBalanceTransaction.id,
        interrupted_exit_code: interrupted.code,
        index_cold_restored: true,
        recovered_without_second_transfer: true,
        deterministic_rerun_transfer_count: transfersAfterReplay.length,
        deterministic_rerun_destination_payment_count: exactDestinationPayments.length,
        ledger_key: settlementKey,
        epoch_apply_hash: applyState.last_apply_hash,
      },
      platform_balance: {
        currency: platformSourceCurrency,
        account_default_currency: platformAccount.default_currency,
        ...balanceSetup,
      },
      usd_transfer_balance: usdBalanceSetup,
      temp_dir: args.keepTemp ? tempDir : undefined,
    };
    await writeFile(path.join(tempDir, 'report.json'), `${JSON.stringify(report, null, 2)}\n`);
    process.stdout.write(`${JSON.stringify(report, null, args.json ? 0 : 2)}\n`);
  } finally {
    await paygate?.stop();
    await harness.close();
    if (!args.keepTemp) await rm(tempDir, { recursive: true, force: true });
  }
}

main().catch((error) => {
  const details = [redactStripeCredentials(error.message), error.code ? `code=${error.code}` : null]
    .filter(Boolean)
    .join(' ');
  console.error(`stripe sandbox payout smoke failed: ${details}`);
  if (process.env.MAYHEM_SMOKE_DEBUG === '1' && error.stack) {
    console.error(redactStripeCredentials(error.stack));
  }
  process.exitCode = 1;
});

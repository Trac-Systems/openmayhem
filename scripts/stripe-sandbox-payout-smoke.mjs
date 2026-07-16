#!/usr/bin/env node
import assert from 'node:assert/strict';
import { createHmac, randomBytes } from 'node:crypto';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import MayhemContract, {
  SESSION_RECEIPT_SCHEMA_VERSION,
} from '../intercom/contract/contract.js';
import { recomputeEpoch } from '../intercom/scripts/recompute-epoch-roots.mjs';
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
const AU_PER_MINOR = 10_000_000_000_000_000n;
const ZERO_HEX = '0'.repeat(64);
const CONNECT_READY_TIMEOUT_MS = 180_000;
const STRIPE_EVENT_TIMEOUT_MS = 60_000;
const STRIPE_BALANCE_TIMEOUT_MS = 60_000;

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const usage = () => `Usage: node scripts/stripe-sandbox-payout-smoke.mjs [options]

Runs the complete Stripe test-mode economic path without browser interaction:
Connect onboarding URL -> API-completed test account -> checkout URL -> confirmed
deposit -> metered fiat session -> provider transfer -> ledger settlement evidence.
It deliberately takes the admin appender offline after Stripe creates the transfer,
then cold-restores the index and proves recovery creates no duplicate transfer.

Options:
  --stripe-env-file <path>  Test STRIPE_SECRET_KEY + STRIPE_WEBHOOK_SECRET
                            (default: ${DEFAULT_STRIPE_ENV_FILE})
  --provider <64-hex>       Provider identity (default: random)
  --country <US|DE>         Connected-account country (default: US)
  --currency <usd|eur>      Settlement currency (default follows country)
  --amount-minor <integer>  Test deposit/session amount (default: ${DEFAULT_AMOUNT_MINOR})
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
    country: 'US',
    currency: null,
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
    else if (arg === '--currency') args.currency = next().trim().toLowerCase();
    else if (arg === '--amount-minor') args.amountMinor = Number.parseInt(next(), 10);
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
  if (!['US', 'DE'].includes(args.country)) throw new Error('--country must be US or DE');
  args.currency ??= args.country === 'DE' ? 'eur' : 'usd';
  if (!['usd', 'eur'].includes(args.currency)) throw new Error('--currency must be usd or eur');
  if ((args.country === 'US' && args.currency !== 'usd') || (args.country === 'DE' && args.currency !== 'eur')) {
    throw new Error('--currency must match the connected account default currency (US=usd, DE=eur)');
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
    /(?:sk|rk|pk)_(?:test|live)_[A-Za-z0-9_.*-]+|whsec_[A-Za-z0-9_.*-]+/g,
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
        const direct = await executeFeature(
          contract,
          storage,
          'mayhem_feature',
          body.key,
          body.value,
          admin
        );
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

async function writePaygateConfig({ file, bind, contractRpc, secrets, eventStore, accountStore, oracleKey }) {
  await writeFile(file, `[server]
bind = "${tomlString(bind)}"

[contract]
rpc_url = "${tomlString(contractRpc)}"
simulate = false
epoch_seconds = ${EPOCH_SECONDS}

[oracle]
key_path = "${tomlString(oracleKey)}"

[stripe]
enabled = true
mode = "test"
secret_key = "${tomlString(secrets.secretKey)}"
webhook_secret = "${tomlString(secrets.webhookSecret)}"
event_store_path = "${tomlString(eventStore)}"
webhook_tolerance_seconds = 300
backfill_enabled = false
connect_account_type = "custom"
connect_accounts_path = "${tomlString(accountStore)}"
connect_return_url = "https://dashboard.stripe.com/"
connect_refresh_url = "https://dashboard.stripe.com/"
`, { mode: 0o600 });
}

function spawnPaygate(binary, configPath) {
  if (!existsSync(binary)) throw new Error(`mayhem-paygate binary does not exist: ${binary}`);
  const child = spawn(binary, ['--config', configPath], {
    cwd: REPO_ROOT,
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

async function requestJson(url, { method = 'GET', body = null, headers = {} } = {}) {
  const response = await fetch(url, {
    method,
    headers: {
      ...(body === null ? {} : { 'content-type': 'application/json' }),
      ...headers,
    },
    ...(body === null ? {} : { body: typeof body === 'string' ? body : JSON.stringify(body) }),
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

async function stripeRequest(secretKey, method, endpoint, params = null, idempotencyKey = null) {
  const url = new URL(`https://api.stripe.com/v1/${endpoint}`);
  const headers = { authorization: `Bearer ${secretKey}` };
  const init = { method, headers };
  if (idempotencyKey) headers['idempotency-key'] = idempotencyKey;
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

const stripeGet = (secret, endpoint, params = null) => stripeRequest(secret, 'GET', endpoint, params);
const stripePost = (secret, endpoint, params, idempotencyKey = null) => (
  stripeRequest(secret, 'POST', endpoint, params, idempotencyKey)
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
  const countryFields = args.country === 'US'
    ? {
        'individual[phone]': '+14155550123',
        'individual[address][line1]': 'address_full_match',
        'individual[address][city]': 'San Francisco',
        'individual[address][state]': 'CA',
        'individual[address][postal_code]': '94107',
        'individual[address][country]': 'US',
        'individual[id_number]': '000000000',
        'individual[ssn_last_4]': '0000',
        external_account: 'btok_us_verified',
      }
    : {
        'individual[phone]': '+4915112345678',
        'individual[address][line1]': 'address_full_match',
        'individual[address][city]': 'Berlin',
        'individual[address][postal_code]': '10115',
        'individual[address][country]': 'DE',
        external_account: 'btok_de',
      };
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
  const source = currency === 'usd' ? 'btok_us_verified' : 'btok_de';
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
    payouts: {
      stripe: {
        addr: account.id,
        method: 'stripe',
        currency: args.currency,
        set_by: admin,
        set_by_role: 'admin',
        set_at: 'stripe-sandbox-payout-smoke',
      },
    },
    probation: { successful_sessions: 1 },
    registered_at: 'stripe-sandbox-payout-smoke',
    updated_at: 'stripe-sandbox-payout-smoke',
  });

  const grossAu = BigInt(args.amountMinor) * AU_PER_MINOR;
  const sessionId = `fo3-${randomBytes(16).toString('hex')}`;
  const receipt = {
    schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
    session_id: sessionId,
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
  let committed;
  let applied;
  try {
    console.log = () => {};
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
  return { sessionId, receipt, roll, committed, applied };
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
    '--operator-currency',
    args.currency,
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
  });
  return (listed.data ?? []).filter((transfer) => (
    transfer.destination === destination
    && transfer.transfer_group === transferGroup
    && transfer.metadata?.mayhem_provider === provider
    && transfer.metadata?.mayhem_epoch === String(epoch)
    && transfer.metadata?.mayhem_epoch_apply_hash === applyHash
  ));
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
  const oracleKey = path.join(tempDir, 'oracle.seed');
  let harness = makeContractHarness();
  let paygate = null;

  try {
    await harness.listen(contractPort);
    await writePaygateConfig({
      file: configPath,
      bind: `127.0.0.1:${paygatePort}`,
      contractRpc: contractBase,
      secrets,
      eventStore,
      accountStore,
      oracleKey,
    });
    paygate = spawnPaygate(args.paygateBin, configPath);
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
    });
    assert.equal(status.account.id, account.id);
    assert.equal(status.account.ready, true);
    assert.equal(status.account.default_currency, args.currency);

    const user = randomBytes(32).toString('hex');
    const grossAu = BigInt(args.amountMinor) * AU_PER_MINOR;
    const checkout = await requestJson(`${paygateBase}/v1/stripe/checkout-sessions`, {
      method: 'POST',
      body: {
        who: user,
        au: grossAu.toString(),
        currency: args.currency,
        locale: 'en',
        success_url: 'https://stripe.com',
        cancel_url: 'https://stripe.com',
        idempotency_key: `mayhem-fo3-checkout-${tag}`,
      },
    });
    assert.equal(new URL(checkout.checkout_session.url).hostname, 'checkout.stripe.com');
    assert.equal(checkout.copy_paste.checkout_url, checkout.checkout_session.url);

    const createdAt = Math.floor(Date.now() / 1_000);
    const created = await requestJson(`${paygateBase}/v1/stripe/payment-intents`, {
      method: 'POST',
      body: {
        who: user,
        au: grossAu.toString(),
        currency: args.currency,
        idempotency_key: `mayhem-fo3-deposit-${tag}`,
      },
    });
    const confirmed = await confirmPaymentIntent(
      secrets.secretKey,
      created.payment_intent.id,
      tag
    );
    assert.equal(confirmed.status, 'succeeded');
    assert.equal(confirmed.amount_received, args.amountMinor);
    const event = await findPaymentIntentEvent(secrets.secretKey, confirmed.id, createdAt);
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
    const providerMinor = Number(
      (BigInt(args.amountMinor) * BigInt(10_000 - args.feeBps)) / 10_000n
    );
    const balanceSetup = await ensureAvailableBalance(
      secrets.secretKey,
      args.currency,
      providerMinor,
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
    const transfersAfterFailure = await exactTransfers(secrets.secretKey, {
      epoch,
      applyHash: applyState.last_apply_hash,
      provider: args.provider,
      destination: account.id,
    });
    assert.equal(transfersAfterFailure.length, 1);
    assert.equal(await harness.storage.get(`settle/fiat/${epoch}`), null);

    await harness.close();
    const rebuiltStorage = MemoryStorage.fromSnapshotBytes(preSettlementSnapshot);
    harness = makeContractHarness({ storage: rebuiltStorage, admin });
    await harness.listen(contractPort);

    const recovered = await runMayhem(args.mayhemBin, commandArgs);
    assert.equal(recovered.report.ok, true);
    assert.equal(recovered.report.submitted, true);
    assert.equal(recovered.report.reconciliation?.all_provider_transfers_verified, true);
    assert.equal(recovered.report.stripe_transfers?.length, 1);
    assert.equal(recovered.report.stripe_transfers[0].transfer.recovered, true);
    assert.equal(recovered.report.stripe_transfers[0].transfer.id, transfersAfterFailure[0].id);
    const settlement = (await harness.storage.get(`settle/fiat/${epoch}`))?.value;
    assert.equal(settlement?.stripe_transfers?.[0]?.ref, transfersAfterFailure[0].id);
    assert.equal(settlement?.epoch_apply_hash, applyState.last_apply_hash);

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
        transfer_id: transfersAfterFailure[0].id,
        transfer_amount_minor: transfersAfterFailure[0].amount,
        transfer_currency: transfersAfterFailure[0].currency,
        transfer_destination: transfersAfterFailure[0].destination,
        interrupted_exit_code: interrupted.code,
        index_cold_restored: true,
        recovered_without_second_transfer: true,
        deterministic_rerun_transfer_count: transfersAfterReplay.length,
        ledger_key: `settle/fiat/${epoch}`,
        epoch_apply_hash: applyState.last_apply_hash,
      },
      platform_balance: balanceSetup,
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
  process.exitCode = 1;
});

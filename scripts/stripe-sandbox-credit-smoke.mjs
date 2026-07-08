#!/usr/bin/env node
import { createHmac, randomBytes } from 'node:crypto';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import MayhemContract from '../intercom/contract/contract.js';
import { createHash, jsonStringify } from '../intercom/trac/trac-peer/src/utils/types.js';

const DEFAULT_STRIPE_ENV_FILE = path.resolve('.mayhem-local', 'secrets', 'stripe.txt');
const AU_PER_USD_CENT = 10_000_000_000_000_000n;
const DEFAULT_AU = '1000000000000000000';
const DEFAULT_CONTRACT_EPOCH_SECONDS = 3_600;
const DEFAULT_SMOKE_ADMIN_EPOCH_SECONDS = 7_200;
const ZERO_HEX = '0'.repeat(64);
const SCRIPT_DIR = path.dirname(fileURLToPath(import.meta.url));

class MemoryStorage {
  constructor(initial = {}) {
    this.values = new Map(Object.entries(initial));
  }

  async get(key) {
    return this.values.has(key) ? { value: this.values.get(key) } : null;
  }

  async put(key, value) {
    this.values.set(key, value);
  }

  async del(key) {
    this.values.delete(key);
  }
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const usage = () => `Usage: node scripts/stripe-sandbox-credit-smoke.mjs [options]

Runs a live Stripe test-mode PaymentIntent creation through mayhem-paygate, then
posts the same signed payment_intent.succeeded webhook twice and verifies that
Mayhem contract state credits bal/<user> exactly once and writes ev/dep/<epoch>.

Options:
  --stripe-env-file <path>   File with STRIPE_SECRET_KEY and STRIPE_WEBHOOK_SECRET
                             (default: ${DEFAULT_STRIPE_ENV_FILE})
  --au <au_usd>              Whole-cent au_usd amount (default: ${DEFAULT_AU})
  --currency <usd|eur>       Stripe checkout currency (default: usd)
  --who <hex-pubkey>         64-hex test user id (default: random)
  --paygate-port <port>      Local paygate port (default: auto)
  --contract-port <port>     Local contract RPC port (default: auto)
  --epoch-seconds <seconds>  Admin params/epoch_seconds to seed in the local contract
                             (default: ${DEFAULT_SMOKE_ADMIN_EPOCH_SECONDS})
  --include-dispute          Also post a signed charge.dispute.created replay and
                             verify clawback/freeze evidence
  --keep-temp                Keep .mayhem-local smoke directory for inspection
  --json                     Emit JSON only
  --help                     Show this help
`;

function parseAu(value) {
  const raw = String(value ?? '').trim();
  if (!/^(0|[1-9]\d*)$/.test(raw)) {
    throw new Error('--au must be a canonical positive atto-USD integer');
  }
  const au = BigInt(raw);
  if (au <= 0n) throw new Error('--au must be positive');
  if (au % AU_PER_USD_CENT !== 0n) throw new Error('--au must be whole-cent aligned');
  return au;
}

function auToStripeMinor(au, label = '--au') {
  const minor = au / AU_PER_USD_CENT;
  if (minor > BigInt(Number.MAX_SAFE_INTEGER)) {
    throw new Error(`${label} exceeds Stripe minor-unit safe integer range`);
  }
  return Number(minor);
}

function parseArgs(argv) {
  const args = {
    stripeEnvFile: DEFAULT_STRIPE_ENV_FILE,
    au: DEFAULT_AU,
    currency: 'usd',
    who: randomBytes(32).toString('hex'),
    paygatePort: null,
    contractPort: null,
    epochSeconds: DEFAULT_SMOKE_ADMIN_EPOCH_SECONDS,
    includeDispute: false,
    keepTemp: false,
    json: false,
  };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    const next = () => {
      i += 1;
      if (i >= argv.length) throw new Error(`${arg} requires a value`);
      return argv[i];
    };
    if (arg === '--stripe-env-file') args.stripeEnvFile = next();
    else if (arg === '--au') args.au = next();
    else if (arg === '--currency') args.currency = next().trim().toLowerCase();
    else if (arg === '--who') args.who = next();
    else if (arg === '--paygate-port') args.paygatePort = Number.parseInt(next(), 10);
    else if (arg === '--contract-port') args.contractPort = Number.parseInt(next(), 10);
    else if (arg === '--epoch-seconds') args.epochSeconds = Number.parseInt(next(), 10);
    else if (arg === '--include-dispute') args.includeDispute = true;
    else if (arg === '--keep-temp') args.keepTemp = true;
    else if (arg === '--json') args.json = true;
    else if (arg === '--help' || arg === '-h') {
      process.stdout.write(usage());
      process.exit(0);
    } else {
      throw new Error(`unknown argument: ${arg}`);
    }
  }
  const au = parseAu(args.au);
  args.au = au.toString();
  args.stripeAmountMinor = auToStripeMinor(au);
  if (!['usd', 'eur'].includes(args.currency)) throw new Error('--currency must be usd or eur');
  if (!/^[0-9a-f]{64}$/i.test(args.who)) throw new Error('--who must be 64 hex characters');
  if (!Number.isSafeInteger(args.epochSeconds) || args.epochSeconds < 60) {
    throw new Error('--epoch-seconds must be a safe integer >= 60');
  }
  for (const [name, port] of [['--paygate-port', args.paygatePort], ['--contract-port', args.contractPort]]) {
    if (port !== null && (!Number.isInteger(port) || port < 1 || port > 65_535)) {
      throw new Error(`${name} must be a TCP port`);
    }
  }
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

async function readKeyFile(file) {
  if (!file || !existsSync(file)) return {};
  const text = await readFile(file, 'utf8');
  const out = {};
  for (const rawLine of text.split(/\r?\n/)) {
    let line = rawLine.trim();
    if (!line || line.startsWith('#')) continue;
    if (line.startsWith('export ')) line = line.slice('export '.length).trim();
    const match = line.match(/^([A-Za-z_][A-Za-z0-9_]*)\s*(?:=|:)\s*(.*)$/);
    if (!match) continue;
    out[match[1]] = stripQuotes(match[2]);
  }
  return out;
}

async function loadStripeSecrets(stripeEnvFile) {
  const file = await readKeyFile(stripeEnvFile);
  const secretKey =
    process.env.STRIPE_SECRET_KEY
    || process.env.MAYHEM_STRIPE_SECRET_KEY
    || file.STRIPE_SECRET_KEY;
  const webhookSecret =
    process.env.STRIPE_WEBHOOK_SECRET
    || process.env.MAYHEM_STRIPE_WEBHOOK_SECRET
    || file.STRIPE_WEBHOOK_SECRET;
  if (!secretKey) throw new Error('missing STRIPE_SECRET_KEY');
  if (!webhookSecret) throw new Error('missing STRIPE_WEBHOOK_SECRET');
  return { secretKey, webhookSecret };
}

async function freePort() {
  const server = createServer();
  server.unref();
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  const { port } = server.address();
  await new Promise((resolve, reject) => server.close((err) => (err ? reject(err) : resolve())));
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
  if (chunks.length === 0) return {};
  return JSON.parse(Buffer.concat(chunks).toString('utf8'));
}

function makeContractHarness() {
  const storage = new MemoryStorage();
  const contract = new MayhemContract({ peer: { wallet: { verify: () => false } } }, {});
  let txCount = 0;
  const submitted = [];

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
        const tx = body.tx;
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
              dispatch: {
                type: prepared.type,
                value: prepared.value,
              },
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
        submitted.push({ tx, address, prepared, result: normalized });
        return sendJson(res, 200, { result: normalized });
      }
      if (req.method === 'GET' && url.pathname === '/v1/state') {
        const key = url.searchParams.get('key');
        if (!key) return sendJson(res, 400, { error: 'missing key' });
        const entry = await storage.get(key);
        return sendJson(res, 200, { key, value: entry?.value ?? null });
      }
      return sendJson(res, 404, { error: 'not found' });
    } catch (err) {
      return sendJson(res, 500, { error: err.message });
    }
  });

  return {
    server,
    storage,
    submitted,
    async listen(port) {
      await new Promise((resolve, reject) => {
        server.once('error', reject);
        server.listen(port, '127.0.0.1', resolve);
      });
      return server.address().port;
    },
    close() {
      return new Promise((resolve, reject) => server.close((err) => (err ? reject(err) : resolve())));
    },
  };
}

function tomlString(value) {
  return String(value).replace(/\\/g, '\\\\').replace(/"/g, '\\"');
}

async function writePaygateConfig({ file, bind, contractRpc, secretKey, webhookSecret, eventStore, oracleKey }) {
  await writeFile(file, `[server]
bind = "${tomlString(bind)}"

[contract]
rpc_url = "${tomlString(contractRpc)}"
simulate = false
epoch_seconds = ${DEFAULT_CONTRACT_EPOCH_SECONDS}

[oracle]
key_path = "${tomlString(oracleKey)}"

[stripe]
enabled = true
mode = "test"
secret_key = "${tomlString(secretKey)}"
webhook_secret = "${tomlString(webhookSecret)}"
event_store_path = "${tomlString(eventStore)}"
webhook_tolerance_seconds = 300

[coinbase]
enabled = false
`, { mode: 0o600 });
}

async function seedAdminEpochSeconds(storage, epochSeconds) {
  await storage.put('params/epoch_seconds', {
    key: 'epoch_seconds',
    current: {
      value: epochSeconds,
      ver: 1,
      submitted_at: 0,
      effective_at: 0,
      set_by_role: 'admin',
      set_at: 'stripe-sandbox-credit-smoke',
    },
    pending: null,
  });
}

function spawnPaygate(configPath) {
  const child = spawn('cargo', ['run', '-p', 'mayhem-paygate', '--', '--config', configPath], {
    cwd: path.resolve(SCRIPT_DIR, '..'),
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  const logs = [];
  const collect = (chunk) => {
    logs.push(chunk.toString('utf8'));
    while (logs.join('').length > 20_000) logs.shift();
  };
  child.stdout.on('data', collect);
  child.stderr.on('data', collect);
  return {
    child,
    logs: () => logs.join(''),
    stop: async () => {
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
    } catch (err) {
      lastError = err;
    }
    await sleep(500);
  }
  throw new Error(`paygate health did not become ready: ${lastError?.message ?? 'timeout'}\n${paygate.logs()}`);
}

async function postJson(url, body, headers = {}) {
  const response = await fetch(url, {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      ...headers,
    },
    body: JSON.stringify(body),
  });
  const text = await response.text();
  let value;
  try {
    value = JSON.parse(text);
  } catch {
    value = { raw: text };
  }
  if (!response.ok) {
    throw new Error(`${url} returned ${response.status}: ${JSON.stringify(value)}`);
  }
  return value;
}

async function postRawJson(url, body, headers = {}) {
  const response = await fetch(url, {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      ...headers,
    },
    body,
  });
  const text = await response.text();
  let value;
  try {
    value = JSON.parse(text);
  } catch {
    value = { raw: text };
  }
  if (!response.ok) {
    throw new Error(`${url} returned ${response.status}: ${JSON.stringify(value)}`);
  }
  return value;
}

async function getJson(url) {
  const response = await fetch(url);
  const text = await response.text();
  let value;
  try {
    value = JSON.parse(text);
  } catch {
    value = { raw: text };
  }
  if (!response.ok) {
    throw new Error(`${url} returned ${response.status}: ${JSON.stringify(value)}`);
  }
  return value;
}

function stripeSignatureHeader(secret, payload, timestamp) {
  const signature = createHmac('sha256', secret)
    .update(`${timestamp}.${payload}`)
    .digest('hex');
  return `t=${timestamp},v1=${signature}`;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const secrets = await loadStripeSecrets(args.stripeEnvFile);
  const localRoot = path.resolve(SCRIPT_DIR, '..', '.mayhem-local');
  await mkdir(localRoot, { recursive: true });
  const tempDir = await mkdtemp(path.join(localRoot, 'stripe-sandbox-credit-smoke-'));
  const contractPort = args.contractPort ?? await freePort();
  const paygatePort = args.paygatePort ?? await freePort();
  const contractBase = `http://127.0.0.1:${contractPort}/v1`;
  const paygateBase = `http://127.0.0.1:${paygatePort}`;
  const eventStore = path.join(tempDir, 'stripe-events.jsonl');
  const configPath = path.join(tempDir, 'paygate.toml');
  const oracleKey = path.join(tempDir, 'oracle.seed');
  const contract = makeContractHarness();
  let paygate = null;

  try {
    await contract.listen(contractPort);
    await writePaygateConfig({
      file: configPath,
      bind: `127.0.0.1:${paygatePort}`,
      contractRpc: contractBase,
      secretKey: secrets.secretKey,
      webhookSecret: secrets.webhookSecret,
      eventStore,
      oracleKey,
    });
    paygate = spawnPaygate(configPath);
    const health = await waitForHealth(paygateBase, paygate);
    await contract.storage.put('admin', health.oracle_pubkey);
    await seedAdminEpochSeconds(contract.storage, args.epochSeconds);

    const idempotencyKey = `mayhem-stripe-smoke-${Date.now()}`;
    const created = await postJson(`${paygateBase}/v1/stripe/payment-intents`, {
      who: args.who,
      au: args.au,
      currency: args.currency,
      idempotency_key: idempotencyKey,
    });
    const intent = created.payment_intent;
    if (intent.currency !== args.currency) {
      throw new Error(`PaymentIntent currency mismatch: ${intent.currency}`);
    }
    const eventId = `evt_mayhem_smoke_${Date.now()}_${randomBytes(4).toString('hex')}`;
    const chargeId = `ch_${eventId.slice(-8)}`;
    const at = Math.floor(Date.now() / 1000);
    const epoch = Math.floor(at / args.epochSeconds) + 1;
    const payload = JSON.stringify({
      id: eventId,
      object: 'event',
      type: 'payment_intent.succeeded',
      created: at,
      data: {
        object: {
          id: intent.id,
          object: 'payment_intent',
          latest_charge: chargeId,
          amount: intent.amount,
          amount_received: intent.amount,
          currency: intent.currency,
          metadata: {
            mayhem_who: args.who,
            mayhem_au: String(args.au),
            mayhem_denom: 'au_usd',
            mayhem_fiat_currency: intent.currency,
            mayhem_fiat_amount_minor: String(intent.amount),
          },
        },
      },
    });
    const signature = stripeSignatureHeader(secrets.webhookSecret, payload, at);
    const first = await postRawJson(`${paygateBase}/v1/stripe/webhook`, payload, {
      'stripe-signature': signature,
    });
    const replay = await postRawJson(`${paygateBase}/v1/stripe/webhook`, payload, {
      'stripe-signature': signature,
    });
    const balance = await getJson(`${contractBase}/state?key=${encodeURIComponent(`bal/${args.who}/fiat`)}`);
    const depositRoot = await getJson(`${contractBase}/state?key=${encodeURIComponent(`ev/dep/${epoch}`)}`);
    const eventLog = existsSync(eventStore) ? await readFile(eventStore, 'utf8') : '';
    const eventLogLines = eventLog.trim() ? eventLog.trim().split(/\r?\n/).length : 0;
    const balanceAu = String(balance.value?.au ?? '0');
    const dep = depositRoot.value;
    const contractPostsAfterCredit = contract.submitted.length;
    let dispute = null;
    if (args.includeDispute) {
      const disputeEventId = `evt_mayhem_dispute_${Date.now()}_${randomBytes(4).toString('hex')}`;
      const disputeAt = at + 1;
      const disputePayload = JSON.stringify({
        id: disputeEventId,
        object: 'event',
        type: 'charge.dispute.created',
        created: disputeAt,
        data: {
          object: {
            id: `dp_${disputeEventId.slice(-8)}`,
            object: 'dispute',
            amount: args.stripeAmountMinor,
            currency: intent.currency,
            charge: chargeId,
            payment_intent: intent.id,
            reason: 'fraudulent',
            status: 'needs_response',
          },
        },
      });
      const disputeSignature = stripeSignatureHeader(secrets.webhookSecret, disputePayload, disputeAt);
      const disputeFirst = await postRawJson(`${paygateBase}/v1/stripe/webhook`, disputePayload, {
        'stripe-signature': disputeSignature,
      });
      const disputeReplay = await postRawJson(`${paygateBase}/v1/stripe/webhook`, disputePayload, {
        'stripe-signature': disputeSignature,
      });
      const balanceAfter = await getJson(`${contractBase}/state?key=${encodeURIComponent(`bal/${args.who}/fiat`)}`);
      const frozen = await getJson(`${contractBase}/state?key=${encodeURIComponent(`frozen/${args.who}`)}`);
      const reversalRoot = await getJson(`${contractBase}/state?key=${encodeURIComponent(`ev/dep/${epoch}`)}`);
      const updatedEventLog = existsSync(eventStore) ? await readFile(eventStore, 'utf8') : '';
      dispute = {
        event_id: disputeEventId,
        first_clawed_back: disputeFirst.clawed_back === true,
        replay_duplicate: disputeReplay.duplicate === true,
        balance_after_chargeback_au: balanceAfter.value?.au == null ? null : String(balanceAfter.value.au),
        frozen_status: frozen.value?.status ?? null,
        disputed_au_cum: frozen.value?.disputed_au_cum == null ? null : String(frozen.value.disputed_au_cum),
        event_log_lines: updatedEventLog.trim() ? updatedEventLog.trim().split(/\r?\n/).length : 0,
        deposit_count_after_reversal: reversalRoot.value?.count ?? null,
        deposit_au_total_after_reversal: reversalRoot.value?.au_total == null ? null : String(reversalRoot.value.au_total),
        reversed_au_total: reversalRoot.value?.reversed_au_total == null ? null : String(reversalRoot.value.reversed_au_total),
        clawback_au_total: reversalRoot.value?.clawback_au_total == null ? null : String(reversalRoot.value.clawback_au_total),
        network_absorbed_au_total: reversalRoot.value?.network_absorbed_au_total == null ? null : String(reversalRoot.value.network_absorbed_au_total),
      };
    }
    const creditOk =
      created.ok === true
      && first.ok === true
      && first.credited === true
      && first.duplicate === false
      && replay.ok === true
      && replay.duplicate === true
      && balanceAu === args.au
      && dep?.type === 'deposit_root'
      && dep.count === 1
      && String(dep.au_total ?? '0') === args.au
      && eventLogLines === 1
      && contractPostsAfterCredit === 1;
    const disputeOk = !args.includeDispute
      || (
        dispute?.first_clawed_back === true
        && dispute?.replay_duplicate === true
        && dispute?.balance_after_chargeback_au === '0'
        && dispute?.frozen_status === 'frozen'
        && dispute?.disputed_au_cum === args.au
        && dispute?.event_log_lines === 2
        && dispute?.reversed_au_total === args.au
        && dispute?.clawback_au_total === args.au
        && contract.submitted.length === 2
      );
    const ok = creditOk && disputeOk;
    const report = {
      ok,
      stripe: {
        payment_intent_id: intent.id,
        payment_intent_status: intent.status,
        payment_intent_currency: intent.currency,
        payment_intent_amount_cents: intent.amount,
        event_id: eventId,
        first_credited: first.credited,
        replay_duplicate: replay.duplicate,
      },
      dispute,
      contract: {
        oracle_pubkey: health.oracle_pubkey,
        who: args.who,
        epoch_seconds: args.epochSeconds,
        epoch,
        balance_au: balanceAu,
        deposit_count: dep?.count ?? null,
        deposit_au_total: dep?.au_total ?? null,
        deposit_root: dep?.merkle_root ?? null,
        submitted_contract_posts: contract.submitted.length,
      },
      paygate: {
        stripe_enabled: health.rails?.stripe?.enabled === true,
        coinbase_enabled: health.rails?.coinbase?.enabled === true,
        event_log_lines: eventLogLines,
      },
      temp_dir: args.keepTemp ? tempDir : undefined,
    };
    if (!ok) throw new Error(`smoke assertions failed: ${JSON.stringify(report)}`);
    process.stdout.write(`${JSON.stringify(report, null, args.json ? 0 : 2)}\n`);
  } finally {
    if (paygate) await paygate.stop();
    await contract.close().catch(() => {});
    if (!args.keepTemp) await rm(tempDir, { recursive: true, force: true });
  }
}

main().catch((err) => {
  console.error(`stripe sandbox credit smoke failed: ${err.message}`);
  process.exitCode = 1;
});

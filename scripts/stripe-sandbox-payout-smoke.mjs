#!/usr/bin/env node
import { randomBytes } from 'node:crypto';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const DEFAULT_STRIPE_ENV_FILE = '/Applications/MAMP/htdocs/gpd/stripe.txt';
const DEFAULT_MU = 1_000_000;
const DEFAULT_STRIPE_AMOUNT_MINOR = 100;
const DEFAULT_FEE_BPS = 1500;
const DEFAULT_BUSINESS_URL = 'https://trac.network';
const CONNECT_READY_TIMEOUT_MS = 180_000;
const CHARGE_TRANSFER_TIMEOUT_MS = 120_000;
const SCRIPT_DIR = path.dirname(fileURLToPath(import.meta.url));
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const usage = () => `Usage: node scripts/stripe-sandbox-payout-smoke.mjs [options]

Creates a live Stripe test-mode Custom connected account, runs a Stripe
destination charge on_behalf_of that connected account, extracts the real Stripe
transfer id, and reports the Stripe transfer facts. Contract payoutConfirm /
ev/pay evidence is retired; fiat transfer evidence is folded through epoch
settlement reports instead of per-payout Trac writes.

Options:
  --stripe-env-file <path>   File with STRIPE_SECRET_KEY
                             (default: ${DEFAULT_STRIPE_ENV_FILE})
  --mu <mu_usd>              Contract payout amount in integer micro-USD
                             (default: ${DEFAULT_MU})
  --currency <usd|eur>       Stripe payout evidence currency (default: usd)
  --amount-cents <cents>     Test charge/transfer amount in minor units
                             (default: ${DEFAULT_STRIPE_AMOUNT_MINOR})
  --fee-bps <bps>            Operator fee retained via application_fee_amount
                             (default: ${DEFAULT_FEE_BPS})
  --business-url <url>       Valid business URL for the test connected account
                             (default: ${DEFAULT_BUSINESS_URL})
  --keep-temp                Keep .mayhem-local smoke directory for inspection
  --json                     Emit compact JSON
  --help                     Show this help
`;

function parseArgs(argv) {
  const args = {
    stripeEnvFile: DEFAULT_STRIPE_ENV_FILE,
    mu: DEFAULT_MU,
    currency: 'usd',
    amountCents: DEFAULT_STRIPE_AMOUNT_MINOR,
    feeBps: DEFAULT_FEE_BPS,
    businessUrl: DEFAULT_BUSINESS_URL,
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
    else if (arg === '--mu') args.mu = Number.parseInt(next(), 10);
    else if (arg === '--currency') args.currency = next().trim().toLowerCase();
    else if (arg === '--amount-cents') args.amountCents = Number.parseInt(next(), 10);
    else if (arg === '--fee-bps') args.feeBps = Number.parseInt(next(), 10);
    else if (arg === '--business-url') args.businessUrl = next();
    else if (arg === '--keep-temp') args.keepTemp = true;
    else if (arg === '--json') args.json = true;
    else if (arg === '--help' || arg === '-h') {
      process.stdout.write(usage());
      process.exit(0);
    } else {
      throw new Error(`unknown argument: ${arg}`);
    }
  }
  if (!Number.isSafeInteger(args.mu) || args.mu <= 0) throw new Error('--mu must be a positive integer');
  if (!['usd', 'eur'].includes(args.currency)) throw new Error('--currency must be usd or eur');
  if (!Number.isSafeInteger(args.amountCents) || args.amountCents <= 0) {
    throw new Error('--amount-cents must be a positive integer');
  }
  if (!Number.isSafeInteger(args.feeBps) || args.feeBps < 0 || args.feeBps > 5000) {
    throw new Error('--fee-bps must be an integer from 0 to 5000');
  }
  const businessUrl = new URL(args.businessUrl);
  if (!['http:', 'https:'].includes(businessUrl.protocol)) {
    throw new Error('--business-url must be an http(s) URL');
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

async function loadStripeSecret(stripeEnvFile) {
  const file = await readKeyFile(stripeEnvFile);
  const secretKey =
    process.env.STRIPE_SECRET_KEY
    || process.env.MAYHEM_STRIPE_SECRET_KEY
    || file.STRIPE_SECRET_KEY;
  if (!secretKey) throw new Error('missing STRIPE_SECRET_KEY');
  return secretKey;
}

async function stripeRequest(secretKey, method, endpoint, params = null) {
  const url = `https://api.stripe.com/v1/${endpoint}`;
  const init = {
    method,
    headers: {
      authorization: `Bearer ${secretKey}`,
    },
  };
  if (params !== null) {
    init.headers['content-type'] = 'application/x-www-form-urlencoded';
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
    const err = new Error(value.error?.message ?? `${endpoint} returned ${response.status}`);
    err.status = response.status;
    err.param = value.error?.param;
    err.code = value.error?.code;
    err.type = value.error?.type;
    throw err;
  }
  return value;
}

const stripePost = (secretKey, endpoint, params) => stripeRequest(secretKey, 'POST', endpoint, params);
const stripeGet = (secretKey, endpoint) => stripeRequest(secretKey, 'GET', endpoint);

async function createConnectedAccount(secretKey, tag, businessUrl, currency) {
  if (currency === 'eur') return createDeConnectedAccount(secretKey, tag, businessUrl);
  return createUsConnectedAccount(secretKey, tag, businessUrl);
}

async function createUsConnectedAccount(secretKey, tag, businessUrl) {
  const now = Math.floor(Date.now() / 1000);
  const account = await stripePost(secretKey, 'accounts', {
    type: 'custom',
    country: 'US',
    email: `mayhem-connect-${tag}@trac.network`,
    business_type: 'individual',
    'business_profile[mcc]': '5734',
    'business_profile[url]': businessUrl,
    'capabilities[card_payments][requested]': 'true',
    'capabilities[transfers][requested]': 'true',
    'individual[first_name]': 'Mayhem',
    'individual[last_name]': 'Provider',
    'individual[email]': `mayhem-provider-${tag}@trac.network`,
    'individual[phone]': '+14155550123',
    'individual[dob][day]': '1',
    'individual[dob][month]': '1',
    'individual[dob][year]': '1901',
    'individual[address][line1]': 'address_full_match',
    'individual[address][city]': 'San Francisco',
    'individual[address][state]': 'CA',
    'individual[address][postal_code]': '94107',
    'individual[address][country]': 'US',
    'individual[id_number]': '000000000',
    'individual[ssn_last_4]': '0000',
    'tos_acceptance[date]': String(now),
    'tos_acceptance[ip]': '127.0.0.1',
    external_account: 'btok_us_verified',
    'metadata[mayhem_smoke]': tag,
  });
  const full = await waitConnectedAccountReady(secretKey, account.id);
  return full;
}

async function createDeConnectedAccount(secretKey, tag, businessUrl) {
  const now = Math.floor(Date.now() / 1000);
  const account = await stripePost(secretKey, 'accounts', {
    type: 'custom',
    country: 'DE',
    email: `mayhem-connect-${tag}@trac.network`,
    business_type: 'individual',
    'business_profile[mcc]': '5734',
    'business_profile[url]': businessUrl,
    'capabilities[card_payments][requested]': 'true',
    'capabilities[transfers][requested]': 'true',
    'individual[first_name]': 'Mayhem',
    'individual[last_name]': 'Provider',
    'individual[email]': `mayhem-provider-${tag}@trac.network`,
    'individual[phone]': '+4915112345678',
    'individual[dob][day]': '1',
    'individual[dob][month]': '1',
    'individual[dob][year]': '1901',
    'individual[address][line1]': 'address_full_match',
    'individual[address][city]': 'Berlin',
    'individual[address][postal_code]': '10115',
    'individual[address][country]': 'DE',
    'tos_acceptance[date]': String(now),
    'tos_acceptance[ip]': '127.0.0.1',
    external_account: 'btok_de',
    'metadata[mayhem_smoke]': tag,
  });
  const full = await waitConnectedAccountReady(secretKey, account.id);
  return full;
}

async function waitConnectedAccountReady(secretKey, accountId) {
  const deadline = Date.now() + CONNECT_READY_TIMEOUT_MS;
  let full = null;
  while (Date.now() < deadline) {
    full = await stripeGet(secretKey, `accounts/${accountId}`);
    if (
      full.capabilities?.card_payments === 'active'
      && full.capabilities?.transfers === 'active'
      && full.charges_enabled === true
      && full.payouts_enabled === true
      && (!Array.isArray(full.requirements?.currently_due) || full.requirements.currently_due.length === 0)
    ) {
      return full;
    }
    await sleep(1_000);
  }
  full = full ?? await stripeGet(secretKey, `accounts/${accountId}`);
  if (full.capabilities?.card_payments !== 'active' || full.capabilities?.transfers !== 'active') {
    throw new Error(`Stripe connected account capabilities not active: ${JSON.stringify(full.capabilities)}`);
  }
  if (full.charges_enabled !== true || full.payouts_enabled !== true) {
    throw new Error('Stripe connected account charges/payouts are not enabled');
  }
  if (Array.isArray(full.requirements?.currently_due) && full.requirements.currently_due.length > 0) {
    throw new Error(`Stripe connected account still has requirements: ${full.requirements.currently_due.join(',')}`);
  }
  return full;
}

async function createDestinationCharge(secretKey, accountId, tag, currency, amountCents, feeBps, mu) {
  const feeCents = Math.floor((amountCents * feeBps) / 10_000);
  const providerNetCents = amountCents - feeCents;
  if (providerNetCents <= 0) {
    throw new Error('operator fee leaves no provider payout amount');
  }
  const params = {
    amount: String(amountCents),
    currency,
    payment_method: 'pm_card_visa',
    confirm: 'true',
    'payment_method_types[]': 'card',
    on_behalf_of: accountId,
    'transfer_data[destination]': accountId,
    'metadata[mayhem_smoke]': tag,
    'metadata[mayhem_denom]': 'mu_usd',
    'metadata[mayhem_mu]': String(mu),
    'metadata[mayhem_fiat_currency]': currency,
    'metadata[mayhem_fiat_amount_minor]': String(amountCents),
    'metadata[mayhem_fee_bps]': String(feeBps),
    'metadata[mayhem_operator_fee_minor]': String(feeCents),
    'metadata[mayhem_provider_net_minor]': String(providerNetCents),
  };
  if (feeCents > 0) {
    params.application_fee_amount = String(feeCents);
  }
  const intent = await stripePost(secretKey, 'payment_intents', params);
  const chargeId = typeof intent.latest_charge === 'string' ? intent.latest_charge : intent.latest_charge?.id;
  if (!chargeId) throw new Error(`Stripe PaymentIntent ${intent.id} did not expose latest_charge`);
  const charge = await waitChargeTransferReady(secretKey, chargeId);
  const transferId = typeof charge.transfer === 'string' ? charge.transfer : charge.transfer?.id;
  if (!transferId) throw new Error(`Stripe charge ${chargeId} did not expose transfer id`);
  const transfer = await stripeGet(secretKey, `transfers/${transferId}`);
  if (intent.currency !== currency || intent.amount !== amountCents) {
    throw new Error(`Stripe PaymentIntent was not ${currency.toUpperCase()} ${amountCents}`);
  }
  if (charge.currency !== currency || charge.amount !== amountCents) {
    throw new Error(`Stripe charge was not ${currency.toUpperCase()} ${amountCents}`);
  }
  if (charge.on_behalf_of !== accountId || charge.destination !== accountId) {
    throw new Error('Stripe charge was not settled on behalf of the connected account');
  }
  const chargeApplicationFeeAmount = charge.application_fee_amount ?? intent.application_fee_amount ?? 0;
  if (chargeApplicationFeeAmount !== feeCents) {
    throw new Error(`Stripe application fee was ${chargeApplicationFeeAmount}, expected ${feeCents}`);
  }
  if (charge.balance_transaction?.currency !== currency) {
    throw new Error(`Stripe balance transaction was not ${currency.toUpperCase()}: ${charge.balance_transaction?.currency}`);
  }
  if (transfer.currency !== currency || transfer.amount !== amountCents || transfer.destination !== accountId) {
    throw new Error(`Stripe transfer did not match expected ${currency.toUpperCase()} destination: ${transfer.id}`);
  }
  const applicationFeeId = typeof charge.application_fee === 'string' ? charge.application_fee : charge.application_fee?.id;
  const applicationFee = applicationFeeId
    ? (typeof charge.application_fee === 'object' ? charge.application_fee : await stripeGet(secretKey, `application_fees/${applicationFeeId}`))
    : null;
  if (feeCents > 0) {
    if (!applicationFee) throw new Error('Stripe charge did not expose an application fee object');
    if (applicationFee.amount !== feeCents || applicationFee.currency !== currency) {
      throw new Error(`Stripe application fee did not match ${currency.toUpperCase()} ${feeCents}`);
    }
  }
  return { intent, charge, transfer, applicationFee, feeCents, providerNetCents };
}

async function waitChargeTransferReady(secretKey, chargeId) {
  const deadline = Date.now() + CHARGE_TRANSFER_TIMEOUT_MS;
  let charge = null;
  while (Date.now() < deadline) {
    charge = await stripeGet(
      secretKey,
      `charges/${chargeId}?expand[]=balance_transaction&expand[]=transfer&expand[]=application_fee`
    );
    const transferId = typeof charge.transfer === 'string' ? charge.transfer : charge.transfer?.id;
    if (transferId) return charge;
    await sleep(1_000);
  }
  return charge ?? await stripeGet(
    secretKey,
    `charges/${chargeId}?expand[]=balance_transaction&expand[]=transfer&expand[]=application_fee`
  );
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const secretKey = await loadStripeSecret(args.stripeEnvFile);
  const localRoot = path.resolve(SCRIPT_DIR, '..', '.mayhem-local');
  await mkdir(localRoot, { recursive: true });
  const tempDir = await mkdtemp(path.join(localRoot, 'stripe-sandbox-payout-smoke-'));
  try {
    const tag = `p7-${Date.now().toString(36)}-${randomBytes(3).toString('hex')}`;
    const account = await createConnectedAccount(secretKey, tag, args.businessUrl, args.currency);
    const stripe = await createDestinationCharge(
      secretKey,
      account.id,
      tag,
      args.currency,
      args.amountCents,
      args.feeBps,
      args.mu
    );
    const report = {
      ok: true,
      stripe: {
        connected_account: account.id,
        country: account.country,
        default_currency: account.default_currency,
        card_payments: account.capabilities?.card_payments,
        transfers: account.capabilities?.transfers,
        charges_enabled: account.charges_enabled,
        payouts_enabled: account.payouts_enabled,
        payment_intent_id: stripe.intent.id,
        charge_id: stripe.charge.id,
        charge_currency: stripe.charge.currency,
        charge_amount_cents: stripe.charge.amount,
        on_behalf_of: stripe.charge.on_behalf_of,
        destination: stripe.charge.destination,
        balance_transaction_currency: stripe.charge.balance_transaction?.currency,
        transfer_id: stripe.transfer.id,
        transfer_currency: stripe.transfer.currency,
        transfer_amount_cents: stripe.transfer.amount,
        transfer_destination: stripe.transfer.destination,
        application_fee_id: stripe.applicationFee?.id ?? null,
        application_fee_currency: stripe.applicationFee?.currency ?? null,
        application_fee_amount_cents: stripe.applicationFee?.amount ?? stripe.feeCents,
      },
      ledger: {
        payout_confirm_retired: true,
        evidence_model: 'epoch_settlement_report',
        mu_usd: args.mu,
        fiat_currency: args.currency,
        gross_amount_minor: args.amountCents,
        fee_bps: args.feeBps,
        operator_fee_minor: stripe.feeCents,
        provider_net_minor: stripe.providerNetCents,
      },
      temp_dir: args.keepTemp ? tempDir : undefined,
    };
    await writeFile(path.join(tempDir, 'report.json'), `${JSON.stringify(report, null, 2)}\n`);
    process.stdout.write(`${JSON.stringify(report, null, args.json ? 0 : 2)}\n`);
  } finally {
    if (!args.keepTemp) await rm(tempDir, { recursive: true, force: true });
  }
}

main().catch((err) => {
  const detail = [
    err.message,
    err.param ? `param=${err.param}` : null,
    err.code ? `code=${err.code}` : null,
  ].filter(Boolean).join(' ');
  console.error(`stripe sandbox payout smoke failed: ${detail}`);
  process.exitCode = 1;
});

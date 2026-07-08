#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { boolArg, parseArgs, sleep } from './msb-local-common.mjs';

const scriptPath = fileURLToPath(import.meta.url);

export const DEFAULT_INTERVAL_MS = 30 * 60 * 1000;
export const GATE_TICKER_URL = 'https://api.gateio.ws/api/v4/spot/tickers?currency_pair=TNK_USDT';
export const MEXC_TICKER_URL = 'https://api.mexc.com/api/v3/ticker/price?symbol=TNKUSDT';
const USD_AU = 1_000_000_000_000_000_000n;

function shellQuote(value) {
  const raw = String(value ?? '');
  if (raw.length === 0) return "''";
  return `'${raw.replaceAll("'", "'\\''")}'`;
}

export function decimalUsdToAu(value) {
  const raw = String(value ?? '').trim();
  if (!/^\d+(\.\d+)?$/.test(raw)) {
    throw new Error(`Invalid decimal USD price: ${raw}`);
  }
  const [whole, fraction = ''] = raw.split('.');
  const nineteenDigits = `${fraction}0000000000000000000`.slice(0, 19);
  const atto = BigInt(whole) * USD_AU + BigInt(nineteenDigits.slice(0, 18));
  const rounded = atto + (Number(nineteenDigits[18]) >= 5 ? 1n : 0n);
  if (rounded <= 0n) {
    throw new Error(`USD price outside positive atto range: ${raw}`);
  }
  return rounded.toString();
}

export function parseGateTicker(payload) {
  const ticker = Array.isArray(payload) ? payload[0] : payload;
  const price = ticker?.last ?? ticker?.price ?? ticker?.last_price;
  if (price === undefined || price === null) {
    throw new Error('Gate ticker response missing last price');
  }
  return {
    source: 'gate-spot',
    raw_price: String(price),
    tnk_usd_au: decimalUsdToAu(price),
  };
}

export function parseMexcTicker(payload) {
  const ticker = Array.isArray(payload)
    ? payload.find((item) => String(item?.symbol ?? '').toUpperCase() === 'TNKUSDT') ?? payload[0]
    : payload;
  const price = ticker?.price ?? ticker?.lastPrice ?? ticker?.last;
  if (price === undefined || price === null) {
    throw new Error('MEXC ticker response missing price');
  }
  return {
    source: 'mexc-spot',
    raw_price: String(price),
    tnk_usd_au: decimalUsdToAu(price),
  };
}

export async function fetchJson(url, fetchImpl = globalThis.fetch) {
  if (typeof fetchImpl !== 'function') throw new Error('fetch is unavailable');
  const response = await fetchImpl(url);
  if (!response?.ok) {
    throw new Error(`GET ${url} failed with ${response?.status ?? 'unknown status'}`);
  }
  return await response.json();
}

function ensureRpcBase(url) {
  const raw = String(url ?? '').trim();
  if (!raw) throw new Error('Missing peer RPC URL');
  return raw.endsWith('/') ? raw : `${raw}/`;
}

async function readContractStateValue(rpcUrl, key, {
  confirmed = false,
  fetchImpl = globalThis.fetch,
} = {}) {
  const url = new URL('state', ensureRpcBase(rpcUrl));
  url.searchParams.set('key', key);
  url.searchParams.set('confirmed', confirmed ? 'true' : 'false');
  const body = await fetchJson(url, fetchImpl);
  return body?.value ?? null;
}

export function rateStateMatches(rate, value) {
  return value?.denom === 'tnk_usd_au'
    && value?.tnk_usd_au === rate.tnk_usd_au
    && value?.source === rate.source
    && value?.ts === rate.ts
    && value?.posted_by_role === 'admin';
}

export async function waitForRateState(rate, {
  rpcUrl,
  timeoutMs = 30_000,
  pollMs = 500,
  fetchImpl = globalThis.fetch,
} = {}) {
  const deadline = Date.now() + timeoutMs;
  let last = null;
  while (Date.now() <= deadline) {
    last = await readContractStateValue(rpcUrl, 'rate/latest', { fetchImpl });
    if (rateStateMatches(rate, last)) {
      return { verified: true, state: last };
    }
    await sleep(pollMs);
  }
  return { verified: false, state: last };
}

export async function resolveTnkUsdRate({
  gateUrl = GATE_TICKER_URL,
  mexcUrl = MEXC_TICKER_URL,
  fetchImpl = globalThis.fetch,
  nowSeconds = () => Math.floor(Date.now() / 1000),
} = {}) {
  const failures = [];
  try {
    const payload = await fetchJson(gateUrl, fetchImpl);
    return {
      ...parseGateTicker(payload),
      ts: nowSeconds(),
      primary: true,
      failures,
    };
  } catch (error) {
    failures.push({ source: 'gate-spot', error: error?.message || String(error) });
  }

  try {
    const payload = await fetchJson(mexcUrl, fetchImpl);
    return {
      ...parseMexcTicker(payload),
      ts: nowSeconds(),
      primary: false,
      failures,
    };
  } catch (error) {
    failures.push({ source: 'mexc-spot', error: error?.message || String(error) });
  }

  throw new Error(`No TNK/USDT source returned a usable price: ${JSON.stringify(failures)}`);
}

export function buildAdminCommandArgs(rate, {
  submit = true,
  sim = false,
  json = true,
  rpcUrl,
  home,
  peerStoreName,
  walletPassword,
} = {}) {
  const args = [
    'admin',
    'rate-oracle',
    '--tnk-usd-au',
    String(rate.tnk_usd_au),
    '--source',
    rate.source,
    '--ts',
    String(rate.ts),
  ];
  if (home) args.push('--home', home);
  if (rpcUrl) args.push('--rpc-url', rpcUrl);
  if (peerStoreName) args.push('--peer-store-name', peerStoreName);
  if (walletPassword) args.push('--wallet-password', walletPassword);
  if (submit) args.push('--submit');
  if (sim) args.push('--sim');
  if (json) args.push('--json');
  return args;
}

export function buildAdminCommand(rate, options = {}) {
  const bin = options.mayhemBin ?? 'mayhem';
  const args = buildAdminCommandArgs(rate, options)
    .filter((arg, index, all) => all[index - 1] !== '--wallet-password' && arg !== '--wallet-password');
  return [bin, ...args].map(shellQuote).join(' ');
}

export async function runOnce(options = {}) {
  const rate = await resolveTnkUsdRate(options);
  const adminOptions = {
    submit: true,
    sim: options.sim,
    json: true,
    mayhemBin: options.mayhemBin ?? 'mayhem',
    rpcUrl: options.rpcUrl,
    home: options.home,
    peerStoreName: options.peerStoreName,
    walletPassword: options.walletPassword,
  };
  const report = {
    ok: true,
    submitted: false,
    verified: false,
    source: rate.source,
    primary: rate.primary,
    raw_price: rate.raw_price,
    tnk_usd_au: rate.tnk_usd_au,
    ts: rate.ts,
    failures: rate.failures,
    copy_paste_admin_submit_command: buildAdminCommand(rate, adminOptions),
  };

  if (options.submit) {
    const spawnImpl = options.spawnImpl ?? spawnSync;
    const child = spawnImpl(
      adminOptions.mayhemBin,
      buildAdminCommandArgs(rate, adminOptions),
      { encoding: 'utf8' }
    );
    report.submitted = child.status === 0;
    report.exit_status = child.status;
    report.stdout = child.stdout?.trim() || null;
    report.stderr = child.stderr?.trim() || null;
    if (child.status !== 0) {
      report.ok = false;
      report.error = `mayhem admin rate-oracle exited ${child.status}`;
    } else if (adminOptions.rpcUrl && options.verify !== false && !adminOptions.sim) {
      const verification = await waitForRateState(rate, {
        rpcUrl: adminOptions.rpcUrl,
        timeoutMs: options.verifyTimeoutMs,
        pollMs: options.verifyPollMs,
        fetchImpl: options.fetchImpl,
      });
      report.verified = verification.verified;
      report.rate_state = verification.state;
      if (!verification.verified) {
        report.ok = false;
        report.error = 'rateOracle submit did not update contract rate/latest with matching admin evidence';
      }
    } else {
      report.verified = child.status === 0 && adminOptions.sim;
    }
  }

  return report;
}

function parsePositiveInt(value, label, fallback = null) {
  if (value === undefined || value === null || value === '') {
    if (fallback !== null) return fallback;
    throw new Error(`Missing ${label}`);
  }
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed <= 0) {
    throw new Error(`${label} must be a positive safe integer`);
  }
  return parsed;
}

async function main() {
  const args = parseArgs();
  const intervalMs = parsePositiveInt(args['interval-ms'] ?? DEFAULT_INTERVAL_MS, '--interval-ms', DEFAULT_INTERVAL_MS);
  const once = boolArg(args.once, false);
  const json = boolArg(args.json, false);
  const submit = boolArg(args.submit, false);
  const options = {
    gateUrl: args['gate-url'] || process.env.MAYHEM_TNK_GATE_TICKER_URL || GATE_TICKER_URL,
    mexcUrl: args['mexc-url'] || process.env.MAYHEM_TNK_MEXC_TICKER_URL || MEXC_TICKER_URL,
    submit,
    sim: boolArg(args.sim, false),
    mayhemBin: args['mayhem-bin'] || 'mayhem',
    rpcUrl: args['admin-rpc-url'] || args['peer-rpc'] || process.env.MAYHEM_PEER_RPC,
    home: args['admin-home'],
    peerStoreName: args['admin-peer-store-name'],
    walletPassword: args['admin-wallet-password-env']
      ? process.env[String(args['admin-wallet-password-env'])]
      : undefined,
    verify: !boolArg(args['no-verify'], false),
    verifyTimeoutMs: parsePositiveInt(args['verify-timeout-ms'] ?? 30_000, '--verify-timeout-ms', 30_000),
    verifyPollMs: parsePositiveInt(args['verify-poll-ms'] ?? 500, '--verify-poll-ms', 500),
  };

  do {
    const report = await runOnce(options);
    if (json) {
      console.log(JSON.stringify(report, null, 2));
    } else {
      console.log('[tnk:rate] TNK/USD oracle tick complete');
      console.log('[tnk:rate] source:', report.source);
      console.log('[tnk:rate] raw USDT price:', report.raw_price);
      console.log('[tnk:rate] tnk_usd_au:', report.tnk_usd_au);
      console.log('[tnk:rate] submitted:', report.submitted);
      console.log('Copy/paste admin TNK rate oracle submit command:');
      console.log(report.copy_paste_admin_submit_command);
      for (const failure of report.failures ?? []) {
        console.log(`[tnk:rate] fallback note: ${failure.source}: ${failure.error}`);
      }
    }
    if (!report.ok) process.exitCode = 2;
    if (once) break;
    await sleep(intervalMs);
  } while (true);
}

if (process.argv[1] && path.resolve(process.argv[1]) === scriptPath) {
  main().catch((error) => {
    console.error(error?.stack || error?.message || String(error));
    process.exit(1);
  });
}

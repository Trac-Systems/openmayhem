#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { ethers } from 'ethers';

const scriptPath = fileURLToPath(import.meta.url);
const USD_E6 = 1_000_000n;

export const DEFAULT_INTERVAL_MS = 30 * 60 * 1000;
export const DEFAULT_CACHE_TTL_MS = 30 * 60 * 1000;
export const DEFAULT_RPC_TIMEOUT_MS = 4_000;
export const DEFAULT_TAP_USDT_POOL = '0x1563e9af51616e78830de3325da752de369c1714';
export const DEFAULT_USDT_ADDRESS = '0xdac17f958d2ee523a2206206994597c13d831ec7';
export const DEFAULT_MAINNET_CHAIN_ID = 1;
export const UNIV2_POOL_ABI = [
  'function token0() view returns (address)',
  'function token1() view returns (address)',
  'function getReserves() view returns (uint112 reserve0, uint112 reserve1, uint32 blockTimestampLast)',
];

let cache = null;
let inFlight = null;

function shellQuote(value) {
  const raw = String(value ?? '');
  if (raw.length === 0) return "''";
  return `'${raw.replaceAll("'", "'\\''")}'`;
}

function parseArgs(argv = process.argv.slice(2)) {
  const out = {};
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (!arg.startsWith('--')) continue;
    const key = arg.slice(2);
    const next = argv[i + 1];
    if (next === undefined || next.startsWith('--')) {
      out[key] = true;
    } else {
      out[key] = next;
      i += 1;
    }
  }
  return out;
}

function boolArg(value, fallback = false) {
  if (value === undefined) return fallback;
  if (value === true) return true;
  const text = String(value).trim().toLowerCase();
  return ['1', 'true', 'yes', 'on'].includes(text);
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function parseNonNegativeInt(value, label, fallback = null) {
  if (value === undefined || value === null || value === '') {
    if (fallback !== null) return fallback;
    throw new Error(`Missing ${label}`);
  }
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < 0) {
    throw new Error(`${label} must be a non-negative safe integer`);
  }
  return parsed;
}

function parsePositiveInt(value, label, fallback = null) {
  const parsed = parseNonNegativeInt(value, label, fallback);
  if (parsed <= 0) throw new Error(`${label} must be positive`);
  return parsed;
}

function pow10(exp) {
  const parsed = parseNonNegativeInt(exp, 'decimal exponent');
  return 10n ** BigInt(parsed);
}

function toPositiveBigInt(value, label) {
  try {
    const parsed = BigInt(String(value ?? '').trim());
    if (parsed <= 0n) throw new Error();
    return parsed;
  } catch (_error) {
    throw new Error(`${label} must be a positive integer`);
  }
}

function toSafePositiveNumber(value, label) {
  if (value <= 0n || value > BigInt(Number.MAX_SAFE_INTEGER)) {
    throw new Error(`${label} outside safe integer range`);
  }
  return Number(value);
}

function normalizeAddress(value, label) {
  try {
    return ethers.getAddress(String(value ?? '')).toLowerCase();
  } catch (_error) {
    throw new Error(`${label} must be an Ethereum address`);
  }
}

function optionalAddress(value, label) {
  if (value === undefined || value === null || value === '') return null;
  return normalizeAddress(value, label);
}

function withTimeout(promise, timeoutMs, label) {
  const ms = parsePositiveInt(timeoutMs, '--rpc-timeout-ms', DEFAULT_RPC_TIMEOUT_MS);
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`${label} timed out after ${ms}ms`)), ms);
    Promise.resolve(promise).then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (error) => {
        clearTimeout(timer);
        reject(error);
      }
    );
  });
}

export function decimalUsdToE6(value) {
  const raw = String(value ?? '').trim();
  if (!/^\d+(\.\d+)?$/.test(raw)) {
    throw new Error(`Invalid decimal USD price: ${raw}`);
  }
  const [whole, fraction = ''] = raw.split('.');
  const sevenDigits = `${fraction}0000000`.slice(0, 7);
  const micros = BigInt(whole) * USD_E6 + BigInt(sevenDigits.slice(0, 6));
  const rounded = micros + (Number(sevenDigits[6]) >= 5 ? 1n : 0n);
  return toSafePositiveNumber(rounded, 'USD price');
}

export function parsePinnedTapUsdE6({
  tapUsdE6,
  tapUsd,
  env = process.env,
} = {}) {
  const e6 = tapUsdE6
    ?? env.MAYHEM_TAP_USD_E6
    ?? env.TAP_USD_E6;
  if (e6 !== undefined && e6 !== null && e6 !== '') {
    return parsePositiveInt(e6, 'TAP_USD_E6');
  }
  const decimal = tapUsd
    ?? env.MAYHEM_TAP_USD
    ?? env.TAP_USD;
  if (decimal !== undefined && decimal !== null && decimal !== '') {
    return decimalUsdToE6(decimal);
  }
  return null;
}

export function tapUsdE6FromReserves({
  tapReserve,
  usdtReserve,
  tapDecimals = 18,
  usdtDecimals = 6,
} = {}) {
  const tap = toPositiveBigInt(tapReserve, 'tap reserve');
  const usdt = toPositiveBigInt(usdtReserve, 'USDT reserve');
  const numerator = usdt * pow10(tapDecimals) * USD_E6;
  const denominator = tap * pow10(usdtDecimals);
  return toSafePositiveNumber(numerator / denominator, 'TAP/USD e6 price');
}

function reserveAt(reserves, index, name) {
  const byName = reserves?.[name];
  const byIndex = reserves?.[index];
  return byName ?? byIndex;
}

export async function readTapUsdE6FromDex({
  provider,
  poolAddress = DEFAULT_TAP_USDT_POOL,
  tapAddress,
  usdtAddress = DEFAULT_USDT_ADDRESS,
  tapDecimals = 18,
  usdtDecimals = 6,
  timeoutMs = DEFAULT_RPC_TIMEOUT_MS,
  poolFactory = (address, abi, providerArg) => new ethers.Contract(address, abi, providerArg),
} = {}) {
  if (!provider) throw new Error('Missing Ethereum RPC provider');
  const pool = poolFactory(poolAddress, UNIV2_POOL_ABI, provider);
  const usdt = normalizeAddress(usdtAddress, 'USDT address');
  const tap = optionalAddress(tapAddress, 'TAP address');
  const [token0, token1, reserves] = await Promise.all([
    withTimeout(pool.token0(), timeoutMs, 'pool.token0'),
    withTimeout(pool.token1(), timeoutMs, 'pool.token1'),
    withTimeout(pool.getReserves(), timeoutMs, 'pool.getReserves'),
  ]);
  const token0Lc = normalizeAddress(token0, 'pool token0');
  const token1Lc = normalizeAddress(token1, 'pool token1');
  const reserve0 = reserveAt(reserves, 0, 'reserve0');
  const reserve1 = reserveAt(reserves, 1, 'reserve1');

  let tapReserve;
  let usdtReserve;
  if (tap) {
    if (token0Lc === tap && token1Lc === usdt) {
      tapReserve = reserve0;
      usdtReserve = reserve1;
    } else if (token1Lc === tap && token0Lc === usdt) {
      tapReserve = reserve1;
      usdtReserve = reserve0;
    } else {
      throw new Error('Configured TAP/USDT pool token ordering does not match the provided token addresses');
    }
  } else if (token0Lc === usdt) {
    tapReserve = reserve1;
    usdtReserve = reserve0;
  } else if (token1Lc === usdt) {
    tapReserve = reserve0;
    usdtReserve = reserve1;
  } else {
    throw new Error('Configured pool is not paired with USDT and no TAP address was provided');
  }

  return {
    tap_usd_e6: tapUsdE6FromReserves({
      tapReserve,
      usdtReserve,
      tapDecimals,
      usdtDecimals,
    }),
    pool_address: normalizeAddress(poolAddress, 'pool address'),
    token0: token0Lc,
    token1: token1Lc,
    tap_reserve: toPositiveBigInt(tapReserve, 'tap reserve').toString(),
    usdt_reserve: toPositiveBigInt(usdtReserve, 'USDT reserve').toString(),
  };
}

function cachedRateReport(nowMs, ttlMs) {
  return cache && nowMs - cache.cached_at_ms < ttlMs
    ? { ...cache, cache_hit: true, ttl_ms: ttlMs }
    : null;
}

function cacheRate(rate, nowMs) {
  cache = { ...rate, cached_at_ms: nowMs };
  return { ...cache };
}

export async function resolveTapUsdRate({
  rpcUrl,
  chainId,
  poolAddress = DEFAULT_TAP_USDT_POOL,
  tapAddress,
  usdtAddress = DEFAULT_USDT_ADDRESS,
  fallbackUsd,
  fallbackUsdE6,
  env = process.env,
  ttlMs = DEFAULT_CACHE_TTL_MS,
  timeoutMs = DEFAULT_RPC_TIMEOUT_MS,
  providerFactory = (url) => new ethers.JsonRpcProvider(url),
  poolFactory,
  nowMs = () => Date.now(),
  nowSeconds = () => Math.floor(Date.now() / 1000),
} = {}) {
  const ttl = parsePositiveInt(ttlMs, '--cache-ttl-ms', DEFAULT_CACHE_TTL_MS);
  const now = nowMs();
  const cached = cachedRateReport(now, ttl);
  if (cached) return cached;

  if (inFlight) return inFlight;

  inFlight = (async () => {
    const fallback = parsePinnedTapUsdE6({ tapUsdE6: fallbackUsdE6, tapUsd: fallbackUsd, env });
    const normalizedChainId = chainId === undefined || chainId === null || chainId === ''
      ? 0
      : parseNonNegativeInt(chainId, '--chain-id');
    const failures = [];

    if (!rpcUrl || normalizedChainId !== DEFAULT_MAINNET_CHAIN_ID) {
      if (fallback === null) throw new Error('Missing pinned TAP_USD fallback for non-mainnet or RPC-less price resolution');
      return cacheRate({
        source: 'config',
        tap_usd_e6: fallback,
        ts: nowSeconds(),
        failures,
      }, now);
    }

    try {
      const provider = providerFactory(rpcUrl);
      const dex = await readTapUsdE6FromDex({
        provider,
        poolAddress,
        tapAddress,
        usdtAddress,
        timeoutMs,
        poolFactory,
      });
      return cacheRate({
        source: 'uniswap-v2',
        tap_usd_e6: dex.tap_usd_e6,
        ts: nowSeconds(),
        pool_address: dex.pool_address,
        tap_reserve: dex.tap_reserve,
        usdt_reserve: dex.usdt_reserve,
        failures,
      }, now);
    } catch (error) {
      failures.push({ source: 'uniswap-v2', error: error?.message || String(error) });
      if (fallback !== null) {
        return cacheRate({
          source: 'config',
          tap_usd_e6: fallback,
          ts: nowSeconds(),
          failures,
        }, now);
      }
      if (cache) {
        return cacheRate({
          source: 'stale',
          tap_usd_e6: cache.tap_usd_e6,
          ts: nowSeconds(),
          stale_from_ts: cache.ts,
          failures,
        }, now);
      }
      throw new Error(`No TAP/USD source returned a usable price: ${JSON.stringify(failures)}`);
    }
  })().finally(() => {
    inFlight = null;
  });

  return inFlight;
}

function ensureRpcBase(url) {
  const raw = String(url ?? '').trim();
  if (!raw) throw new Error('Missing peer RPC URL');
  return raw.endsWith('/') ? raw : `${raw}/`;
}

async function fetchJson(url, fetchImpl = globalThis.fetch) {
  if (typeof fetchImpl !== 'function') throw new Error('fetch is unavailable');
  const response = await fetchImpl(url);
  if (!response?.ok) {
    throw new Error(`GET ${url} failed with ${response?.status ?? 'unknown status'}`);
  }
  return await response.json();
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

export function tapRateStateMatches(rate, value) {
  return value?.denom === 'tap_usd_e6'
    && value?.tap_usd_e6 === rate.tap_usd_e6
    && value?.source === rate.source
    && value?.ts === rate.ts
    && value?.posted_by_role === 'admin';
}

export async function waitForTapRateState(rate, {
  rpcUrl,
  timeoutMs = 30_000,
  pollMs = 500,
  fetchImpl = globalThis.fetch,
} = {}) {
  const deadline = Date.now() + timeoutMs;
  let last = null;
  while (Date.now() <= deadline) {
    last = await readContractStateValue(rpcUrl, 'tap/rate/latest', { fetchImpl });
    if (tapRateStateMatches(rate, last)) {
      return { verified: true, state: last };
    }
    await sleep(pollMs);
  }
  return { verified: false, state: last };
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
    'tap-rate-oracle',
    '--tap-usd-e6',
    String(rate.tap_usd_e6),
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
  const rate = await resolveTapUsdRate({
    ...options,
    rpcUrl: options.priceRpcUrl ?? options.ethRpcUrl ?? options.rpcUrl,
  });
  const adminOptions = {
    submit: true,
    sim: options.sim,
    json: true,
    mayhemBin: options.mayhemBin ?? 'mayhem',
    rpcUrl: options.adminRpcUrl ?? options.peerRpcUrl,
    home: options.home,
    peerStoreName: options.peerStoreName,
    walletPassword: options.walletPassword,
  };
  const report = {
    ok: true,
    submitted: false,
    verified: false,
    source: rate.source,
    tap_usd_e6: rate.tap_usd_e6,
    ts: rate.ts,
    pool_address: rate.pool_address ?? null,
    tap_reserve: rate.tap_reserve ?? null,
    usdt_reserve: rate.usdt_reserve ?? null,
    stale_from_ts: rate.stale_from_ts ?? null,
    cache_hit: Boolean(rate.cache_hit),
    failures: rate.failures ?? [],
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
      report.error = `mayhem admin tap-rate-oracle exited ${child.status}`;
    } else if (adminOptions.rpcUrl && options.verify !== false && !adminOptions.sim) {
      const verification = await waitForTapRateState(rate, {
        rpcUrl: adminOptions.rpcUrl,
        timeoutMs: options.verifyTimeoutMs,
        pollMs: options.verifyPollMs,
        fetchImpl: options.fetchImpl,
      });
      report.verified = verification.verified;
      report.rate_state = verification.state;
      if (!verification.verified) {
        report.ok = false;
        report.error = 'tapRateOracle submit did not update contract tap/rate/latest with matching admin evidence';
      }
    } else {
      report.verified = child.status === 0 && adminOptions.sim;
    }
  }

  return report;
}

export function resetTapPriceCacheForTest() {
  cache = null;
  inFlight = null;
}

async function main() {
  const args = parseArgs();
  const ethRpc = args['eth-rpc'] || args.rpc || process.env.MAYHEM_TAP_ETH_RPC || process.env.ETH_RPC;
  const adminRpcUrl = args['admin-rpc-url'] || args['peer-rpc'] || process.env.MAYHEM_PEER_RPC;
  const chainId = args['chain-id'] ?? process.env.MAYHEM_TAP_ETH_CHAIN_ID ?? (ethRpc ? DEFAULT_MAINNET_CHAIN_ID : 0);
  const intervalMs = parsePositiveInt(args['interval-ms'] ?? DEFAULT_INTERVAL_MS, '--interval-ms', DEFAULT_INTERVAL_MS);
  const once = boolArg(args.once, false);
  const json = boolArg(args.json, false);
  const submit = boolArg(args.submit, false);
  const options = {
    priceRpcUrl: ethRpc,
    adminRpcUrl,
    chainId,
    poolAddress: args.pool || process.env.MAYHEM_TAP_USDT_POOL || DEFAULT_TAP_USDT_POOL,
    tapAddress: args['tap-token'] || process.env.MAYHEM_TAP_TOKEN_ADDR,
    usdtAddress: args['usdt-token'] || process.env.MAYHEM_USDT_TOKEN_ADDR || DEFAULT_USDT_ADDRESS,
    fallbackUsd: args['tap-usd'],
    fallbackUsdE6: args['tap-usd-e6'],
    env: process.env,
    ttlMs: parsePositiveInt(args['cache-ttl-ms'] ?? DEFAULT_CACHE_TTL_MS, '--cache-ttl-ms', DEFAULT_CACHE_TTL_MS),
    timeoutMs: parsePositiveInt(args['rpc-timeout-ms'] ?? DEFAULT_RPC_TIMEOUT_MS, '--rpc-timeout-ms', DEFAULT_RPC_TIMEOUT_MS),
    submit,
    sim: boolArg(args.sim, false),
    mayhemBin: args['mayhem-bin'] || 'mayhem',
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
      console.log('[tap:rate] TAP/USD oracle tick complete');
      console.log('[tap:rate] source:', report.source);
      console.log('[tap:rate] tap_usd_e6:', report.tap_usd_e6);
      console.log('[tap:rate] submitted:', report.submitted);
      console.log('Copy/paste admin TAP rate oracle submit command:');
      console.log(report.copy_paste_admin_submit_command);
      for (const failure of report.failures ?? []) {
        console.log(`[tap:rate] fallback note: ${failure.source}: ${failure.error}`);
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

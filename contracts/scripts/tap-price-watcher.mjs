#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { ethers } from 'ethers';

const scriptPath = fileURLToPath(import.meta.url);
const USD_AU = 1_000_000_000_000_000_000n;

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

function splitRpcUrls(value) {
  if (value === undefined || value === null || value === '') return [];
  if (Array.isArray(value)) return value.flatMap(splitRpcUrls);
  return String(value)
    .split(/[\s,]+/)
    .map((entry) => entry.trim())
    .filter(Boolean);
}

export function rpcUrlCandidates({ rpcUrl, rpcUrls, fallbackRpcUrls } = {}) {
  const out = [];
  const seen = new Set();
  for (const url of [
    ...splitRpcUrls(rpcUrls),
    ...splitRpcUrls(rpcUrl),
    ...splitRpcUrls(fallbackRpcUrls),
  ]) {
    if (seen.has(url)) continue;
    seen.add(url);
    out.push(url);
  }
  return out;
}

function rpcFailureLabel(url) {
  try {
    const parsed = new URL(url);
    return parsed.hostname;
  } catch (_error) {
    return 'configured-rpc';
  }
}

function rpcErrorMessage(error) {
  return String(error?.message || error || 'unknown error').replace(/https?:\/\/[^\s"'<>]+/g, 'https://<redacted>');
}

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

function parsePositiveDecimalIntegerString(value, label) {
  const raw = String(value ?? '').trim();
  if (!/^\d+$/.test(raw)) {
    throw new Error(`${label} must be a positive integer string`);
  }
  const parsed = BigInt(raw);
  if (parsed <= 0n) throw new Error(`${label} must be positive`);
  return parsed.toString();
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
  if (rounded <= 0n) throw new Error('USD price must be positive');
  return rounded.toString();
}

export function parsePinnedTapUsdAu({
  tapUsdAu,
  tapUsd,
  env = process.env,
} = {}) {
  const au = tapUsdAu
    ?? env.MAYHEM_TAP_USD_AU
    ?? env.TAP_USD_AU;
  if (au !== undefined && au !== null && au !== '') {
    return parsePositiveDecimalIntegerString(au, 'TAP_USD_AU');
  }
  const decimal = tapUsd
    ?? env.MAYHEM_TAP_USD
    ?? env.TAP_USD;
  if (decimal !== undefined && decimal !== null && decimal !== '') {
    return decimalUsdToAu(decimal);
  }
  return null;
}

export function tapUsdAuFromReserves({
  tapReserve,
  usdtReserve,
  tapDecimals = 18,
  usdtDecimals = 6,
} = {}) {
  const tap = toPositiveBigInt(tapReserve, 'tap reserve');
  const usdt = toPositiveBigInt(usdtReserve, 'USDT reserve');
  const numerator = usdt * pow10(tapDecimals) * USD_AU;
  const denominator = tap * pow10(usdtDecimals);
  return (numerator / denominator).toString();
}

function reserveAt(reserves, index, name) {
  const byName = reserves?.[name];
  const byIndex = reserves?.[index];
  return byName ?? byIndex;
}

export async function readTapUsdAuFromDex({
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
    tap_usd_au: tapUsdAuFromReserves({
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
  rpcUrls,
  fallbackRpcUrls,
  chainId,
  poolAddress = DEFAULT_TAP_USDT_POOL,
  tapAddress,
  usdtAddress = DEFAULT_USDT_ADDRESS,
  fallbackUsd,
  fallbackUsdAu,
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
    const fallback = parsePinnedTapUsdAu({ tapUsdAu: fallbackUsdAu, tapUsd: fallbackUsd, env });
    const normalizedChainId = chainId === undefined || chainId === null || chainId === ''
      ? 0
      : parseNonNegativeInt(chainId, '--chain-id');
    const failures = [];
    const candidates = rpcUrlCandidates({ rpcUrl, rpcUrls, fallbackRpcUrls });

    if (candidates.length === 0 || normalizedChainId !== DEFAULT_MAINNET_CHAIN_ID) {
      if (fallback === null) throw new Error('Missing pinned TAP_USD fallback for non-mainnet or RPC-less price resolution');
      return cacheRate({
        source: 'config',
        tap_usd_au: fallback,
        ts: nowSeconds(),
        failures,
      }, now);
    }

    for (const candidate of candidates) {
      try {
        const provider = providerFactory(candidate);
        const dex = await readTapUsdAuFromDex({
          provider,
          poolAddress,
          tapAddress,
          usdtAddress,
          timeoutMs,
          poolFactory,
        });
        return cacheRate({
          source: 'uniswap-v2',
          tap_usd_au: dex.tap_usd_au,
          ts: nowSeconds(),
          pool_address: dex.pool_address,
          tap_reserve: dex.tap_reserve,
          usdt_reserve: dex.usdt_reserve,
          rpc_source: rpcFailureLabel(candidate),
          failures,
        }, now);
      } catch (error) {
        failures.push({
          source: 'uniswap-v2',
          rpc_source: rpcFailureLabel(candidate),
          error: rpcErrorMessage(error),
        });
      }
    }

    if (fallback !== null) {
      return cacheRate({
        source: 'config',
        tap_usd_au: fallback,
        ts: nowSeconds(),
        failures,
      }, now);
    }
    if (cache) {
      return cacheRate({
        source: 'stale',
        tap_usd_au: cache.tap_usd_au,
        ts: nowSeconds(),
        stale_from_ts: cache.ts,
        failures,
      }, now);
    }
    throw new Error(`No TAP/USD source returned a usable price: ${JSON.stringify(failures)}`);
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
  return value?.denom === 'tap_usd_au'
    && value?.tap_usd_au === rate.tap_usd_au
    && value?.source === rate.source
    && value?.ts === rate.ts
    && value?.posted_by_role === 'admin';
}

export async function waitForTapRateState(rate, {
  rpcUrl,
  timeoutMs = 0,
  pollMs = 500,
  fetchImpl = globalThis.fetch,
} = {}) {
  const deadline = timeoutMs > 0 ? Date.now() + timeoutMs : null;
  let last = null;
  while (deadline === null || Date.now() <= deadline) {
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
    '--tap-usd-au',
    String(rate.tap_usd_au),
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
  const chainId = options.chainId === undefined || options.chainId === null || options.chainId === ''
    ? 0
    : parseNonNegativeInt(options.chainId, '--chain-id');
  const rate = await resolveTapUsdRate({
    ...options,
    rpcUrl: options.priceRpcUrl ?? options.ethRpcUrl ?? options.rpcUrl,
    fallbackRpcUrls: options.fallbackPriceRpcUrls ?? options.ethRpcFallbacks,
    chainId,
  });
  if (
    options.requireLiveMainnetPrice
    && chainId === DEFAULT_MAINNET_CHAIN_ID
    && rate.source !== 'uniswap-v2'
  ) {
    throw new Error(`Refusing to post non-mainnet-live TAP price on chain-id 1 (source=${rate.source}). Configure MAYHEM_TAP_ETH_RPC plus QuickNode fallback, or pass --allow-price-fallback only for local/emergency dry runs.`);
  }
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
    tap_usd_au: rate.tap_usd_au,
    ts: rate.ts,
    pool_address: rate.pool_address ?? null,
    tap_reserve: rate.tap_reserve ?? null,
    usdt_reserve: rate.usdt_reserve ?? null,
    rpc_source: rate.rpc_source ?? null,
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
  const ethRpc = args['eth-rpc']
    || args.rpc
    || process.env.MAYHEM_TAP_ETH_RPC
    || process.env.ETH_RPC;
  const ethRpcFallbacks = args['eth-rpc-fallbacks']
    || args['eth-rpc-fallback']
    || process.env.MAYHEM_TAP_ETH_RPC_FALLBACKS
    || process.env.MAYHEM_TAP_ETH_RPC_FALLBACK
    || process.env.ETH_RPC_FALLBACKS;
  const adminRpcUrl = args['admin-rpc-url'] || args['peer-rpc'] || process.env.MAYHEM_PEER_RPC;
  const chainId = args['chain-id'] ?? process.env.MAYHEM_TAP_ETH_CHAIN_ID ?? (ethRpc ? DEFAULT_MAINNET_CHAIN_ID : 0);
  const intervalMs = parsePositiveInt(args['interval-ms'] ?? DEFAULT_INTERVAL_MS, '--interval-ms', DEFAULT_INTERVAL_MS);
  const once = boolArg(args.once, false);
  const json = boolArg(args.json, false);
  const submit = boolArg(args.submit, false);
  const options = {
    priceRpcUrl: ethRpc,
    fallbackPriceRpcUrls: ethRpcFallbacks,
    adminRpcUrl,
    chainId,
    poolAddress: args.pool || process.env.MAYHEM_TAP_USDT_POOL || DEFAULT_TAP_USDT_POOL,
    tapAddress: args['tap-token'] || process.env.MAYHEM_TAP_TOKEN_ADDR,
    usdtAddress: args['usdt-token'] || process.env.MAYHEM_USDT_TOKEN_ADDR || DEFAULT_USDT_ADDRESS,
    fallbackUsd: args['tap-usd'],
    fallbackUsdAu: args['tap-usd-au'] ?? args['fallback-usd-au'],
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
    verifyTimeoutMs: parseNonNegativeInt(args['verify-timeout-ms'] ?? 0, '--verify-timeout-ms', 0),
    verifyPollMs: parsePositiveInt(args['verify-poll-ms'] ?? 500, '--verify-poll-ms', 500),
    requireLiveMainnetPrice: !boolArg(args['allow-price-fallback'], false),
  };

  do {
    const report = await runOnce(options);
    if (json) {
      console.log(JSON.stringify(report, null, 2));
    } else {
      console.log('[tap:rate] TAP/USD oracle tick complete');
      console.log('[tap:rate] source:', report.source);
      if (report.rpc_source) console.log('[tap:rate] rpc_source:', report.rpc_source);
      console.log('[tap:rate] tap_usd_au:', report.tap_usd_au);
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

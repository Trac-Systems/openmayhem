#!/usr/bin/env node

import { setTimeout as sleep } from 'node:timers/promises';

export const MAINNET_MSB = Object.freeze({
  networkId: 918,
  bootstrap: 'acbc3a4344d3a804101d40e53db1dda82b767646425af73599d4cd6577d69685',
  channel: '0000trac0network0msb0mainnet0000',
});

const PAYMENT_KEYS = Object.freeze([
  'denom',
  'fiat',
  'rails',
  'set_by',
  'set_by_role',
  'tap',
  'tnk',
  'updated_at',
  'ver',
]);
const FIAT_KEYS = Object.freeze([
  'adaptive_pricing',
  'integration_currency',
  'locale',
  'payout_currencies',
  'processor',
]);
const TAP_KEYS = Object.freeze(['chain_id', 'pool_address', 'token_address']);
const TNK_KEYS = Object.freeze(['network', 'treasury_address']);

function parseArgs(argv = process.argv.slice(2)) {
  const args = {};
  for (let i = 0; i < argv.length; i += 1) {
    const raw = argv[i];
    if (!raw.startsWith('--')) continue;
    const key = raw.slice(2);
    const next = argv[i + 1];
    if (next === undefined || next.startsWith('--')) args[key] = true;
    else {
      args[key] = next;
      i += 1;
    }
  }
  return args;
}

function positiveInteger(value, label, fallback) {
  const candidate = value === undefined || value === null || value === '' ? fallback : value;
  const parsed = Number(candidate);
  if (!Number.isSafeInteger(parsed) || parsed <= 0) throw new Error(`${label} must be positive`);
  return parsed;
}

function nonNegativeInteger(value, label, fallback) {
  const candidate = value === undefined || value === null || value === '' ? fallback : value;
  const parsed = Number(candidate);
  if (!Number.isSafeInteger(parsed) || parsed < 0) {
    throw new Error(`${label} must be a non-negative integer`);
  }
  return parsed;
}

function rpcBase(value) {
  const raw = String(value ?? '').trim();
  if (!raw) throw new Error('peer RPC is required');
  return raw.endsWith('/') ? raw : `${raw}/`;
}

function sanitizeError(error) {
  return String(error?.message ?? error ?? 'unknown error')
    .replace(/https?:\/\/[^\s]+/gi, '<redacted-url>')
    .replace(/([?&](?:key|token|api_key|apikey)=)[^&\s]+/gi, '$1<redacted>');
}

async function responseJson(response, label) {
  if (!response?.ok) throw new Error(`${label} returned HTTP ${response?.status ?? 'unknown'}`);
  return await response.json();
}

async function fetchWithTimeout(fetchImpl, input, init, timeoutMs) {
  const controller = new AbortController();
  let timer;
  const deadline = new Promise((_, reject) => {
    timer = setTimeout(() => {
      controller.abort();
      reject(new Error(`request attempt timed out after ${timeoutMs}ms`));
    }, timeoutMs);
  });
  try {
    return await Promise.race([
      fetchImpl(input, { ...init, signal: controller.signal }),
      deadline,
    ]);
  } finally {
    clearTimeout(timer);
  }
}

function exactKeys(value, expected) {
  return value !== null
    && typeof value === 'object'
    && !Array.isArray(value)
    && JSON.stringify(Object.keys(value).sort()) === JSON.stringify(expected);
}

function isLowerHex(value, length) {
  return typeof value === 'string' && new RegExp(`^[0-9a-f]{${length}}$`).test(value);
}

function isEthereumAddress(value) {
  return typeof value === 'string' && /^0x[0-9a-f]{40}$/.test(value);
}

export function validateCanonicalPaymentsState(state) {
  const failures = [];
  const payments = state?.value;
  if (state?.confirmed !== true) failures.push('payments/current is not confirmed');
  if (!exactKeys(payments, PAYMENT_KEYS)) {
    failures.push('payments/current has an incompatible top-level schema');
  }
  if (payments?.denom !== 'au_usd') failures.push('payments/current denom is not au_usd');
  if (JSON.stringify(payments?.rails) !== JSON.stringify(['fiat', 'tap', 'tnk'])) {
    failures.push('payments/current rails are not exactly fiat,tap,tnk');
  }
  if (!Number.isSafeInteger(payments?.ver) || payments.ver <= 0) {
    failures.push('payments/current version is not positive');
  }
  if (payments?.set_by_role !== 'admin' || !isLowerHex(payments?.set_by, 64)) {
    failures.push('payments/current is not attributed to a canonical admin identity');
  }

  const fiat = payments?.fiat;
  if (!exactKeys(fiat, FIAT_KEYS)) {
    failures.push('payments/current fiat schema is incompatible');
  }
  if (fiat?.processor !== 'stripe') failures.push('payments/current fiat processor is not stripe');
  if (fiat?.integration_currency !== 'usd') {
    failures.push('payments/current Stripe integration currency is not usd');
  }
  if (fiat?.adaptive_pricing !== true) {
    failures.push('payments/current Stripe Adaptive Pricing is not enabled');
  }
  if (fiat?.locale !== 'en') failures.push('payments/current Stripe locale is not en');
  const payoutCurrencies = fiat?.payout_currencies;
  if (!Array.isArray(payoutCurrencies)
      || payoutCurrencies.length < 3
      || payoutCurrencies.some((currency) => !/^[a-z]{3}$/.test(currency))
      || new Set(payoutCurrencies).size !== payoutCurrencies.length
      || JSON.stringify(payoutCurrencies) !== JSON.stringify([...payoutCurrencies].sort())
      || !['eur', 'gbp', 'usd'].every((currency) => payoutCurrencies.includes(currency))) {
    failures.push('payments/current Stripe payout currencies are invalid');
  }

  const tap = payments?.tap;
  if (!exactKeys(tap, TAP_KEYS)) failures.push('payments/current TAP schema is incompatible');
  if (!Number.isSafeInteger(tap?.chain_id) || tap.chain_id <= 0) {
    failures.push('payments/current TAP chain id is invalid');
  }
  if (!isEthereumAddress(tap?.token_address) || !isEthereumAddress(tap?.pool_address)) {
    failures.push('payments/current TAP addresses are invalid');
  }

  const tnk = payments?.tnk;
  if (!exactKeys(tnk, TNK_KEYS)) failures.push('payments/current TNK schema is incompatible');
  if (tnk?.network !== 'mainnet' || !/^trac1[a-z0-9]{20,120}$/.test(tnk?.treasury_address ?? '')) {
    failures.push('payments/current TNK mainnet directory is invalid');
  }

  return {
    ok: failures.length === 0,
    failures,
    version: Number.isSafeInteger(payments?.ver) ? payments.ver : null,
  };
}

export function validateMainnetState(chainId, status, paymentsState) {
  const msb = status?.msb ?? {};
  const payments = validateCanonicalPaymentsState(paymentsState);
  const failures = [...payments.failures];
  if (chainId !== 1) failures.push(`Ethereum chainId is ${chainId ?? 'missing'}, expected 1`);
  if (msb.ready !== true) failures.push('MSB is not ready');
  if (msb.networkId !== MAINNET_MSB.networkId) {
    failures.push(`MSB networkId is ${msb.networkId ?? 'missing'}, expected ${MAINNET_MSB.networkId}`);
  }
  if (String(msb.bootstrapHex ?? '').toLowerCase() !== MAINNET_MSB.bootstrap) {
    failures.push('MSB bootstrap is not the official mainnet bootstrap');
  }
  if (msb.channel !== MAINNET_MSB.channel) {
    failures.push('MSB channel is not the official mainnet channel');
  }
  if (!Number.isSafeInteger(msb.signedLength) || msb.signedLength <= 0) {
    failures.push('MSB signed length is not positive');
  }
  if (!Number.isSafeInteger(msb.connectedValidators) || msb.connectedValidators <= 0) {
    failures.push('MSB has no connected validator');
  }
  return {
    ok: failures.length === 0,
    failures,
    ethereum: { chain_id: chainId },
    msb: {
      ready: msb.ready === true,
      network_id: msb.networkId ?? null,
      bootstrap: msb.bootstrapHex ?? null,
      channel: msb.channel ?? null,
      signed_length: msb.signedLength ?? null,
      connected_validators: msb.connectedValidators ?? null,
    },
    payments: {
      version: payments.version,
      schema_valid: payments.ok,
    },
  };
}

export async function proveMainnet({
  ethRpc,
  peerRpc = 'http://127.0.0.1:49223/v1',
  timeoutSeconds = 180,
  pollMs = 2_000,
  attemptTimeoutMs = 30_000,
  fetchImpl = globalThis.fetch,
} = {}) {
  if (!String(ethRpc ?? '').trim()) throw new Error('Ethereum RPC is required');
  if (typeof fetchImpl !== 'function') throw new Error('fetch is unavailable');
  const timeout = nonNegativeInteger(timeoutSeconds, 'timeoutSeconds', 180) * 1_000;
  const poll = positiveInteger(pollMs, 'pollMs', 2_000);
  const attemptTimeout = positiveInteger(attemptTimeoutMs, 'attemptTimeoutMs', 30_000);
  const deadline = timeout > 0 ? Date.now() + timeout : null;
  let last = 'no attempt';

  do {
    try {
      const [chainResponse, statusResponse, paymentsResponse] = await Promise.all([
        fetchWithTimeout(fetchImpl, ethRpc, {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'eth_chainId', params: [] }),
        }, attemptTimeout),
        fetchWithTimeout(
          fetchImpl,
          new URL('status', rpcBase(peerRpc)),
          undefined,
          attemptTimeout,
        ),
        fetchWithTimeout(
          fetchImpl,
          new URL('state?key=payments%2Fcurrent', rpcBase(peerRpc)),
          undefined,
          attemptTimeout,
        ),
      ]);
      const [chainBody, status, paymentsState] = await Promise.all([
        responseJson(chainResponse, 'Ethereum RPC'),
        responseJson(statusResponse, 'peer status'),
        responseJson(paymentsResponse, 'payments/current'),
      ]);
      const chainId = typeof chainBody?.result === 'string'
        ? Number.parseInt(chainBody.result, 16)
        : null;
      const report = validateMainnetState(chainId, status, paymentsState);
      if (report.ok) return report;
      last = report.failures.join('; ');
    } catch (error) {
      last = sanitizeError(error);
    }
    if (deadline !== null && Date.now() >= deadline) break;
    await sleep(deadline === null ? poll : Math.min(poll, Math.max(1, deadline - Date.now())));
  } while (deadline === null || Date.now() <= deadline);

  throw new Error(`mainnet proof failed: ${last}`);
}

async function main() {
  const args = parseArgs();
  const report = await proveMainnet({
    ethRpc: args['eth-rpc'] || process.env.MAYHEM_TAP_ETH_RPC,
    peerRpc: args['peer-rpc'] || process.env.MAYHEM_PEER_RPC,
    timeoutSeconds: args['timeout-seconds'] || process.env.MAYHEM_MAINNET_PROOF_TIMEOUT_SECONDS,
    pollMs: args['poll-ms'],
    attemptTimeoutMs:
      args['attempt-timeout-ms'] || process.env.MAYHEM_MAINNET_PROOF_ATTEMPT_TIMEOUT_MS,
  });
  if (args.json) console.log(JSON.stringify(report, null, 2));
  else {
    console.log(`Mainnet proof passed: Ethereum chainId ${report.ethereum.chain_id}`);
    console.log(`MSB ${report.msb.channel}: signed length ${report.msb.signed_length}, validators ${report.msb.connected_validators}`);
  }
}

if (import.meta.url === `file://${process.argv[1]}`) {
  main().catch((error) => {
    console.error(sanitizeError(error));
    process.exit(1);
  });
}

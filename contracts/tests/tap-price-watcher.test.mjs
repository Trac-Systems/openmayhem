import assert from 'node:assert/strict';
import test from 'node:test';

import {
  DEFAULT_USDT_ADDRESS,
  buildAdminCommand,
  buildAdminCommandArgs,
  decimalUsdToE6,
  parsePinnedTapUsdE6,
  readTapUsdE6FromDex,
  resetTapPriceCacheForTest,
  resolveTapUsdRate,
  tapRateStateMatches,
  tapUsdE6FromReserves,
} from '../scripts/tap-price-watcher.mjs';

const TAP = '0x1111111111111111111111111111111111111111';
const POOL = '0x2222222222222222222222222222222222222222';
const U18 = (n) => BigInt(n) * 1_000_000_000_000_000_000n;
const U6 = (n) => BigInt(n) * 1_000_000n;

function fakePool({
  token0 = TAP,
  token1 = DEFAULT_USDT_ADDRESS,
  reserve0 = U18(100),
  reserve1 = U6(250),
} = {}) {
  return {
    token0: async () => token0,
    token1: async () => token1,
    getReserves: async () => [reserve0, reserve1, 0],
  };
}

test('TAP reserve conversion is integer-exact', () => {
  assert.equal(tapUsdE6FromReserves({
    tapReserve: U18(100),
    usdtReserve: U6(250),
  }), 2_500_000);
  assert.equal(tapUsdE6FromReserves({
    tapReserve: 3n * 1_000_000_000_000_000_000n,
    usdtReserve: 1n * 1_000_000n,
  }), 333_333);
  assert.equal(decimalUsdToE6('0.1234567'), 123_457);
  assert.equal(parsePinnedTapUsdE6({ env: { TAP_USD: '0.05' } }), 50_000);
  assert.equal(parsePinnedTapUsdE6({ env: { MAYHEM_TAP_USD_E6: '50001' } }), 50_001);
});

test('TAP DEX reader maps either Uniswap token order', async () => {
  const forward = await readTapUsdE6FromDex({
    provider: {},
    poolAddress: POOL,
    tapAddress: TAP,
    poolFactory: () => fakePool(),
  });
  assert.equal(forward.tap_usd_e6, 2_500_000);
  assert.equal(forward.tap_reserve, U18(100).toString());
  assert.equal(forward.usdt_reserve, U6(250).toString());

  const reverse = await readTapUsdE6FromDex({
    provider: {},
    poolAddress: POOL,
    tapAddress: TAP,
    poolFactory: () => fakePool({
      token0: DEFAULT_USDT_ADDRESS,
      token1: TAP,
      reserve0: U6(75),
      reserve1: U18(30),
    }),
  });
  assert.equal(reverse.tap_usd_e6, 2_500_000);
});

test('TAP rate resolver caches DEX reads and falls back safely', async () => {
  resetTapPriceCacheForTest();
  let calls = 0;
  const first = await resolveTapUsdRate({
    rpcUrl: 'http://eth.local',
    chainId: 1,
    tapAddress: TAP,
    providerFactory: () => ({}),
    poolFactory: () => {
      calls += 1;
      return fakePool();
    },
    nowMs: () => 1_000,
    nowSeconds: () => 10,
    ttlMs: 1_000,
  });
  assert.equal(first.source, 'uniswap-v2');
  assert.equal(first.tap_usd_e6, 2_500_000);
  assert.equal(calls, 1);

  const cached = await resolveTapUsdRate({
    rpcUrl: 'http://eth.local',
    chainId: 1,
    tapAddress: TAP,
    providerFactory: () => ({}),
    poolFactory: () => {
      calls += 1;
      return fakePool();
    },
    nowMs: () => 1_500,
    nowSeconds: () => 11,
    ttlMs: 1_000,
  });
  assert.equal(cached.source, 'uniswap-v2');
  assert.equal(cached.cache_hit, true);
  assert.equal(calls, 1);

  const stale = await resolveTapUsdRate({
    rpcUrl: 'http://eth.local',
    chainId: 1,
    tapAddress: TAP,
    providerFactory: () => ({}),
    poolFactory: () => ({
      token0: async () => { throw new Error('rpc down'); },
      token1: async () => DEFAULT_USDT_ADDRESS,
      getReserves: async () => [U18(1), U6(1), 0],
    }),
    nowMs: () => 3_000,
    nowSeconds: () => 30,
    ttlMs: 1_000,
    timeoutMs: 50,
  });
  assert.equal(stale.source, 'stale');
  assert.equal(stale.tap_usd_e6, 2_500_000);
  assert.equal(stale.stale_from_ts, 10);
  assert.match(stale.failures[0].error, /rpc down/);

  resetTapPriceCacheForTest();
  const config = await resolveTapUsdRate({
    rpcUrl: 'http://eth.local',
    chainId: 1,
    tapAddress: TAP,
    fallbackUsd: '0.125',
    providerFactory: () => ({}),
    poolFactory: () => ({
      token0: async () => { throw new Error('rpc down'); },
      token1: async () => DEFAULT_USDT_ADDRESS,
      getReserves: async () => [U18(1), U6(1), 0],
    }),
    nowMs: () => 4_000,
    nowSeconds: () => 40,
    ttlMs: 1_000,
    timeoutMs: 50,
  });
  assert.equal(config.source, 'config');
  assert.equal(config.tap_usd_e6, 125_000);
  assert.match(config.failures[0].error, /rpc down/);

  resetTapPriceCacheForTest();
  const noRpc = await resolveTapUsdRate({
    chainId: 0,
    fallbackUsdE6: 55_000,
    nowMs: () => 5_000,
    nowSeconds: () => 50,
  });
  assert.equal(noRpc.source, 'config');
  assert.equal(noRpc.tap_usd_e6, 55_000);
});

test('TAP rate watcher builds redacted admin commands and verifies state shape', () => {
  const rate = { source: 'config', tap_usd_e6: 50_000, ts: 3_600 };
  const args = buildAdminCommandArgs(rate, {
    rpcUrl: 'http://127.0.0.1:49223/v1',
    walletPassword: 'secret',
  });
  assert.deepEqual(args.slice(0, 5), ['admin', 'tap-rate-oracle', '--tap-usd-e6', '50000', '--source']);
  assert.equal(args.includes('--wallet-password'), true);
  assert.equal(args.includes('secret'), true);

  const command = buildAdminCommand(rate, {
    rpcUrl: 'http://127.0.0.1:49223/v1',
    walletPassword: 'secret',
  });
  assert.match(command, /tap-rate-oracle/);
  assert.match(command, /--tap-usd-e6/);
  assert.doesNotMatch(command, /secret/);
  assert.doesNotMatch(command, /wallet-password/);

  assert.equal(tapRateStateMatches(rate, {
    denom: 'tap_usd_e6',
    tap_usd_e6: 50_000,
    source: 'config',
    ts: 3_600,
    posted_by_role: 'admin',
  }), true);
  assert.equal(tapRateStateMatches(rate, {
    denom: 'tap_usd_e6',
    tap_usd_e6: 49_999,
    source: 'config',
    ts: 3_600,
    posted_by_role: 'admin',
  }), false);
});

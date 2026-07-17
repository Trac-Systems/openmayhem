import assert from 'node:assert/strict';
import test from 'node:test';

import {
  DEFAULT_ADMIN_SUBMIT_TIMEOUT_MS,
  DEFAULT_USDT_ADDRESS,
  buildAdminCommand,
  buildAdminCommandArgs,
  counterfactualCumulativePrices,
  decimalUsdToAu,
  deviationWithinBps,
  medianInteger,
  parsePinnedTapUsdAu,
  readTapUsdAuFromDex,
  readTapUsdTwapFromDex,
  resetTapPriceCacheForTest,
  resolveTapUsdRate,
  rpcUrlCandidates,
  runOnce,
  tapRateStateMatches,
  tapUsdAuFromReserves,
  waitForTapRateState,
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

function fakeTwap({
  tapUsdAu = '2500000000000000000',
  spotTapUsdAu = tapUsdAu,
} = {}) {
  return {
    tap_usd_au: tapUsdAu,
    spot_tap_usd_au: spotTapUsdAu,
    pool_address: POOL,
    start_block: 50,
    end_block: 200,
    start_timestamp: 600,
    end_timestamp: 2_400,
    window_seconds: 1_800,
  };
}

test('TAP reserve conversion is integer-exact', () => {
  assert.equal(tapUsdAuFromReserves({
    tapReserve: U18(100),
    usdtReserve: U6(250),
  }), '2500000000000000000');
  assert.equal(tapUsdAuFromReserves({
    tapReserve: 3n * 1_000_000_000_000_000_000n,
    usdtReserve: 1n * 1_000_000n,
  }), '333333333333333333');
  assert.equal(decimalUsdToAu('0.1234567890123456789'), '123456789012345679');
  assert.equal(parsePinnedTapUsdAu({ env: { TAP_USD: '0.05' } }), '50000000000000000');
  assert.equal(parsePinnedTapUsdAu({
    env: { MAYHEM_TAP_USD_AU: '50001000000000000' },
  }), '50001000000000000');
});

test('TAP DEX reader maps either Uniswap token order', async () => {
  const forward = await readTapUsdAuFromDex({
    provider: {},
    poolAddress: POOL,
    tapAddress: TAP,
    poolFactory: () => fakePool(),
  });
  assert.equal(forward.tap_usd_au, '2500000000000000000');
  assert.equal(forward.tap_reserve, U18(100).toString());
  assert.equal(forward.usdt_reserve, U6(250).toString());

  const reverse = await readTapUsdAuFromDex({
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
  assert.equal(reverse.tap_usd_au, '2500000000000000000');
});

test('TAP TWAP reader uses finalized cumulative prices and normalizes token decimals', async () => {
  const provider = {
    async send(method, [tag, includeTransactions]) {
      assert.equal(method, 'eth_getBlockByNumber');
      assert.equal(includeTransactions, false);
      const number = tag === 'finalized' ? 200 : Number(BigInt(tag));
      return {
        number: `0x${number.toString(16)}`,
        timestamp: `0x${(number * 12).toString(16)}`,
      };
    },
  };
  const pool = {
    token0: async () => TAP,
    token1: async () => DEFAULT_USDT_ADDRESS,
    getReserves: async () => [U18(100), U6(250), 0],
    price0CumulativeLast: async () => 0n,
    price1CumulativeLast: async () => 0n,
  };
  const twap = await readTapUsdTwapFromDex({
    provider,
    poolAddress: POOL,
    tapAddress: TAP,
    windowSeconds: 1_800,
    poolFactory: () => pool,
  });
  assert.equal(twap.tap_usd_au, '2499999999999999999');
  assert.equal(twap.spot_tap_usd_au, '2500000000000000000');
  assert.equal(twap.start_block, 50);
  assert.equal(twap.end_block, 200);
  assert.equal(twap.window_seconds, 1_800);

  const wrapped = counterfactualCumulativePrices({
    price0Cumulative: 0,
    price1Cumulative: 0,
    reserve0: 1,
    reserve1: 2,
    blockTimestampLast: 0xfffffff0,
    blockTimestamp: 5,
  });
  assert.equal(wrapped.block_timestamp, 5);
  assert.equal(wrapped.price0_cumulative > 0n, true);
  assert.equal(medianInteger([10, 14]).toString(), '12');
  assert.equal(deviationWithinBps(110, 100, 1_000), true);
  assert.equal(deviationWithinBps(111, 100, 1_000), false);
});

test('TAP rate resolver caches two-source TWAP medians and fails closed when live sources disappear', async () => {
  resetTapPriceCacheForTest();
  let calls = 0;
  const first = await resolveTapUsdRate({
    rpcUrls: 'https://provider-a.example/rpc https://provider-b.example/rpc',
    chainId: 1,
    tapAddress: TAP,
    hardFloorAu: '1000000000000000',
    hardCeilingAu: '10000000000000000000',
    providerFactory: (url) => ({ url }),
    priceReader: async () => {
      calls += 1;
      return fakeTwap();
    },
    nowMs: () => 1_000,
    nowSeconds: () => 10,
    ttlMs: 1_000,
  });
  assert.equal(first.source, 'uniswap-v2-twap-median');
  assert.equal(first.tap_usd_au, '2500000000000000000');
  assert.equal(first.rpc_sources.length, 2);
  assert.equal(calls, 2);

  const cached = await resolveTapUsdRate({
    rpcUrls: 'https://provider-a.example/rpc https://provider-b.example/rpc',
    chainId: 1,
    tapAddress: TAP,
    hardFloorAu: '1000000000000000',
    hardCeilingAu: '10000000000000000000',
    priceReader: async () => {
      calls += 1;
      return fakeTwap();
    },
    nowMs: () => 1_500,
    nowSeconds: () => 11,
    ttlMs: 1_000,
  });
  assert.equal(cached.source, 'uniswap-v2-twap-median');
  assert.equal(cached.cache_hit, true);
  assert.equal(calls, 2);

  await assert.rejects(
    resolveTapUsdRate({
      rpcUrls: 'https://provider-a.example/rpc https://provider-b.example/rpc',
      chainId: 1,
      tapAddress: TAP,
      hardFloorAu: '1000000000000000',
      hardCeilingAu: '10000000000000000000',
      priceReader: async () => { throw new Error('rpc down'); },
      nowMs: () => 3_000,
      nowSeconds: () => 30,
      ttlMs: 1_000,
    }),
    /source quorum unavailable/,
  );

  resetTapPriceCacheForTest();
  const config = await resolveTapUsdRate({
    rpcUrls: 'https://provider-a.example/rpc https://provider-b.example/rpc',
    chainId: 1,
    tapAddress: TAP,
    fallbackUsd: '0.125',
    hardFloorAu: '1000000000000000',
    hardCeilingAu: '10000000000000000000',
    priceReader: async () => { throw new Error('rpc down'); },
    nowMs: () => 4_000,
    nowSeconds: () => 40,
    ttlMs: 1_000,
  });
  assert.equal(config.source, 'config');
  assert.equal(config.tap_usd_au, '125000000000000000');
  assert.match(config.failures[0].error, /rpc down/);

  resetTapPriceCacheForTest();
  const noRpc = await resolveTapUsdRate({
    chainId: 0,
    fallbackUsdAu: '55000000000000000',
    nowMs: () => 5_000,
    nowSeconds: () => 50,
  });
  assert.equal(noRpc.source, 'config');
  assert.equal(noRpc.tap_usd_au, '55000000000000000');
});

test('TAP rate resolver requires quorum, medianizes independent RPCs, and redacts failures', async () => {
  resetTapPriceCacheForTest();
  assert.deepEqual(rpcUrlCandidates({
    rpcUrl: 'https://provider-a.example/rpc',
    fallbackRpcUrls: 'https://provider-b.example/rpc, https://provider-a.example/rpc',
  }), [
    'https://provider-a.example/rpc',
    'https://provider-b.example/rpc',
  ]);

  const used = [];
  const rate = await resolveTapUsdRate({
    rpcUrl: 'https://provider-a.example/private-token',
    fallbackRpcUrls: 'https://provider-b.example/private-token https://provider-c.example/private-token',
    chainId: 1,
    tapAddress: TAP,
    hardFloorAu: '1000000000000000',
    hardCeilingAu: '10000000000000000000',
    providerFactory: (url) => {
      used.push(url);
      return { url };
    },
    priceReader: async ({ provider }) => {
      if (provider.url.includes('provider-a')) {
        throw new Error(`down ${provider.url}`);
      }
      return provider.url.includes('provider-b')
        ? fakeTwap({ tapUsdAu: '2400000000000000000', spotTapUsdAu: '2500000000000000000' })
        : fakeTwap({ tapUsdAu: '2600000000000000000', spotTapUsdAu: '2500000000000000000' });
    },
    nowMs: () => 7_000,
    nowSeconds: () => 70,
    ttlMs: 1_000,
  });

  assert.deepEqual(used, [
    'https://provider-a.example/private-token',
    'https://provider-b.example/private-token',
    'https://provider-c.example/private-token',
  ]);
  assert.equal(rate.source, 'uniswap-v2-twap-median');
  assert.equal(rate.tap_usd_au, '2500000000000000000');
  assert.deepEqual(rate.rpc_sources, ['provider-b.example', 'provider-c.example']);
  assert.equal(rate.failures.length, 1);
  assert.equal(rate.failures[0].rpc_source, 'provider-a.example');
  assert.doesNotMatch(rate.failures[0].error, /private-token/);
  assert.match(rate.failures[0].error, /https:\/\/provider-a\.example\/\.\.\./);
});

test('TAP rate resolver enforces spot deviation and hard bounds before publication', async () => {
  const base = {
    rpcUrls: 'https://one.example/rpc https://two.example/rpc',
    chainId: 1,
    tapAddress: TAP,
    hardFloorAu: '1000000000000000000',
    hardCeilingAu: '10000000000000000000',
    minimumSources: 2,
    maxDeviationBps: 1_000,
    providerFactory: (url) => ({ url }),
    ttlMs: 1,
  };

  resetTapPriceCacheForTest();
  await assert.rejects(
    resolveTapUsdRate({
      ...base,
      priceReader: async () => fakeTwap({
        tapUsdAu: '2500000000000000000',
        spotTapUsdAu: '4000000000000000000',
      }),
      nowMs: () => 1_000,
    }),
    /spot price.*deviation band/i,
  );

  resetTapPriceCacheForTest();
  await assert.rejects(
    resolveTapUsdRate({
      ...base,
      priceReader: async () => fakeTwap({ tapUsdAu: '500000000000000000' }),
      nowMs: () => 2_000,
    }),
    /outside.*hard price bounds/i,
  );
});

test('TAP rate resolver consults public RPCs only when private quorum is unavailable', async () => {
  resetTapPriceCacheForTest();
  const used = [];
  const rate = await resolveTapUsdRate({
    rpcUrls: 'https://private-one.example/rpc https://private-two.example/rpc',
    publicFallbackRpcUrls: 'https://public-one.example/rpc https://public-two.example/rpc',
    chainId: 1,
    tapAddress: TAP,
    hardFloorAu: '1000000000000000',
    hardCeilingAu: '10000000000000000000',
    providerFactory: (url) => ({ url }),
    priceReader: async ({ provider }) => {
      used.push(provider.url);
      if (provider.url.includes('private-two')) throw new Error('private RPC unavailable');
      return fakeTwap();
    },
    nowMs: () => 3_000,
    nowSeconds: () => 30,
    ttlMs: 1,
  });
  assert.equal(rate.source, 'uniswap-v2-twap-median');
  assert.equal(used.includes('https://public-one.example/rpc'), true);
  assert.equal(used.includes('https://public-two.example/rpc'), false);
  assert.equal(rate.observations.filter((item) => item.public_fallback).length, 1);
});

test('TAP rate watcher builds redacted admin commands and verifies state shape', () => {
  const rate = { source: 'config', tap_usd_au: '50000000000000000', ts: 3_600 };
  const args = buildAdminCommandArgs(rate, {
    rpcUrl: 'http://127.0.0.1:49223/v1',
    walletPassword: 'secret',
  });
  assert.deepEqual(args.slice(0, 5), ['admin', 'tap-rate-oracle', '--tap-usd-au', '50000000000000000', '--source']);
  assert.equal(args.includes('--wallet-password'), true);
  assert.equal(args.includes('secret'), true);

  const command = buildAdminCommand(rate, {
    rpcUrl: 'http://127.0.0.1:49223/v1',
    walletPassword: 'secret',
  });
  assert.match(command, /tap-rate-oracle/);
  assert.match(command, /--tap-usd-au/);
  assert.doesNotMatch(command, /secret/);
  assert.doesNotMatch(command, /wallet-password/);

  assert.equal(tapRateStateMatches(rate, {
    denom: 'tap_usd_au',
    tap_usd_au: '50000000000000000',
    source: 'config',
    ts: 3_600,
    posted_by_role: 'admin',
  }), true);
  assert.equal(tapRateStateMatches(rate, {
    denom: 'tap_usd_au',
    tap_usd_au: '49999999999999999',
    source: 'config',
    ts: 3_600,
    posted_by_role: 'admin',
  }), false);
});

test('TAP rate watcher bounds admin submission and reports a killed hung child', async () => {
  resetTapPriceCacheForTest();
  let spawnOptions = null;
  const submitted = await runOnce({
    chainId: 0,
    fallbackUsdAu: '50000000000000000',
    submit: true,
    sim: true,
    spawnImpl: (_bin, _args, options) => {
      spawnOptions = options;
      return { status: 0, stdout: '{}', stderr: '' };
    },
    nowMs: () => 20_000,
    nowSeconds: () => 200,
  });
  assert.equal(submitted.ok, true);
  assert.equal(spawnOptions.timeout, DEFAULT_ADMIN_SUBMIT_TIMEOUT_MS);
  assert.equal(spawnOptions.killSignal, 'SIGKILL');

  resetTapPriceCacheForTest();
  const timeoutError = Object.assign(new Error('spawnSync mayhem ETIMEDOUT'), {
    code: 'ETIMEDOUT',
  });
  const timedOut = await runOnce({
    chainId: 0,
    fallbackUsdAu: '50000000000000000',
    submit: true,
    submitTimeoutMs: 25,
    spawnImpl: () => ({
      status: null,
      signal: 'SIGKILL',
      error: timeoutError,
      stdout: '',
      stderr: '',
    }),
    nowMs: () => 21_000,
    nowSeconds: () => 210,
  });
  assert.equal(timedOut.ok, false);
  assert.equal(timedOut.submit_timed_out, true);
  assert.match(timedOut.error, /timed out after 25ms/);
});

test('TAP rate watcher bounds each verification read and the overall verification window', async () => {
  const result = await waitForTapRateState({
    source: 'uniswap-v2-twap-median',
    tap_usd_au: '50000000000000000',
    ts: 1,
  }, {
    rpcUrl: 'http://127.0.0.1:1/v1',
    timeoutMs: 20,
    requestTimeoutMs: 5,
    pollMs: 1,
    fetchImpl: async () => await new Promise(() => {}),
  });
  assert.equal(result.verified, false);
  assert.match(result.error, /timed out/);
});

test('TAP rate watcher refuses non-live mainnet fallback unless explicitly allowed', async () => {
  resetTapPriceCacheForTest();
  await assert.rejects(
    runOnce({
      chainId: 1,
      fallbackUsd: '0.05',
      requireLiveMainnetPrice: true,
      submit: false,
      nowMs: () => 10_000,
      nowSeconds: () => 100,
    }),
    /Refusing to use a non-live TAP price for a mainnet submission/
  );

  resetTapPriceCacheForTest();
  const allowed = await runOnce({
    chainId: 1,
    fallbackUsd: '0.05',
    requireLiveMainnetPrice: false,
    submit: false,
    nowMs: () => 11_000,
    nowSeconds: () => 110,
  });
  assert.equal(allowed.source, 'config');
  assert.equal(allowed.tap_usd_au, '50000000000000000');

  resetTapPriceCacheForTest();
  await assert.rejects(
    runOnce({
      chainId: 1,
      fallbackUsd: '0.05',
      requireLiveMainnetPrice: false,
      submit: true,
      nowMs: () => 12_000,
      nowSeconds: () => 120,
    }),
    /pinned fallback prices are dry-run only/,
  );
});

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  buildAdminCommand,
  decimalUsdToE6,
  parseGateTicker,
  parseMexcTicker,
  rateStateMatches,
  resolveTnkUsdRate,
  runOnce,
} from '../scripts/tnk-rate-watcher.mjs';

test('tnk rate watcher converts decimal USD strings to e6 fixed point', () => {
  assert.equal(decimalUsdToE6('1'), 1_000_000);
  assert.equal(decimalUsdToE6('0.0500004'), 50_000);
  assert.equal(decimalUsdToE6('0.0500005'), 50_001);
  assert.equal(decimalUsdToE6('2.1234567'), 2_123_457);
  assert.throws(() => decimalUsdToE6('1e-2'), /invalid decimal/i);
  assert.throws(() => decimalUsdToE6('0.0000004'), /outside safe/i);
});

test('tnk rate watcher parses Gate and MEXC ticker payloads', () => {
  assert.deepEqual(parseGateTicker([{ currency_pair: 'TNK_USDT', last: '0.0525' }]), {
    source: 'gate-spot',
    raw_price: '0.0525',
    tnk_usd_e6: 52_500,
  });
  assert.deepEqual(parseMexcTicker({ symbol: 'TNKUSDT', price: '0.052501' }), {
    source: 'mexc-spot',
    raw_price: '0.052501',
    tnk_usd_e6: 52_501,
  });
});

test('tnk rate watcher uses Gate first and MEXC as fallback', async () => {
  const calls = [];
  const rate = await resolveTnkUsdRate({
    nowSeconds: () => 3_600,
    fetchImpl: async (url) => {
      calls.push(String(url));
      if (String(url).includes('gateio')) {
        return { ok: false, status: 503, json: async () => ({}) };
      }
      return {
        ok: true,
        status: 200,
        json: async () => ({ symbol: 'TNKUSDT', price: '0.04125' }),
      };
    },
  });

  assert.equal(calls.length, 2);
  assert.equal(rate.source, 'mexc-spot');
  assert.equal(rate.primary, false);
  assert.equal(rate.tnk_usd_e6, 41_250);
  assert.equal(rate.ts, 3_600);
  assert.match(rate.failures[0].error, /503/);
});

test('tnk rate watcher copy/paste command redacts wallet password', () => {
  const command = buildAdminCommand(
    {
      source: 'gate-spot',
      tnk_usd_e6: 52_500,
      ts: 3_600,
    },
    {
      mayhemBin: 'mayhem',
      rpcUrl: 'http://127.0.0.1:49223/v1',
      walletPassword: 'secret-password',
      submit: true,
      json: true,
    }
  );
  assert.match(command, /mayhem/);
  assert.match(command, /rate-oracle/);
  assert.match(command, /'--source' 'gate-spot'/);
  assert.match(command, /'--rpc-url' 'http:\/\/127\.0\.0\.1:49223\/v1'/);
  assert.match(command, /--submit/);
  assert.doesNotMatch(command, /secret-password/);
  assert.doesNotMatch(command, /wallet-password/);
});

test('tnk rate watcher verifies exact admin rate state after submit', async () => {
  const matchingState = {
    denom: 'tnk_usd_e6',
    tnk_usd_e6: 52_500,
    source: 'gate-spot',
    ts: 3_600,
    posted_by_role: 'admin',
  };
  assert.equal(rateStateMatches({
    tnk_usd_e6: 52_500,
    source: 'gate-spot',
    ts: 3_600,
  }, matchingState), true);
  assert.equal(rateStateMatches({
    tnk_usd_e6: 52_500,
    source: 'gate-spot',
    ts: 3_600,
  }, { ...matchingState, posted_by_role: 'provider' }), false);
  assert.equal(rateStateMatches({
    tnk_usd_e6: 52_500,
    source: 'gate-spot',
    ts: 3_601,
  }, matchingState), false);

  const spawned = [];
  const report = await runOnce({
    submit: true,
    mayhemBin: 'mayhem',
    rpcUrl: 'http://127.0.0.1:49223/v1',
    nowSeconds: () => 3_600,
    fetchImpl: async (url) => {
      const raw = String(url);
      if (raw.includes('/state?')) {
        assert.match(raw, /key=rate%2Flatest/);
        return { ok: true, status: 200, json: async () => ({ value: matchingState }) };
      }
      return {
        ok: true,
        status: 200,
        json: async () => ([{ currency_pair: 'TNK_USDT', last: '0.0525' }]),
      };
    },
    spawnImpl: (bin, args, options) => {
      spawned.push({ bin, args, options });
      return { status: 0, stdout: '{"ok":true}\n', stderr: '' };
    },
  });

  assert.equal(report.ok, true);
  assert.equal(report.submitted, true);
  assert.equal(report.verified, true);
  assert.deepEqual(report.rate_state, matchingState);
  assert.equal(spawned.length, 1);
  assert.deepEqual(spawned[0].options, { encoding: 'utf8' });
});

test('tnk rate watcher fails submit when contract state is not admin evidence', async () => {
  const report = await runOnce({
    submit: true,
    mayhemBin: 'mayhem',
    rpcUrl: 'http://127.0.0.1:49223/v1',
    nowSeconds: () => 3_600,
    verifyTimeoutMs: 1,
    verifyPollMs: 1,
    fetchImpl: async (url) => {
      if (String(url).includes('/state?')) {
        return {
          ok: true,
          status: 200,
          json: async () => ({
            value: {
              denom: 'tnk_usd_e6',
              tnk_usd_e6: 52_500,
              source: 'gate-spot',
              ts: 3_600,
              posted_by_role: 'provider',
            },
          }),
        };
      }
      return {
        ok: true,
        status: 200,
        json: async () => ([{ currency_pair: 'TNK_USDT', last: '0.0525' }]),
      };
    },
    spawnImpl: () => ({ status: 0, stdout: '{"ok":true}\n', stderr: '' }),
  });

  assert.equal(report.ok, false);
  assert.equal(report.submitted, true);
  assert.equal(report.verified, false);
  assert.match(report.error, /did not update contract rate\/latest/);
  assert.equal(report.rate_state.posted_by_role, 'provider');
});

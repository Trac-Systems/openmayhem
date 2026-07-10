import assert from 'node:assert/strict';
import test from 'node:test';

import { parseRootMsbBalanceHelperArgs } from '../src/msb-balance-helper.js';
import { runRootMsbTransferHelper } from '../src/msb-transfer-helper.js';

test('root Intercom app resolves the bundled MSB transfer helper', async () => {
  await assert.rejects(
    runRootMsbTransferHelper('invalid'),
    /Usage:/
  );
});

test('root Intercom app parses a read-only official-MSB balance helper', () => {
  assert.deepEqual(
    parseRootMsbBalanceHelperArgs([
      '--network', 'mainnet',
      '--stores-directory', '/tmp/mayhem/stores',
      '--store-name', 'balance-reader',
      '--address', 'trac1treasury',
      '--timeout-seconds', '90',
    ]),
    {
      network: 'mainnet',
      storesDirectory: '/tmp/mayhem/stores',
      storeName: 'balance-reader',
      address: 'trac1treasury',
      timeoutSeconds: 90,
    }
  );
});

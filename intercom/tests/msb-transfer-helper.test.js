import assert from 'node:assert/strict';
import test from 'node:test';

import { parseRootMsbBalanceHelperArgs } from '../src/msb-balance-helper.js';
import {
  MAYHEM_NETWORK_ENV,
  createMayhemMsbConfig,
  createMayhemPeerConfig,
} from '../src/network-config.js';
import {
  executeTransfer,
} from '../src/msb-settlement-transfer-helper.js';
import { runRootMsbTransferHelper } from '../src/msb-transfer-helper.js';
import { bigIntTo16ByteBuffer } from '../trac/msb/src/utils/amountSerialization.js';

test('root Intercom app resolves the bundled MSB transfer helper', async () => {
  await assert.rejects(
    runRootMsbTransferHelper('invalid'),
    /Supported MSB helper commands/
  );
  await assert.rejects(
    runRootMsbTransferHelper('settlement-transfer-prepare'),
    /Missing --network/
  );
  await assert.rejects(
    runRootMsbTransferHelper('settlement-transfer-execute'),
    /Missing --network/
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
      '--direct-peer', `${'AB'.repeat(32)},${'cd'.repeat(32)},${'ab'.repeat(32)}`,
    ]),
    {
      network: 'mainnet',
      storesDirectory: '/tmp/mayhem/stores',
      storeName: 'balance-reader',
      address: 'trac1treasury',
      timeoutSeconds: 90,
      directPeers: ['ab'.repeat(32), 'cd'.repeat(32)],
    }
  );
});

test('app-owned network adapter supplies testnet without modifying pinned config modules', () => {
  const msb = createMayhemMsbConfig(MAYHEM_NETWORK_ENV.TESTNET1, {
    storeName: 'test-msb',
  });
  const peer = createMayhemPeerConfig(MAYHEM_NETWORK_ENV.TESTNET1, {
    storeName: 'test-peer',
  });
  assert.equal(msb.networkId, 919);
  assert.equal(msb.addressPrefix, 'testtrac');
  assert.equal(
    msb.bootstrap.toString('hex'),
    'c184f4ad8e9cf5e911f9415b60e7dcfb30aed73ebd8a402ef68e1b154624f5ef'
  );
  assert.equal(peer.storeName, 'test-peer');
  assert.equal(peer.bootstrap, null);
  assert.throws(
    () => createMayhemPeerConfig('unknown', { storeName: 'invalid' }),
    /Unknown Mayhem network environment/
  );
});

test('app-owned transfer helper broadcasts and verifies exactly one canonical transfer', async () => {
  const from = 'testtrac1sender';
  const to = 'testtrac1recipient';
  const amount = '1.25';
  const txHash = 'ab'.repeat(32);
  let broadcasts = 0;
  const payload = {
    address: from,
    tro: {
      tx: txHash,
      to,
      am: bigIntTo16ByteBuffer(1_250_000_000_000_000_000n).toString('hex'),
    },
  };
  const msb = {
    wallet: { address: from },
    network: {
      validatorConnectionManager: {
        connectionCount: () => 1,
      },
    },
    state: {
      getNodeEntry: async () => ({
        balance: bigIntTo16ByteBuffer(3_000_000_000_000_000_000n),
      }),
      getFee: () => bigIntTo16ByteBuffer(10_000_000_000_000_000n),
      getIndexerSequenceState: async () => Buffer.alloc(32),
      getSigned: async (hash) => hash === txHash,
      getTransactionConfirmedLength: async () => {
        throw new Error('sparse confirmation must not scan transaction history');
      },
      getSignedLength: () => 80,
    },
    async broadcastPartialTransaction(value) {
      broadcasts += 1;
      assert.deepEqual(value, payload);
      return true;
    },
  };
  const result = await executeTransfer({
    msb,
    config: {},
    network: 'testnet1',
    to,
    amount,
    timeoutSeconds: 1,
    stderr: { write() {} },
    buildPayload: async () => payload,
    readConfirmedTransfer: async (_msb, _hash, _config, confirmedLength) => ({
      confirmed_length: confirmedLength,
      txDetails: {
        address: from,
        tro: {
          tx: txHash,
          to,
          am: '1250000000000000000',
        },
      },
    }),
  });
  assert.equal(broadcasts, 1);
  assert.equal(result.command, 'transfer');
  assert.equal(result.tx_hash, txHash);
  assert.equal(result.confirmed_length, 80);
});

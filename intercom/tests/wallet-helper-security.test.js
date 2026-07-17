import assert from 'node:assert/strict';
import { Readable } from 'node:stream';
import test from 'node:test';
import {
  readWalletHelperSecretsFromStdin,
  walletHelperFlagsWithSecrets,
} from '../src/wallet-helper.js';

test('wallet helper accepts secrets only through the inherited channel', () => {
  const flags = walletHelperFlagsWithSecrets(
    {
      'wallet-helper': 'create',
      keypair: '/tmp/wallet/keypair.json',
    },
    {
      password: 'wallet password',
      mnemonic: 'test mnemonic',
      ethereum_private_key: '0xprivate',
    }
  );
  assert.equal(flags.password, 'wallet password');
  assert.equal(flags.mnemonic, 'test mnemonic');
  assert.equal(flags['ethereum-private-key'], '0xprivate');
});

test('wallet helper refuses a secret-valued argv flag', () => {
  assert.throws(
    () => walletHelperFlagsWithSecrets({
      'wallet-helper': 'inspect',
      keypair: '/tmp/wallet/keypair.json',
      password: 'argv-secret',
    }),
    /must not be passed on argv/
  );
});

test('wallet helper preserves empty and replacement passwords from stdin', () => {
  const flags = walletHelperFlagsWithSecrets(
    {
      'wallet-helper': 'inspect',
      keypair: '/tmp/wallet/keypair.json',
    },
    {
      password: '',
      new_password: 'next-password',
    }
  );
  assert.equal(flags.password, '');
  assert.equal(flags['new-password'], 'next-password');
});

test('wallet helper reads one bounded JSON object from stdin', async () => {
  const input = Readable.from([
    JSON.stringify({
      password: 'stdin-password',
      mnemonic: 'stdin mnemonic',
    }),
  ]);
  const secrets = await readWalletHelperSecretsFromStdin(input);
  assert.deepEqual(secrets, {
    password: 'stdin-password',
    mnemonic: 'stdin mnemonic',
  });
});

test('wallet helper rejects an oversized secret stdin object', async () => {
  const input = Readable.from([
    JSON.stringify({
      mnemonic: 'x'.repeat(1_000_001),
    }),
  ]);
  await assert.rejects(
    readWalletHelperSecretsFromStdin(input),
    /exceeded 1000000 bytes/
  );
});

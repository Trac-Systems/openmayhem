import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import b4a from 'b4a';
import PeerWallet from 'trac-wallet';
import { resolvePeerWalletFiles, loadExplicitPeerWallet } from '../src/peer-wallet-files.js';

async function fixture(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'peer-wallet-files-'));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const wallet = new PeerWallet();
  await wallet.ready;
  await wallet.generateKeyPair();
  const keyFile = path.join(dir, 'encrypted.json');
  const passwordFile = path.join(dir, 'password');
  wallet.exportToFile(keyFile, b4a.from(' fixture password '));
  fs.chmodSync(keyFile, 0o600);
  fs.writeFileSync(passwordFile, ' fixture password \r\n', { mode: 0o600 });
  const files = { keyFile, passwordFile, publicKey: b4a.toString(wallet.publicKey, 'hex') };
  return { dir, wallet, files };
}

test('default peer wallet behavior remains opt-in; partial and inline options fail', () => {
  assert.equal(resolvePeerWalletFiles({}), null);
  for (const flags of [
    { 'peer-wallet-key-file': '/tmp/key' },
    { 'peer-wallet-password-file': true },
    { 'peer-wallet-key-file': 'relative', 'peer-wallet-password-file': '/tmp/p', 'peer-wallet-public-key': '1'.repeat(64) },
  ]) assert.throws(() => resolvePeerWalletFiles(flags));
});

test('loads real encrypted wallet without copying, rewriting or exporting it', async t => {
  const { dir, wallet, files } = await fixture(t);
  const before = fs.readFileSync(files.keyFile);
  const loaded = new PeerWallet();
  await loadExplicitPeerWallet(loaded, files);
  assert.deepEqual(loaded.publicKey, wallet.publicKey);
  assert.deepEqual(loaded.sign(b4a.from('identity-check')), wallet.sign(b4a.from('identity-check')));
  assert.deepEqual(fs.readFileSync(files.keyFile), before);
  assert.deepEqual(fs.readdirSync(dir).sort(), ['encrypted.json', 'password']);
});

test('wrong identity/password, exposed files, links and unsupported ACL checks fail closed', async t => {
  const { dir, files } = await fixture(t);
  const rejection = /Unable to load the protected peer wallet with its expected public identity/;
  const check = async (input = files, options) => {
    await assert.rejects(loadExplicitPeerWallet(new PeerWallet(), input, options), rejection);
  };
  await check({ ...files, publicKey: '0'.repeat(64) });
  fs.writeFileSync(files.passwordFile, 'WRONG_SECRET_DO_NOT_LOG');
  await check();
  fs.writeFileSync(files.passwordFile, ' fixture password ');
  fs.chmodSync(files.passwordFile, 0o644);
  await check();
  fs.chmodSync(files.passwordFile, 0o600);
  const link = path.join(dir, 'link');
  fs.symlinkSync(files.keyFile, link);
  await check({ ...files, keyFile: link });
  fs.unlinkSync(link);
  fs.linkSync(files.keyFile, link);
  await check();
  fs.unlinkSync(link);
  await check(files, { platform: 'win32' });
  fs.writeFileSync(files.passwordFile, 'x'.repeat(8193));
  await check();
});

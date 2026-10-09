import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { generateKeyPairSync, createPublicKey } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import Wallet from 'trac-wallet';
import cryptoApi from 'trac-crypto-api';
import { secp256k1 } from 'ethereum-cryptography/secp256k1';
import { keccak256 } from 'ethereum-cryptography/keccak';
import { exportCustodyInventory, verifyCustodyInventory, readCustodyFile } from '../scripts/proxy-admission-custody.mjs';

const purpose = 'proxy_admission_receiver';
const script = fileURLToPath(new URL('../scripts/proxy-admission-custody.mjs', import.meta.url));
function fixture(t) {
  const dir = fs.mkdtempSync(path.join(fs.realpathSync(os.tmpdir()), 'mayhem-custody-test-'));
  fs.chmodSync(dir, 0o700); t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const password = Buffer.from('generated-fixture-password-never-a-live-key');
  const write = (name, data, mode = 0o600) => { const file = path.join(dir, name); fs.writeFileSync(file, data, { mode }); return file; };
  const password_file = write('password', password);
  const tnk = generateKeyPairSync('ed25519').privateKey;
  const tap = generateKeyPairSync('ec', { namedCurve: 'secp256k1' }).privateKey;
  const pub = Buffer.from(createPublicKey(tnk).export({ format: 'jwk' }).x, 'base64url');
  const tapSecret = Buffer.from(tap.export({ format: 'jwk' }).d, 'base64url');
  const tapAddress = `0x${Buffer.from(keccak256(secp256k1.getPublicKey(tapSecret, false).subarray(1))).subarray(12).toString('hex')}`;
  const receivers = [
    { collection: { rail: 'TNK', network: 'testnet1', destination: Wallet.encodeBech32m('testtrac', pub) }, custody_reference: 'tnk-fixture',
      format: 'encrypted_pkcs8', key_file: write('tnk.pem', tnk.export({ type: 'pkcs8', format: 'pem', cipher: 'aes-256-cbc', passphrase: password })), password_file },
    { collection: { rail: 'TAP', chain_id: 31337, token_contract: `0x${'1'.repeat(40)}`, destination: tapAddress }, custody_reference: 'tap-fixture',
      format: 'encrypted_pkcs8', key_file: write('tap.pem', tap.export({ type: 'pkcs8', format: 'pem', cipher: 'aes-256-cbc', passphrase: password })), password_file },
  ];
  return { dir, write, tnk, pub, password, config: { schema_version: 1, purpose, receivers } };
}
test('existing encrypted TNK/TAP keys produce independently verified public ownership; CLI exports no secrets', async t => {
  const f = fixture(t), result = await exportCustodyInventory(f.config);
  assert.deepEqual(verifyCustodyInventory(result), result);
  const configPath = f.write('config.json', JSON.stringify(f.config));
  const cli = spawnSync(process.execPath, [script, 'export', configPath], { encoding: 'utf8', timeout: 10000 });
  assert.equal(cli.status, 0, cli.stderr); assert.equal(cli.stderr, '');
  const publicResult = verifyCustodyInventory(JSON.parse(cli.stdout)); assert.equal(publicResult.receivers.length, 2);
  for (const secret of [f.dir, f.password.toString(), 'password_file', 'key_file', 'PRIVATE KEY']) assert.equal(cli.stdout.includes(secret), false);
  const publicFile = f.write('public.json', cli.stdout, 0o644);
  assert.equal(spawnSync(process.execPath, [script, 'verify', publicFile], { encoding: 'utf8', timeout: 10000 }).status, 0);
});
test('existing Trac wallet encryption is compatible without re-opening the validated key path', async t => {
  const f = fixture(t), seed = Buffer.from(f.tnk.export({ format: 'jwk' }).d, 'base64url');
  const encrypted = cryptoApi.data.encrypt(Buffer.from(JSON.stringify({ publicKey: f.pub.toString('hex'), secretKey: Buffer.concat([seed,f.pub]).toString('hex'), mnemonic: null, derivationPath: null })), f.password);
  f.config.receivers = [f.config.receivers[0]];
  f.config.receivers[0].format = 'trac_wallet';
  f.config.receivers[0].key_file = f.write('trac.json', JSON.stringify(Object.fromEntries(Object.entries(encrypted).map(([k,v]) => [k,v.toString('hex')]))));
  f.config.receivers[0].collection = { rail: 'TNK', network: 'mainnet', destination: Wallet.encodeBech32m('trac', f.pub) };
  assert.equal((await exportCustodyInventory(f.config)).receivers[0].claim.collection.network, 'mainnet');
});
test('claim/signature tampering and duplicated physical destinations fail closed', async t => {
  const f = fixture(t), result = await exportCustodyInventory(f.config);
  for (const change of [
    r => { r.receivers[0].claim.custody_reference = 'changed'; },
    r => { r.receivers[0].claim.public_key = 'ab'.repeat(32); },
    r => { r.receivers[0].signature = '00'.repeat(64); },
    r => { r.receivers[0].claim.purpose = 'retail_payment'; },
    r => { r.receivers[1].claim.collection.chain_id = 1; },
    r => { r.receivers[1].claim.collection.token_contract = `0x${'2'.repeat(40)}`; },
    r => { r.receivers[0].private_key = 'never-allowed'; },
    r => { r.receivers.push(structuredClone(r.receivers[0])); },
  ]) { const changed = structuredClone(result); change(changed); assert.throws(() => verifyCustodyInventory(changed)); }
  f.config.receivers.push({ ...f.config.receivers[1], custody_reference: 'another-reference', collection: { destination: f.config.receivers[1].collection.destination,
    token_contract: `0x${'3'.repeat(40)}`, chain_id: 31337, rail: 'TAP' } });
  await assert.rejects(exportCustodyInventory(f.config), /duplicate/);
});
test('wrong password, address, network, unsafe files and oversized input cannot export an inventory', async t => {
  const f = fixture(t);
  for (const mutate of [
    c => { c.receivers[0].password_file = f.write('wrong-password', 'different'); },
    c => { c.receivers[0].collection.network = 'mainnet'; },
    c => { c.receivers[1].collection.destination = `0x${'2'.repeat(40)}`; },
    c => { c.receivers[0].key_file = f.write('public-key-file', 'not-secret', 0o644); },
  ]) { const c = structuredClone(f.config); mutate(c); await assert.rejects(exportCustodyInventory(c)); }
  const symlink = path.join(f.dir, 'link'); fs.symlinkSync(f.config.receivers[0].key_file, symlink);
  assert.throws(() => readCustodyFile(symlink));
  const link = path.join(f.dir, 'hardlink'); fs.linkSync(f.config.receivers[1].key_file, link);
  assert.throws(() => readCustodyFile(link));
  assert.throws(() => readCustodyFile(f.write('big', Buffer.alloc(17)), { max: 16 }));
  const c = f.write('bad.json', JSON.stringify({ ...f.config, private: 'never-echo-this' }));
  const cli = spawnSync(process.execPath, [script, 'export', c], { encoding: 'utf8', timeout: 10000 });
  assert.equal(cli.status, 1); assert.equal(cli.stdout, ''); assert.equal(cli.stderr.includes(f.dir), false); assert.equal(cli.stderr.includes('never-echo-this'), false);
});

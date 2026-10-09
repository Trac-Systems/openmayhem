#!/usr/bin/env node
// Offline platform-operator inventory. No network, payment, issuer or provider
// wallet authority is exposed to SITE. Existing custody keys remain on this host.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createPrivateKey, createPublicKey, sign, verify } from 'node:crypto';
import { secp256k1 } from 'ethereum-cryptography/secp256k1';
import { keccak256 } from 'ethereum-cryptography/keccak';
import Wallet from 'trac-wallet';
import tracCryptoApi from 'trac-crypto-api';
import { proxyCanonicalSigningBytes } from '../contract/proxy-protocol.js';
import { shape, need, uint } from './proxy-admission-wire.mjs';

const PURPOSE = 'proxy_admission_receiver';
const DOMAIN = 'mayhem/proxy/admission-receiver-custody/v1';
const MAX_BYTES = 256 * 1024;
const opaque = v => typeof v === 'string' && /^[A-Za-z0-9_-]{1,128}$/.test(v);
const eth = v => typeof v === 'string' && /^0x[0-9a-f]{40}$/.test(v) && v !== `0x${'0'.repeat(40)}`;
const hex = (v, n) => typeof v === 'string' && v.length === n * 2 && /^[0-9a-f]+$/.test(v);

/** Open one bounded regular file without following a symlink. Private input is
 * owner-only; Unix checks cannot pretend to validate Windows ACLs. */
export function readCustodyFile(file, { privateInput = true, max = MAX_BYTES } = {}) {
  need(typeof file === 'string' && path.isAbsolute(file) && fs.realpathSync(file) === path.resolve(file), 'custody file must be a canonical absolute path');
  need(process.platform !== 'win32' && typeof process.getuid === 'function', 'custody import requires supported owner-only file protection');
  const fd = fs.openSync(file, fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW);
  try {
    const stat = fs.fstatSync(fd);
    need(stat.isFile() && stat.nlink === 1 && stat.size > 0 && stat.size <= max, 'invalid bounded custody file');
    if (privateInput) need(stat.uid === process.getuid() && (stat.mode & 0o077) === 0, 'custody file must be owner-only');
    const data = Buffer.alloc(stat.size + 1);
    try {
      let size = 0, read;
      while (size < data.length && (read = fs.readSync(fd, data, size, data.length - size, size)) > 0) size += read;
      const after = fs.fstatSync(fd);
      need(size === stat.size && after.size === stat.size && after.mtimeMs === stat.mtimeMs && after.ctimeMs === stat.ctimeMs, 'custody file changed');
      return data.subarray(0, size);
    } catch (error) { data.fill(0); throw error; }
  } finally { fs.closeSync(fd); }
}

function collection(c) {
  if (c?.rail === 'TNK') {
    shape(c, ['rail', 'network', 'destination']);
    need(['mainnet', 'testnet1'].includes(c.network), 'invalid custody network');
    const prefix = c.network === 'mainnet' ? 'trac' : 'testtrac';
    need(typeof c.destination === 'string' && c.destination.startsWith(`${prefix}1`)
      && Wallet.encodeBech32m(prefix, Wallet.decodeBech32m(c.destination)) === c.destination, 'invalid custody destination');
  } else {
    shape(c, ['rail', 'chain_id', 'token_contract', 'destination']);
    need(c.rail === 'TAP' && uint(c.chain_id, 1) && eth(c.token_contract) && eth(c.destination), 'invalid custody asset');
  }
  return c;
}
function claim(raw) {
  shape(raw, ['schema_version', 'purpose', 'collection', 'custody_reference', 'public_key']);
  need(raw.schema_version === 1 && raw.purpose === PURPOSE && opaque(raw.custody_reference), 'invalid custody claim');
  const c = collection(raw.collection), key = raw.public_key;
  need(hex(key, c.rail === 'TNK' ? 32 : 65), 'invalid custody public key');
  const expected = c.rail === 'TNK' ? Wallet.encodeBech32m(c.network === 'mainnet' ? 'trac' : 'testtrac', Buffer.from(key, 'hex'))
    : `0x${Buffer.from(keccak256(Buffer.from(key, 'hex').subarray(1))).subarray(12).toString('hex')}`;
  need(c.destination === expected && (c.rail === 'TNK' || key.startsWith('04')), 'custody public key does not own destination');
  return raw;
}
function bytes(v) { return proxyCanonicalSigningBytes(DOMAIN, v); }
export function verifyCustodyInventory(raw) {
  shape(raw, ['schema_version', 'purpose', 'receivers']);
  need(raw.schema_version === 1 && raw.purpose === PURPOSE && Array.isArray(raw.receivers)
    && raw.receivers.length > 0 && raw.receivers.length <= 128 && Buffer.byteLength(JSON.stringify(raw)) <= MAX_BYTES, 'invalid custody inventory');
  const seen = new Set(), references = new Set();
  for (const entry of raw.receivers) {
    shape(entry, ['claim', 'signature']); const c = claim(entry.claim), key = Buffer.from(c.public_key, 'hex');
    need(hex(entry.signature, 64), 'invalid custody signature');
    const signature = Buffer.from(entry.signature, 'hex');
    const valid = c.collection.rail === 'TNK' ? verify(null, bytes(c), createPublicKey({ key: Buffer.concat([
      Buffer.from('302a300506032b6570032100', 'hex'), key]), format: 'der', type: 'spki' }), signature)
      : secp256k1.verify(signature, keccak256(bytes(c)), key, { lowS: true });
    need(valid, 'custody signature does not match');
    // Same receiving address cannot appear twice with reordered object keys or
    // different TAP token contracts on the same chain.
    const id = c.collection.rail === 'TNK' ? `tnk/${c.collection.network}/${c.collection.destination}`
      : `tap/${c.collection.chain_id}/${c.collection.destination}`;
    need(!seen.has(id) && !references.has(c.custody_reference), 'duplicate custody inventory');
    seen.add(id); references.add(c.custody_reference);
  }
  return raw;
}

/** Attest control of EXISTING receiving keys. This never generates a wallet or
 * sends money. A custody reference is an opaque backup/operator inventory ID,
 * not a filename. Private paths and passwords are excluded from the result. */
export async function exportCustodyInventory(config) {
  shape(config, ['schema_version', 'purpose', 'receivers']);
  need(config.schema_version === 1 && config.purpose === PURPOSE && Array.isArray(config.receivers)
    && config.receivers.length > 0 && config.receivers.length <= 128, 'invalid private custody configuration');
  const receivers = [];
  for (const item of config.receivers) {
    shape(item, ['collection', 'custody_reference', 'format', 'key_file', 'password_file']);
    const c = collection(item.collection);
    need(opaque(item.custody_reference) && ['encrypted_pkcs8', 'trac_wallet'].includes(item.format)
      && (item.format !== 'trac_wallet' || c.rail === 'TNK'), 'invalid private custody profile');
    const password = readCustodyFile(item.password_file, { max: 4096 });
    need(password.length > 0, 'an explicit custody password is required');
    let privateBytes, rawSecret, decrypted;
    try {
      privateBytes = readCustodyFile(item.key_file, { max: 16384 });
      let publicKey, signer;
      if (item.format === 'trac_wallet') {
        // Use the existing Trac encryption with the bytes already read from the
        // protected descriptor. Do not reopen a path after validating its owner.
        const encrypted = JSON.parse(privateBytes.toString('utf8'));
        shape(encrypted, ['salt', 'nonce', 'ciphertext']);
        for (const value of Object.values(encrypted)) need(typeof value === 'string' && /^(?:[0-9a-f]{2})+$/.test(value), 'invalid Trac keystore');
        need(hex(encrypted.salt, 16) && hex(encrypted.nonce, 24) && encrypted.ciphertext.length >= 32, 'invalid Trac encryption dimensions');
        decrypted = tracCryptoApi.data.decrypt(Object.fromEntries(Object.entries(encrypted).map(([k,v]) => [k,Buffer.from(v,'hex')])), password);
        const key = JSON.parse(decrypted.toString('utf8'));
        need(hex(key.publicKey, 32) && hex(key.secretKey, 64), 'invalid Trac key pair');
        rawSecret = Buffer.from(key.secretKey, 'hex');
        publicKey = key.publicKey; signer = data => Buffer.from(Wallet.sign(data, rawSecret)).toString('hex');
      } else {
        need(privateBytes.toString('ascii', 0, 40).startsWith('-----BEGIN ENCRYPTED PRIVATE KEY-----'), 'encrypted PKCS8 required');
        const key = createPrivateKey({ key: privateBytes, format: 'pem', passphrase: password });
        const jwk = createPublicKey(key).export({ format: 'jwk' });
        if (c.rail === 'TNK') {
          need(key.asymmetricKeyType === 'ed25519' && jwk.crv === 'Ed25519', 'TNK custody key must be Ed25519');
          publicKey = Buffer.from(jwk.x, 'base64url').toString('hex'); signer = data => sign(null, data, key).toString('hex');
        } else {
          need(key.asymmetricKeyType === 'ec' && jwk.crv === 'secp256k1', 'TAP custody key must be secp256k1');
          rawSecret = Buffer.from(key.export({ format: 'jwk' }).d, 'base64url'); publicKey = Buffer.from(secp256k1.getPublicKey(rawSecret, false)).toString('hex');
          signer = data => secp256k1.sign(keccak256(data), rawSecret, { lowS: true }).toCompactHex();
        }
      }
      const body = claim({ schema_version: 1, purpose: PURPOSE, collection: c, custody_reference: item.custody_reference, public_key: publicKey });
      receivers.push({ claim: body, signature: signer(bytes(body)) });
    } finally { password.fill(0); privateBytes?.fill(0); rawSecret?.fill(0); decrypted?.fill(0); }
  }
  return verifyCustodyInventory({ schema_version: 1, purpose: PURPOSE, receivers });
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const [action, file, ...extra] = process.argv.slice(2);
    need(['export', 'verify'].includes(action) && file && extra.length === 0, 'use export|verify with one explicit file');
    const raw = readCustodyFile(path.resolve(file), { privateInput: action === 'export' });
    let input;
    try { input = JSON.parse(raw.toString('utf8')); } finally { raw.fill(0); }
    const result = action === 'export' ? await exportCustodyInventory(input) : verifyCustodyInventory(input);
    process.stdout.write(JSON.stringify(result) + '\n');
  } catch { process.stderr.write('Admission custody inventory rejected; inspect local configuration and file protection.\n'); process.exitCode = 1; }
}

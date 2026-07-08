#!/usr/bin/env node
import fs from 'fs';
import path from 'path';
import crypto from 'node:crypto';
import { createRequire } from 'node:module';
import { fileURLToPath, pathToFileURL } from 'url';

const helperPath = fileURLToPath(import.meta.url);
const repoRoot = path.resolve(path.dirname(helperPath), '../../..');
const intercomNodeModules = path.join(repoRoot, 'intercom', 'node_modules');
const contractsRequire = createRequire(path.join(repoRoot, 'contracts', 'package.json'));

const walletModule = await import(
  pathToFileURL(path.join(intercomNodeModules, 'trac-wallet', 'index.js')).href
);
const b4aModule = await import(pathToFileURL(path.join(intercomNodeModules, 'b4a', 'index.js')).href);
const ethersModule = await import(pathToFileURL(contractsRequire.resolve('ethers')).href);

const Wallet = walletModule.default;
const b4a = b4aModule.default;
const { HDNodeWallet, Wallet: EthereumWallet } = ethersModule;
const ETHEREUM_DERIVATION_PATH = "m/44'/60'/0'/0/0";
const ETHEREUM_SIDECAR_VERSION = 1;

const args = process.argv.slice(2);

const usage = () => {
  console.error(`Usage:
  wallet-helper.mjs create --keypair <path> [--password <value>] [--mnemonic <phrase>] [--ethereum-private-key <0x...>] [--ethereum-mnemonic <phrase>] [--force]
  wallet-helper.mjs inspect --keypair <path> [--password <value>] [--network-prefix <prefix>]
  wallet-helper.mjs backup --keypair <path> [--password <value>]
  wallet-helper.mjs passwd --keypair <path> [--password <old>] --new-password <new>
  wallet-helper.mjs eth-key --keypair <path> [--password <value>]
  wallet-helper.mjs seed --keypair <path> [--password <value>]
  wallet-helper.mjs sign --keypair <path> [--password <value>] (--message <text> | --message-hex <hex>)`);
};

const fail = (message) => {
  console.error(message);
  process.exit(1);
};

const takeOption = (name) => {
  const idx = args.indexOf(name);
  if (idx === -1) return null;
  const value = args[idx + 1];
  if (value === undefined || value.startsWith('--')) fail(`Missing value for ${name}.`);
  args.splice(idx, 2);
  return value;
};

const takeFlag = (name) => {
  const idx = args.indexOf(name);
  if (idx === -1) return false;
  args.splice(idx, 1);
  return true;
};

const command = args.shift();
if (!command) {
  usage();
  process.exit(2);
}

const keypairPath = takeOption('--keypair');
const password = takeOption('--password') ?? '';
const networkPrefix = takeOption('--network-prefix');
const newPassword = takeOption('--new-password');
const mnemonic = takeOption('--mnemonic');
const ethereumPrivateKey = takeOption('--ethereum-private-key');
const ethereumMnemonic = takeOption('--ethereum-mnemonic');
const force = takeFlag('--force');
const message = takeOption('--message');
const messageHex = takeOption('--message-hex');

if (!keypairPath) fail('Missing --keypair.');
if (args.length > 0) fail(`Unknown argument: ${args[0]}`);

const passwordBuffer = b4a.from(password, 'utf8');
const ethereumSidecarPath = `${keypairPath}.ethereum.json`;

const quietConsoleLog = async (fn) => {
  const original = console.log;
  console.log = () => {};
  try {
    return await fn();
  } finally {
    console.log = original;
  }
};

const loadWallet = async () => {
  const wallet = new Wallet(networkPrefix ? { networkPrefix } : undefined);
  await wallet.ready;
  await quietConsoleLog(async () => wallet.importFromFile(keypairPath, passwordBuffer));
  return wallet;
};

const normalizeEthereumPrivateKey = (privateKey) => {
  try {
    return new EthereumWallet(String(privateKey ?? '').trim()).privateKey;
  } catch (_error) {
    fail('--ethereum-private-key must be a 0x-prefixed 32-byte Ethereum private key.');
  }
};

const ethereumFromMnemonic = (phrase) => {
  const text = String(phrase ?? '').trim();
  if (!text) return null;
  try {
    const wallet = HDNodeWallet.fromPhrase(text, undefined, ETHEREUM_DERIVATION_PATH);
    return {
      address: wallet.address,
      private_key: wallet.privateKey,
      derivation_path: ETHEREUM_DERIVATION_PATH,
      source: 'mnemonic',
    };
  } catch (_error) {
    return null;
  }
};

const ethereumFromPrivateKey = (privateKey, source = 'imported_private_key') => {
  const wallet = new EthereumWallet(normalizeEthereumPrivateKey(privateKey));
  return {
    address: wallet.address,
    private_key: wallet.privateKey,
    derivation_path: null,
    source,
  };
};

const ethereumSidecarKey = (salt, sourcePasswordBuffer = passwordBuffer) =>
  crypto.scryptSync(sourcePasswordBuffer, salt, 32);

const writeEthereumSidecar = (account, targetPasswordBuffer = passwordBuffer) => {
  fs.mkdirSync(path.dirname(ethereumSidecarPath), { recursive: true });
  const salt = crypto.randomBytes(16);
  const nonce = crypto.randomBytes(12);
  const key = ethereumSidecarKey(salt, targetPasswordBuffer);
  const cipher = crypto.createCipheriv('aes-256-gcm', key, nonce);
  const plaintext = Buffer.from(JSON.stringify({
    private_key: account.private_key,
    source: account.source,
    created_at: new Date().toISOString(),
  }));
  const ciphertext = Buffer.concat([cipher.update(plaintext), cipher.final()]);
  const tag = cipher.getAuthTag();
  fs.writeFileSync(ethereumSidecarPath, JSON.stringify({
    version: ETHEREUM_SIDECAR_VERSION,
    kdf: 'scrypt',
    cipher: 'aes-256-gcm',
    salt: salt.toString('hex'),
    nonce: nonce.toString('hex'),
    tag: tag.toString('hex'),
    ciphertext: ciphertext.toString('hex'),
  }));
};

const readEthereumSidecar = (sourcePasswordBuffer = passwordBuffer) => {
  if (!fs.existsSync(ethereumSidecarPath)) return null;
  const file = JSON.parse(fs.readFileSync(ethereumSidecarPath, 'utf8'));
  if (file.version !== ETHEREUM_SIDECAR_VERSION || file.cipher !== 'aes-256-gcm') {
    throw new Error('Unsupported Ethereum wallet sidecar format.');
  }
  const salt = Buffer.from(file.salt, 'hex');
  const nonce = Buffer.from(file.nonce, 'hex');
  const tag = Buffer.from(file.tag, 'hex');
  const ciphertext = Buffer.from(file.ciphertext, 'hex');
  const key = ethereumSidecarKey(salt, sourcePasswordBuffer);
  const decipher = crypto.createDecipheriv('aes-256-gcm', key, nonce);
  decipher.setAuthTag(tag);
  const plaintext = Buffer.concat([decipher.update(ciphertext), decipher.final()]);
  const data = JSON.parse(plaintext.toString('utf8'));
  return ethereumFromPrivateKey(data.private_key, data.source ?? 'imported_private_key');
};

const importedEthereumAccount = () => {
  if (ethereumPrivateKey !== null && ethereumMnemonic !== null) {
    fail('Pass only one of --ethereum-private-key or --ethereum-mnemonic.');
  }
  if (ethereumPrivateKey !== null) return ethereumFromPrivateKey(ethereumPrivateKey);
  if (ethereumMnemonic !== null) {
    const account = ethereumFromMnemonic(ethereumMnemonic);
    if (!account) fail('--ethereum-mnemonic must be a valid BIP-39 mnemonic.');
    return { ...account, source: 'imported_mnemonic' };
  }
  return null;
};

const resolveEthereumAccount = (wallet, sourcePasswordBuffer = passwordBuffer) => {
  const sidecar = readEthereumSidecar(sourcePasswordBuffer);
  if (sidecar) return sidecar;
  return ethereumFromMnemonic(wallet.mnemonic);
};

const walletJson = (
  wallet,
  created,
  includeMnemonic = false,
  includeEthereumPrivateKey = false,
  sourcePasswordBuffer = passwordBuffer
) => {
  const ethereum = resolveEthereumAccount(wallet, sourcePasswordBuffer);
  return {
    created,
    keypair_path: path.resolve(keypairPath),
    public_key: b4a.toString(wallet.publicKey, 'hex'),
    address: wallet.address ?? null,
    derivation_path: wallet.derivationPath ?? null,
    ethereum_address: ethereum?.address ?? null,
    ethereum_derivation_path: ethereum?.derivation_path ?? null,
    ethereum_source: ethereum?.source ?? null,
    ethereum_private_key: includeEthereumPrivateKey ? ethereum?.private_key ?? null : null,
    mnemonic: includeMnemonic ? wallet.mnemonic ?? null : null,
  };
};

if (command === 'create') {
  if (fs.existsSync(keypairPath) && !force) {
    const wallet = await loadWallet();
    console.log(JSON.stringify(walletJson(wallet, false, false)));
    process.exit(0);
  }

  fs.mkdirSync(path.dirname(keypairPath), { recursive: true });
  const wallet = new Wallet();
  await wallet.ready;
  await wallet.generateKeyPair(mnemonic ?? null);
  await quietConsoleLog(async () => wallet.exportToFile(keypairPath, passwordBuffer));
  const importedEthereum = importedEthereumAccount();
  if (importedEthereum) {
    writeEthereumSidecar(importedEthereum);
  } else if (fs.existsSync(ethereumSidecarPath)) {
    fs.unlinkSync(ethereumSidecarPath);
  }
  console.log(JSON.stringify(walletJson(wallet, true, true)));
  process.exit(0);
}

if (command === 'inspect') {
  const wallet = await loadWallet();
  console.log(JSON.stringify(walletJson(wallet, false, false)));
  process.exit(0);
}

if (command === 'backup') {
  const wallet = await loadWallet();
  const ethereum = resolveEthereumAccount(wallet);
  console.log(JSON.stringify(walletJson(wallet, false, true, ethereum?.source !== 'mnemonic')));
  process.exit(0);
}

if (command === 'passwd') {
  if (newPassword === null) fail('Missing --new-password.');

  const wallet = await loadWallet();
  const ethereum = resolveEthereumAccount(wallet);
  const nextPasswordBuffer = b4a.from(newPassword, 'utf8');
  const tmpPath = `${keypairPath}.tmp-${process.pid}-${Date.now()}`;
  try {
    await quietConsoleLog(async () => wallet.exportToFile(tmpPath, nextPasswordBuffer));
    fs.renameSync(tmpPath, keypairPath);
    if (ethereum?.source !== 'mnemonic') {
      writeEthereumSidecar(ethereum, nextPasswordBuffer);
    }
  } catch (err) {
    try {
      if (fs.existsSync(tmpPath)) fs.unlinkSync(tmpPath);
    } catch {}
    throw err;
  }
  console.log(JSON.stringify(walletJson(wallet, false, false, false, nextPasswordBuffer)));
  process.exit(0);
}

if (command === 'eth-key') {
  const wallet = await loadWallet();
  const ethereum = resolveEthereumAccount(wallet);
  if (!ethereum?.private_key) fail('Wallet does not contain a restorable Ethereum account.');
  console.log(JSON.stringify({
    address: ethereum.address,
    private_key: ethereum.private_key,
    derivation_path: ethereum.derivation_path,
    source: ethereum.source,
  }));
  process.exit(0);
}

if (command === 'seed') {
  const wallet = await loadWallet();
  console.log(JSON.stringify({
    public_key: b4a.toString(wallet.publicKey, 'hex'),
    signing_seed_hex: b4a.toString(wallet.secretKey.subarray(0, 32), 'hex')
  }));
  process.exit(0);
}

if (command === 'sign') {
  if ((message === null && messageHex === null) || (message !== null && messageHex !== null)) {
    fail('Provide exactly one of --message or --message-hex.');
  }

  const wallet = await loadWallet();
  const messageBuffer =
    messageHex !== null ? b4a.from(messageHex, 'hex') : b4a.from(message ?? '', 'utf8');
  const signature = wallet.sign(messageBuffer);
  console.log(JSON.stringify({ signature: b4a.toString(signature, 'hex') }));
  process.exit(0);
}

usage();
process.exit(2);

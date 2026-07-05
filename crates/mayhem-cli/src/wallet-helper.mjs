#!/usr/bin/env node
import fs from 'fs';
import path from 'path';
import { fileURLToPath, pathToFileURL } from 'url';

const helperPath = fileURLToPath(import.meta.url);
const repoRoot = path.resolve(path.dirname(helperPath), '../../..');
const intercomNodeModules = path.join(repoRoot, 'intercom', 'node_modules');

const walletModule = await import(
  pathToFileURL(path.join(intercomNodeModules, 'trac-wallet', 'index.js')).href
);
const b4aModule = await import(pathToFileURL(path.join(intercomNodeModules, 'b4a', 'index.js')).href);

const Wallet = walletModule.default;
const b4a = b4aModule.default;

const args = process.argv.slice(2);

const usage = () => {
  console.error(`Usage:
  wallet-helper.mjs create --keypair <path> [--password <value>] [--mnemonic <phrase>] [--force]
  wallet-helper.mjs inspect --keypair <path> [--password <value>]
  wallet-helper.mjs backup --keypair <path> [--password <value>]
  wallet-helper.mjs passwd --keypair <path> [--password <old>] --new-password <new>
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
const newPassword = takeOption('--new-password');
const mnemonic = takeOption('--mnemonic');
const force = takeFlag('--force');
const message = takeOption('--message');
const messageHex = takeOption('--message-hex');

if (!keypairPath) fail('Missing --keypair.');
if (args.length > 0) fail(`Unknown argument: ${args[0]}`);

const passwordBuffer = b4a.from(password, 'utf8');

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
  const wallet = new Wallet();
  await wallet.ready;
  await quietConsoleLog(async () => wallet.importFromFile(keypairPath, passwordBuffer));
  return wallet;
};

const walletJson = (wallet, created, includeMnemonic = false) => ({
  created,
  keypair_path: path.resolve(keypairPath),
  public_key: b4a.toString(wallet.publicKey, 'hex'),
  address: wallet.address ?? null,
  derivation_path: wallet.derivationPath ?? null,
  mnemonic: includeMnemonic ? wallet.mnemonic ?? null : null,
});

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
  console.log(JSON.stringify(walletJson(wallet, false, true)));
  process.exit(0);
}

if (command === 'passwd') {
  if (newPassword === null) fail('Missing --new-password.');

  const wallet = await loadWallet();
  const nextPasswordBuffer = b4a.from(newPassword, 'utf8');
  const tmpPath = `${keypairPath}.tmp-${process.pid}-${Date.now()}`;
  try {
    await quietConsoleLog(async () => wallet.exportToFile(tmpPath, nextPasswordBuffer));
    fs.renameSync(tmpPath, keypairPath);
  } catch (err) {
    try {
      if (fs.existsSync(tmpPath)) fs.unlinkSync(tmpPath);
    } catch {}
    throw err;
  }
  console.log(JSON.stringify(walletJson(wallet, false, false)));
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

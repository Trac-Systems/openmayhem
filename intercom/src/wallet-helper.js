import fs from 'fs';
import path from 'path';
import b4a from 'b4a';
import PeerWallet from 'trac-wallet';
import tracCryptoApi from 'trac-crypto-api';
import { mnemonicToSeedSync, validateMnemonic } from 'ethereum-cryptography/bip39';
import { wordlist as englishWordlist } from 'ethereum-cryptography/bip39/wordlists/english';
import { HDKey } from 'ethereum-cryptography/hdkey';
import { keccak256 } from 'ethereum-cryptography/keccak';
import { secp256k1 } from 'ethereum-cryptography/secp256k1';

const ETHEREUM_DERIVATION_PATH = "m/44'/60'/0'/0/0";
const ETHEREUM_SIDECAR_VERSION = 1;
const WALLET_HELPER_SECRET_NAMES = [
  'password',
  'new-password',
  'mnemonic',
  'ethereum-private-key',
  'ethereum-mnemonic',
];

export const walletHelperFlagsWithSecrets = (flags, inherited = {}) => {
  const secretNames = WALLET_HELPER_SECRET_NAMES;
  for (const name of secretNames) {
    if (flags[name] !== undefined) {
      throw new Error(`Wallet helper secret --${name} must not be passed on argv.`);
    }
  }
  if (!inherited || typeof inherited !== 'object' || Array.isArray(inherited)) {
    throw new Error('Wallet helper stdin must contain a JSON object.');
  }
  const allowed = new Set(secretNames.map((name) => name.replaceAll('-', '_')));
  const unknown = Object.keys(inherited).filter((name) => !allowed.has(name));
  if (unknown.length > 0) {
    throw new Error('Wallet helper stdin contains unsupported fields.');
  }
  const merged = { ...flags };
  for (const name of secretNames) {
    const jsonName = name.replaceAll('-', '_');
    const value = inherited[jsonName];
    if (value !== undefined) {
      if (typeof value !== 'string') {
        throw new Error(`Wallet helper secret ${jsonName} must be a string.`);
      }
      merged[name] = value;
    }
  }
  return merged;
};

export const readWalletHelperSecretsFromStdin = async (providedInput = null) => {
  const input = providedInput ??
    globalThis.process?.stdin ??
    (await import('bare-process')).default.stdin;
  return await new Promise((resolve, reject) => {
    if (!input || typeof input.on !== 'function') {
      reject(new Error('Wallet helper requires a secret stdin pipe.'));
      return;
    }
    const chunks = [];
    let bytes = 0;
    let settled = false;
    input.on('data', (chunk) => {
      if (settled) return;
      const buffer = typeof chunk === 'string' ? b4a.from(chunk, 'utf8') : chunk;
      bytes += buffer.length;
      if (bytes > 1_000_000) {
        settled = true;
        input.pause?.();
        reject(new Error('Wallet helper secret stdin exceeded 1000000 bytes.'));
        return;
      }
      chunks.push(buffer);
    });
    input.on('error', (error) => {
      if (settled) return;
      settled = true;
      reject(error);
    });
    input.on('end', () => {
      if (settled) return;
      settled = true;
      try {
        const encoded = b4a.toString(b4a.concat(chunks), 'utf8');
        resolve(JSON.parse(encoded));
      } catch {
        reject(new Error('Wallet helper secret stdin must contain valid JSON.'));
      }
    });
    input.resume?.();
  });
};

const requireString = (flags, name) => {
  const value = flags[name];
  if (value === undefined || value === true || String(value).length === 0) {
    throw new Error(`Missing --${name}.`);
  }
  return String(value);
};

const optionalString = (flags, name) => {
  const value = flags[name];
  if (value === undefined) return null;
  if (value === true) throw new Error(`Missing value for --${name}.`);
  return String(value);
};

const quietConsoleLog = async (fn) => {
  const original = console.log;
  console.log = () => {};
  try {
    return await fn();
  } finally {
    console.log = original;
  }
};

const ethereumAddress = (privateKeyBytes) => {
  const publicKey = secp256k1.getPublicKey(privateKeyBytes, false);
  const lower = b4a.toString(keccak256(publicKey.subarray(1)).subarray(12), 'hex');
  const checksum = b4a.toString(keccak256(b4a.from(lower, 'utf8')), 'hex');
  let address = '';
  for (let index = 0; index < lower.length; index += 1) {
    address += Number.parseInt(checksum[index], 16) >= 8 ? lower[index].toUpperCase() : lower[index];
  }
  return `0x${address}`;
};

const ethereumPersonalMessageHash = (message) => {
  const body = b4a.from(message, 'utf8');
  const prefix = b4a.from(`\x19Ethereum Signed Message:\n${body.length}`, 'utf8');
  return keccak256(b4a.concat([prefix, body]));
};

const ethereumSignMessage = (account, message) => {
  const privateKey = b4a.from(account.private_key.slice(2), 'hex');
  const signature = secp256k1.sign(ethereumPersonalMessageHash(message), privateKey, {
    lowS: true,
  });
  const bytes = b4a.alloc(65);
  bytes.set(signature.toCompactRawBytes(), 0);
  bytes[64] = 27 + signature.recovery;
  return `0x${b4a.toString(bytes, 'hex')}`;
};

const ethereumFromPrivateKey = (
  privateKey,
  source = 'imported_private_key',
  derivationPath = null
) => {
  const normalized = String(privateKey ?? '').trim().toLowerCase();
  if (!/^0x[0-9a-f]{64}$/.test(normalized)) {
    throw new Error('--ethereum-private-key must be a 0x-prefixed 32-byte Ethereum private key.');
  }
  const privateKeyBytes = b4a.from(normalized.slice(2), 'hex');
  if (!secp256k1.utils.isValidPrivateKey(privateKeyBytes)) {
    throw new Error('--ethereum-private-key must be a valid secp256k1 private key.');
  }
  return {
    address: ethereumAddress(privateKeyBytes),
    private_key: normalized,
    derivation_path: derivationPath,
    source,
  };
};

const ethereumFromMnemonic = (phrase, source = 'mnemonic') => {
  const mnemonic = String(phrase ?? '').trim();
  if (!mnemonic || !validateMnemonic(mnemonic, englishWordlist)) return null;
  const child = HDKey.fromMasterSeed(mnemonicToSeedSync(mnemonic)).derive(
    ETHEREUM_DERIVATION_PATH
  );
  if (!child.privateKey) return null;
  return ethereumFromPrivateKey(
    `0x${b4a.toString(child.privateKey, 'hex')}`,
    source,
    ETHEREUM_DERIVATION_PATH
  );
};

const encryptedFileJson = (encrypted) => ({
  version: ETHEREUM_SIDECAR_VERSION,
  cipher: 'xsalsa20-poly1305',
  kdf: 'argon2i',
  nonce: b4a.toString(encrypted.nonce, 'hex'),
  salt: b4a.toString(encrypted.salt, 'hex'),
  ciphertext: b4a.toString(encrypted.ciphertext, 'hex'),
});

const writeEthereumSidecar = (sidecarPath, account, passwordBuffer) => {
  fs.mkdirSync(path.dirname(sidecarPath), { recursive: true });
  const plaintext = b4a.from(
    JSON.stringify({
      private_key: account.private_key,
      source: account.source,
      derivation_path: account.derivation_path,
      created_at: new Date().toISOString(),
    }),
    'utf8'
  );
  const encrypted = tracCryptoApi.data.encrypt(plaintext, passwordBuffer);
  try {
    fs.writeFileSync(sidecarPath, JSON.stringify(encryptedFileJson(encrypted)), { mode: 0o600 });
    fs.chmodSync(sidecarPath, 0o600);
  } finally {
    tracCryptoApi.utils.memzero(plaintext);
    tracCryptoApi.utils.memzero(encrypted.nonce);
    tracCryptoApi.utils.memzero(encrypted.salt);
    tracCryptoApi.utils.memzero(encrypted.ciphertext);
  }
};

const readEthereumSidecar = (sidecarPath, passwordBuffer) => {
  if (!fs.existsSync(sidecarPath)) return null;
  const file = JSON.parse(fs.readFileSync(sidecarPath, 'utf8'));
  if (
    file.version !== ETHEREUM_SIDECAR_VERSION ||
    file.cipher !== 'xsalsa20-poly1305' ||
    file.kdf !== 'argon2i'
  ) {
    throw new Error('Unsupported Ethereum wallet sidecar format.');
  }
  const plaintext = tracCryptoApi.data.decrypt(
    {
      nonce: b4a.from(file.nonce, 'hex'),
      salt: b4a.from(file.salt, 'hex'),
      ciphertext: b4a.from(file.ciphertext, 'hex'),
    },
    passwordBuffer
  );
  try {
    const data = JSON.parse(b4a.toString(plaintext, 'utf8'));
    return ethereumFromPrivateKey(
      data.private_key,
      data.source ?? 'imported_private_key',
      data.derivation_path ?? null
    );
  } finally {
    tracCryptoApi.utils.memzero(plaintext);
  }
};

const importedEthereumAccount = (ethereumPrivateKey, ethereumMnemonic) => {
  if (ethereumPrivateKey !== null && ethereumMnemonic !== null) {
    throw new Error('Pass only one of --ethereum-private-key or --ethereum-mnemonic.');
  }
  if (ethereumPrivateKey !== null) return ethereumFromPrivateKey(ethereumPrivateKey);
  if (ethereumMnemonic !== null) {
    const account = ethereumFromMnemonic(ethereumMnemonic, 'imported_mnemonic');
    if (!account) throw new Error('--ethereum-mnemonic must be a valid BIP-39 mnemonic.');
    return account;
  }
  return null;
};

export async function runWalletHelper(rawFlags, inheritedSecrets = {}) {
  const flags = walletHelperFlagsWithSecrets(rawFlags, inheritedSecrets);
  const command = requireString(flags, 'wallet-helper');
  const keypairPath = requireString(flags, 'keypair');
  const password = optionalString(flags, 'password') ?? '';
  const passwordBuffer = b4a.from(password, 'utf8');
  const networkPrefix = optionalString(flags, 'network-prefix');
  const newPassword = optionalString(flags, 'new-password');
  const mnemonic = optionalString(flags, 'mnemonic');
  const ethereumPrivateKey = optionalString(flags, 'ethereum-private-key');
  const ethereumMnemonic = optionalString(flags, 'ethereum-mnemonic');
  const force = flags.force === true || flags.force === 'true';
  const message = optionalString(flags, 'message');
  const messageHex = optionalString(flags, 'message-hex');
  const ethereumSidecarPath = `${keypairPath}.ethereum.json`;

  const loadWallet = async () => {
    const wallet = new PeerWallet(networkPrefix ? { networkPrefix } : undefined);
    await wallet.ready;
    await quietConsoleLog(async () => wallet.importFromFile(keypairPath, passwordBuffer));
    return wallet;
  };

  const resolveEthereumAccount = (wallet, sourcePasswordBuffer = passwordBuffer) =>
    readEthereumSidecar(ethereumSidecarPath, sourcePasswordBuffer) ??
    ethereumFromMnemonic(wallet.mnemonic);

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
      return walletJson(wallet, false);
    }
    fs.mkdirSync(path.dirname(keypairPath), { recursive: true });
    const wallet = new PeerWallet();
    await wallet.ready;
    await wallet.generateKeyPair(mnemonic);
    await quietConsoleLog(async () => wallet.exportToFile(keypairPath, passwordBuffer));
    fs.chmodSync(keypairPath, 0o600);
    const importedEthereum = importedEthereumAccount(ethereumPrivateKey, ethereumMnemonic);
    if (importedEthereum) {
      writeEthereumSidecar(ethereumSidecarPath, importedEthereum, passwordBuffer);
    } else if (fs.existsSync(ethereumSidecarPath)) {
      fs.unlinkSync(ethereumSidecarPath);
    }
    return walletJson(wallet, true, false, true);
  }

  if (command === 'inspect') {
    return walletJson(await loadWallet(), false);
  }

  if (command === 'backup') {
    const wallet = await loadWallet();
    const ethereum = resolveEthereumAccount(wallet);
    return walletJson(wallet, false, true, ethereum?.source !== 'mnemonic');
  }

  if (command === 'passwd') {
    if (newPassword === null) throw new Error('Missing --new-password.');
    const wallet = await loadWallet();
    const ethereum = resolveEthereumAccount(wallet);
    const nextPasswordBuffer = b4a.from(newPassword, 'utf8');
    const tmpPath = `${keypairPath}.tmp-${Date.now()}-${b4a.toString(
      tracCryptoApi.nonce.generate().subarray(0, 6),
      'hex'
    )}`;
    try {
      await quietConsoleLog(async () => wallet.exportToFile(tmpPath, nextPasswordBuffer));
      fs.chmodSync(tmpPath, 0o600);
      fs.renameSync(tmpPath, keypairPath);
      if (ethereum?.source !== 'mnemonic') {
        writeEthereumSidecar(ethereumSidecarPath, ethereum, nextPasswordBuffer);
      }
    } catch (error) {
      try {
        if (fs.existsSync(tmpPath)) fs.unlinkSync(tmpPath);
      } catch {}
      throw error;
    }
    return walletJson(wallet, false, false, false, nextPasswordBuffer);
  }

  if (command === 'eth-key') {
    const wallet = await loadWallet();
    const ethereum = resolveEthereumAccount(wallet);
    if (!ethereum?.private_key) {
      throw new Error('Wallet does not contain a restorable Ethereum account.');
    }
    return {
      address: ethereum.address,
      private_key: ethereum.private_key,
      derivation_path: ethereum.derivation_path,
      source: ethereum.source,
    };
  }

  if (command === 'eth-sign') {
    if (message === null || messageHex !== null) {
      throw new Error('Ethereum signing requires --message and does not accept --message-hex.');
    }
    const wallet = await loadWallet();
    const ethereum = resolveEthereumAccount(wallet);
    if (!ethereum?.private_key) {
      throw new Error('Wallet does not contain a restorable Ethereum account.');
    }
    return {
      address: ethereum.address,
      signature: ethereumSignMessage(ethereum, message),
    };
  }

  if (command === 'seed') {
    const wallet = await loadWallet();
    return {
      public_key: b4a.toString(wallet.publicKey, 'hex'),
      signing_seed_hex: b4a.toString(wallet.secretKey.subarray(0, 32), 'hex'),
    };
  }

  if (command === 'sign') {
    if ((message === null && messageHex === null) || (message !== null && messageHex !== null)) {
      throw new Error('Provide exactly one of --message or --message-hex.');
    }
    const wallet = await loadWallet();
    const messageBuffer =
      messageHex !== null ? b4a.from(messageHex, 'hex') : b4a.from(message ?? '', 'utf8');
    return { signature: b4a.toString(wallet.sign(messageBuffer), 'hex') };
  }

  throw new Error(`Unsupported wallet helper command: ${command}.`);
}

// Explicit encrypted wallet references for a peer whose transport identity must
// match an existing signing wallet. Never copy/export keys into a peer store.
import fs from 'fs';
import path from 'path';
import b4a from 'b4a';

const flag = (flags, name) => {
  const value = flags[name];
  if (value === undefined) return null;
  if (typeof value !== 'string' || !value.trim()) throw new Error('Invalid peer wallet file options.');
  return value;
};

export const resolvePeerWalletFiles = (flags = {}) => {
  const keyFile = flag(flags, 'peer-wallet-key-file');
  const passwordFile = flag(flags, 'peer-wallet-password-file');
  const publicKey = flag(flags, 'peer-wallet-public-key');
  if (keyFile === null && passwordFile === null && publicKey === null) return null;
  if (!keyFile || !passwordFile || !publicKey || !path.isAbsolute(keyFile) ||
      !path.isAbsolute(passwordFile) || !/^[0-9a-f]{64}$/.test(publicKey) || keyFile === passwordFile) {
    throw new Error('Explicit peer wallets require absolute key/password file references and the expected public key.');
  }
  return Object.freeze({ keyFile, passwordFile, publicKey });
};

const privateFile = (file, maxBytes, platform) => {
  const stat = fs.lstatSync(file);
  // An explicit encrypted-wallet deployment currently requires POSIX file
  // protection. Never treat Windows mode bits as proof of its ACLs.
  if (platform === 'win32' || !stat.isFile() || stat.isSymbolicLink() ||
      stat.nlink !== 1 || (stat.mode & 0o077) !== 0 || stat.size > maxBytes) {
    throw new Error('Peer wallet files require protected regular files.');
  }
  return stat;
};

export const loadExplicitPeerWallet = async (wallet, files, {
  platform = typeof process !== 'undefined' ? process.platform : globalThis.Bare?.platform,
} = {}) => {
  let password;
  try {
    const before = privateFile(files.keyFile, 64 * 1024, platform);
    if (before.size === 0) throw new Error('Empty wallet.');
    privateFile(files.passwordFile, 8192, platform);
    password = fs.readFileSync(files.passwordFile);
    // Match password-file conventions without stripping intentional spaces.
    while (password.length && (password[password.length - 1] === 10 || password[password.length - 1] === 13)) {
      password[password.length - 1] = 0;
      password = password.subarray(0, password.length - 1);
    }
    await wallet.ready;
    await wallet.importFromFile(files.keyFile, password);
    const after = privateFile(files.keyFile, 64 * 1024, platform);
    const actual = typeof wallet.publicKey === 'string' ? wallet.publicKey : b4a.toString(wallet.publicKey, 'hex');
    if (before.dev !== after.dev || before.ino !== after.ino || before.size !== after.size ||
        before.mtimeMs !== after.mtimeMs || actual !== files.publicKey) {
      throw new Error('Peer wallet identity changed.');
    }
  } catch {
    // Keystore/parser errors can contain paths or input. Only a fixed operator
    // diagnosis leaves this boundary; no password, key or mnemonic is logged.
    throw new Error('Unable to load the protected peer wallet with its expected public identity.');
  } finally {
    password?.fill(0);
  }
};

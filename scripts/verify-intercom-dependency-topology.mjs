#!/usr/bin/env node

import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const fail = (message) => {
  throw new Error(`Intercom dependency topology: ${message}`);
};

const readJson = (file) => {
  const stat = fs.lstatSync(file);
  if (!stat.isFile() || stat.isSymbolicLink()) {
    fail(`expected a regular file: ${file}`);
  }
  return JSON.parse(fs.readFileSync(file, 'utf8'));
};

const requestedRoot = process.argv[2];
if (!requestedRoot) {
  fail('usage: verify-intercom-dependency-topology.mjs <intercom-root>');
}

const root = fs.realpathSync(path.resolve(requestedRoot));
const packageJsonPath = path.join(root, 'package.json');
const lockfilePath = path.join(root, 'package-lock.json');
const npmrcPath = path.join(root, '.npmrc');
const manifest = readJson(packageJsonPath);
const lock = readJson(lockfilePath);
const npmrc = fs.readFileSync(npmrcPath, 'utf8');
const materializerPath = path.join(root, 'scripts', 'materialize-local-dependencies.mjs');
const { verifyLocalDependencies } = await import(pathToFileURL(materializerPath).href);

if (!/^\s*install-links\s*=\s*true\s*(?:[#;].*)?$/m.test(npmrc)) {
  fail(`${npmrcPath} must set install-links=true`);
}
if (manifest.dependencies?.['trac-wallet'] !== '1.0.1') {
  fail('root dependencies must pin trac-wallet to 1.0.1');
}
if (Object.prototype.hasOwnProperty.call(manifest.overrides ?? {}, 'trac-wallet')) {
  fail('root overrides must not collapse trac-wallet across pinned MSB/peer dependencies');
}
if (lock.lockfileVersion !== 3 || typeof lock.packages !== 'object') {
  fail('root package-lock.json must use lockfileVersion 3');
}

const localPackages = [
  { name: 'trac-msb', source: 'trac/msb' },
  { name: 'trac-peer', source: 'trac/trac-peer' },
];
for (const { name, source } of localPackages) {
  if (manifest.dependencies?.[name] !== `file:${source}`) {
    fail(`root dependencies must install ${name} from file:${source}`);
  }
  if (fs.existsSync(path.join(root, source, 'node_modules'))) {
    fail(`pinned source must not retain its own dependency tree: ${source}/node_modules`);
  }

  const lockPath = `node_modules/${name}`;
  const locked = lock.packages[lockPath];
  if (!locked || locked.link === true || locked.resolved !== `file:${source}`) {
    fail(`${lockPath} must be a physical root-lock file dependency`);
  }
  if (Object.keys(lock.packages).some((entry) => entry === source || entry.startsWith(`${source}/`))) {
    fail(`root lock retains a source-owned ${name} dependency subtree`);
  }

  const installed = path.join(root, lockPath);
  const installedStat = fs.lstatSync(installed);
  if (installedStat.isSymbolicLink() || !installedStat.isDirectory()) {
    fail(`${lockPath} must be a real directory`);
  }
  if (fs.realpathSync(installed) !== installed) {
    fail(`${lockPath} resolves outside its physical root location`);
  }

  const installedManifest = readJson(path.join(installed, 'package.json'));
  const sourceManifest = readJson(path.join(root, source, 'package.json'));
  if (installedManifest.name !== name ||
      installedManifest.name !== sourceManifest.name ||
      installedManifest.version !== sourceManifest.version ||
      installedManifest.version !== locked.version) {
    fail(`${lockPath} does not match its root lock and pinned source identity`);
  }
}

const walletLockPaths = Object.keys(lock.packages).filter(
  (entry) => entry === 'node_modules/trac-wallet' ||
    entry.endsWith('/node_modules/trac-wallet'),
);
const expectedWalletLocks = new Map([
  ['node_modules/trac-wallet', '1.0.1'],
  ['node_modules/trac-msb/node_modules/trac-wallet', '2.1.0'],
  ['node_modules/trac-peer/node_modules/trac-wallet', '1.0.4'],
]);
if (walletLockPaths.length !== expectedWalletLocks.size) {
  fail(`root lock must contain the three pinned trac-wallet installs, found: ${walletLockPaths.join(', ')}`);
}
for (const [walletPath, expectedVersion] of expectedWalletLocks) {
  if (lock.packages[walletPath]?.version !== expectedVersion) {
    fail(`${walletPath} must install trac-wallet ${expectedVersion}`);
  }
}

const installedPackages = [];
const walletDirectories = [];
const visitNodeModules = (directory) => {
  if (!fs.existsSync(directory)) return;
  const stat = fs.lstatSync(directory);
  if (stat.isSymbolicLink() || !stat.isDirectory()) {
    fail(`node_modules must be a real directory: ${directory}`);
  }

  const visitPackage = (packageRoot, installName) => {
    const packageStat = fs.lstatSync(packageRoot);
    if (packageStat.isSymbolicLink() || !packageStat.isDirectory()) {
      fail(`installed package must be a real directory: ${packageRoot}`);
    }
    const packageManifest = readJson(path.join(packageRoot, 'package.json'));
    installedPackages.push({ root: packageRoot, manifest: packageManifest });
    if (installName === 'trac-wallet') walletDirectories.push(packageRoot);
    visitNodeModules(path.join(packageRoot, 'node_modules'));
  };

  for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
    if (entry.name.startsWith('.')) continue;
    const entryPath = path.join(directory, entry.name);
    if (entry.name.startsWith('@')) {
      const scopeStat = fs.lstatSync(entryPath);
      if (scopeStat.isSymbolicLink() || !scopeStat.isDirectory()) {
        fail(`installed package scope must be a real directory: ${entryPath}`);
      }
      for (const scoped of fs.readdirSync(entryPath, { withFileTypes: true })) {
        visitPackage(path.join(entryPath, scoped.name), `${entry.name}/${scoped.name}`);
      }
    } else {
      visitPackage(entryPath, entry.name);
    }
  }
};

visitNodeModules(path.join(root, 'node_modules'));

const walletRoot = path.join(root, 'node_modules', 'trac-wallet');
const walletRoots = new Map([
  [walletRoot, '1.0.1'],
  [path.join(root, 'node_modules', 'trac-msb', 'node_modules', 'trac-wallet'), '2.1.0'],
  [path.join(root, 'node_modules', 'trac-peer', 'node_modules', 'trac-wallet'), '1.0.4'],
]);
if (walletDirectories.length !== walletRoots.size) {
  fail(`runtime must contain the three pinned trac-wallet installs, found: ${walletDirectories.join(', ')}`);
}
for (const [walletPath, expectedVersion] of walletRoots) {
  if (!walletDirectories.includes(walletPath)) {
    fail(`runtime missing pinned trac-wallet at ${walletPath}`);
  }
  const manifest = readJson(path.join(walletPath, 'package.json'));
  if (manifest.name !== 'trac-wallet' || manifest.version !== expectedVersion) {
    fail(`${walletPath} must be trac-wallet ${expectedVersion}`);
  }
}
const walletModule = await import(pathToFileURL(path.join(walletRoot, 'index.js')).href);
if (typeof walletModule.default?.encodeBech32mSafe !== 'function') {
  fail('installed top-level wallet must expose PeerWallet.encodeBech32mSafe');
}

const dependencySections = [
  'dependencies',
  'optionalDependencies',
  'peerDependencies',
  'devDependencies',
];
const declaresWallet = (packageManifest) => dependencySections.some(
  (section) => Object.prototype.hasOwnProperty.call(
    packageManifest[section] ?? {},
    'trac-wallet',
  ),
);
const resolutionContexts = new Map([
  [root, 'root'],
  [path.join(root, 'node_modules', 'trac-peer'), 'trac-peer'],
  [path.join(root, 'node_modules', 'trac-msb'), 'trac-msb'],
]);
if (declaresWallet(manifest)) resolutionContexts.set(root, 'root wallet declarer');
for (const installed of installedPackages) {
  if (installed.manifest.name !== 'trac-wallet' && declaresWallet(installed.manifest)) {
    resolutionContexts.set(
      installed.root,
      `${installed.manifest.name ?? installed.root} wallet declarer`,
    );
  }
}

const resolveInstalledWalletRoot = (contextRoot) => {
  let current = contextRoot;
  while (current.startsWith(root)) {
    const candidate = path.join(current, 'node_modules', 'trac-wallet');
    if (fs.existsSync(path.join(candidate, 'package.json'))) {
      return fs.realpathSync(candidate);
    }
    const parent = path.dirname(current);
    if (parent === current) break;
    current = parent;
  }
  fail(`cannot resolve trac-wallet from ${contextRoot}`);
};

for (const [contextRoot, label] of resolutionContexts) {
  const resolvedPath = resolveInstalledWalletRoot(contextRoot);
  let expectedWalletRoot = walletRoot;
  if (contextRoot === path.join(root, 'node_modules', 'trac-msb')) {
    expectedWalletRoot = path.join(root, 'node_modules', 'trac-msb', 'node_modules', 'trac-wallet');
  } else if (contextRoot === path.join(root, 'node_modules', 'trac-peer')) {
    expectedWalletRoot = path.join(root, 'node_modules', 'trac-peer', 'node_modules', 'trac-wallet');
  }
  if (resolvedPath !== expectedWalletRoot) {
    fail(`${label} resolves trac-wallet outside its pinned package: ${resolvedPath}`);
  }
}

verifyLocalDependencies(root);

console.log(
  `verified Intercom dependency topology: ${installedPackages.length} packages, ` +
  `${resolutionContexts.size} wallet resolution contexts`,
);

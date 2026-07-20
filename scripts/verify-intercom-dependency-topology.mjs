#!/usr/bin/env node

import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
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
if (manifest.overrides?.['trac-wallet'] !== '1.0.1') {
  fail('root overrides must pin trac-wallet to 1.0.1');
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
if (walletLockPaths.length !== 1 || walletLockPaths[0] !== 'node_modules/trac-wallet') {
  fail(`root lock must contain exactly one top-level trac-wallet, found: ${walletLockPaths.join(', ')}`);
}
if (lock.packages['node_modules/trac-wallet']?.version !== '1.0.1') {
  fail('root lock must install trac-wallet 1.0.1');
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
if (walletDirectories.length !== 1 || walletDirectories[0] !== walletRoot) {
  fail(`runtime must contain exactly one top-level trac-wallet, found: ${walletDirectories.join(', ')}`);
}
const walletManifest = readJson(path.join(walletRoot, 'package.json'));
if (walletManifest.name !== 'trac-wallet' || walletManifest.version !== '1.0.1') {
  fail('installed top-level wallet must be trac-wallet 1.0.1');
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
  if (declaresWallet(installed.manifest)) {
    resolutionContexts.set(
      installed.root,
      `${installed.manifest.name ?? installed.root} wallet declarer`,
    );
  }
}

for (const [contextRoot, label] of resolutionContexts) {
  const resolved = createRequire(path.join(contextRoot, 'package.json')).resolve('trac-wallet');
  const resolvedPath = fs.realpathSync(resolved);
  if (resolvedPath !== walletRoot && !resolvedPath.startsWith(`${walletRoot}${path.sep}`)) {
    fail(`${label} resolves trac-wallet outside the root 1.0.1 package: ${resolvedPath}`);
  }
}

verifyLocalDependencies(root);

console.log(
  `verified Intercom dependency topology: ${installedPackages.length} packages, ` +
  `${resolutionContexts.size} wallet resolution contexts`,
);

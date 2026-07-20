#!/usr/bin/env node

import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

export const LOCAL_PACKAGE_RUNTIME_PATHS = Object.freeze({
  'trac-msb': Object.freeze([
    'package.json',
    'msb.mjs',
    'migration',
    'proto',
    'rpc',
    'src',
    'whitelist',
  ]),
  'trac-peer': Object.freeze([
    'package.json',
    'rpc',
    'scripts/run-peer.mjs',
    'src',
  ]),
});

const fail = (message) => {
  throw new Error(`Intercom local dependency materialization: ${message}`);
};

const inspect = (target, label) => {
  let stat;
  try {
    stat = fs.lstatSync(target);
  } catch (error) {
    fail(`${label} cannot be inspected: ${error?.message ?? error}`);
  }
  if (stat.isSymbolicLink()) fail(`${label} must not be a symbolic link`);
  return stat;
};

const rootsFor = (intercomRoot, packageName) => {
  const root = fs.realpathSync(path.resolve(intercomRoot));
  const source = path.join(root, 'trac', packageName === 'trac-msb' ? 'msb' : 'trac-peer');
  const destination = path.join(root, 'node_modules', packageName);
  const sourceStat = inspect(source, `${packageName} pinned source`);
  const destinationStat = inspect(destination, `${packageName} physical package`);
  if (!sourceStat.isDirectory() || !destinationStat.isDirectory()) {
    fail(`${packageName} source and physical package must be real directories`);
  }
  if (fs.realpathSync(destination) !== destination) {
    fail(`${packageName} physical package resolves outside its root location`);
  }
  return { source, destination };
};

const visitSource = (source, relativePath, callback) => {
  const absolute = path.join(source, relativePath);
  const stat = inspect(absolute, `pinned runtime path ${relativePath}`);
  callback({ absolute, relativePath, stat });
  if (!stat.isDirectory()) return;
  const entries = fs.readdirSync(absolute, { withFileTypes: true })
    .sort((left, right) => left.name.localeCompare(right.name));
  for (const entry of entries) {
    visitSource(source, path.posix.join(relativePath, entry.name), callback);
  }
};

const ensureDestinationDirectory = (directory, packageRoot) => {
  if (directory === packageRoot) return;
  ensureDestinationDirectory(path.dirname(directory), packageRoot);
  if (!fs.existsSync(directory)) {
    fs.mkdirSync(directory);
    return;
  }
  const stat = inspect(directory, `runtime destination ${directory}`);
  if (!stat.isDirectory()) fail(`runtime destination ${directory} must be a directory`);
};

export const materializeLocalDependencies = (intercomRoot) => {
  for (const [packageName, runtimePaths] of Object.entries(LOCAL_PACKAGE_RUNTIME_PATHS)) {
    const { source, destination } = rootsFor(intercomRoot, packageName);
    for (const runtimePath of runtimePaths) {
      visitSource(source, runtimePath, ({ absolute, relativePath, stat }) => {
        const target = path.join(destination, relativePath);
        if (stat.isDirectory()) {
          ensureDestinationDirectory(target, destination);
          return;
        }
        if (!stat.isFile()) fail(`pinned runtime path ${relativePath} must be a regular file`);
        ensureDestinationDirectory(path.dirname(target), destination);
        if (fs.existsSync(target) && inspect(target, `runtime destination ${target}`).isDirectory()) {
          fail(`runtime destination ${target} unexpectedly exists as a directory`);
        }
        fs.copyFileSync(absolute, target);
      });
    }
  }
};

export const verifyLocalDependencies = (intercomRoot) => {
  for (const [packageName, runtimePaths] of Object.entries(LOCAL_PACKAGE_RUNTIME_PATHS)) {
    const { source, destination } = rootsFor(intercomRoot, packageName);
    for (const runtimePath of runtimePaths) {
      visitSource(source, runtimePath, ({ absolute, relativePath, stat }) => {
        const target = path.join(destination, relativePath);
        const targetStat = inspect(target, `materialized runtime path ${packageName}/${relativePath}`);
        if (stat.isDirectory()) {
          if (!targetStat.isDirectory()) {
            fail(`materialized runtime path ${packageName}/${relativePath} must be a directory`);
          }
          return;
        }
        if (!stat.isFile() || !targetStat.isFile()) {
          fail(`materialized runtime path ${packageName}/${relativePath} must be a regular file`);
        }
        if (!fs.readFileSync(absolute).equals(fs.readFileSync(target))) {
          fail(`materialized runtime file differs from pinned source: ${packageName}/${relativePath}`);
        }
      });
    }
  }
};

const invokedPath = process.argv[1]
  ? pathToFileURL(fs.realpathSync(path.resolve(process.argv[1]))).href
  : null;
if (invokedPath === import.meta.url) {
  const root = process.argv[2];
  if (!root) fail('usage: materialize-local-dependencies.mjs <intercom-root>');
  materializeLocalDependencies(root);
  verifyLocalDependencies(root);
  console.log('materialized canonical Intercom local dependencies');
}

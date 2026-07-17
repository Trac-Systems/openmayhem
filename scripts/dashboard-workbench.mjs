#!/usr/bin/env node

import { spawn } from 'node:child_process';
import fs from 'node:fs';
import fsp from 'node:fs/promises';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const BINARY_NAME = process.platform === 'win32'
  ? 'mayhem-dashboard-workbench.exe'
  : 'mayhem-dashboard-workbench';
const BUILD_BINARY = path.join(ROOT, 'target', 'debug', BINARY_NAME);
const RUNTIME_DIR = path.join(ROOT, 'target', 'dashboard-workbench', 'runtime');
const WATCH_PATHS = [
  path.join(ROOT, 'crates', 'mayhem-gateway', 'src'),
  path.join(ROOT, 'crates', 'mayhem-gateway', 'Cargo.toml'),
  path.join(ROOT, 'crates', 'mayhem-gateway', 'assets'),
  path.join(ROOT, 'catalog'),
  path.join(ROOT, 'Cargo.lock'),
];

let buildChild = null;
let serverChild = null;
let buildRunning = false;
let rebuildQueued = false;
let restartNumber = 0;
let debounceTimer = null;
let shuttingDown = false;
const watchers = [];
const runtimeCopies = new Set();

function usage() {
  console.log(`Usage:
  node scripts/dashboard-workbench.mjs [--bind 127.0.0.1:11436]

Builds the isolated dashboard workbench, watches its Rust and catalog sources,
and restarts it after successful builds. Open http://127.0.0.1:11436/ to choose
a dashboard and fixture scenario. The browser reloads after each restart.`);
}

function serverArgs() {
  const args = process.argv.slice(2);
  if (args.includes('--help') || args.includes('-h')) {
    usage();
    process.exit(0);
  }
  return args;
}

const SERVER_ARGS = serverArgs();

function queueBuild(changedPath = 'startup') {
  if (shuttingDown) return;
  if (buildRunning) {
    rebuildQueued = true;
    return;
  }
  clearTimeout(debounceTimer);
  debounceTimer = setTimeout(() => {
    void buildAndRestart(changedPath);
  }, changedPath === 'startup' ? 0 : 180);
}

async function stopServer() {
  const child = serverChild;
  if (!child) return;
  serverChild = null;
  child.kill();
  await new Promise((resolve) => {
    child.once('exit', resolve);
    setTimeout(resolve, 1500).unref();
  });
}

async function startServer() {
  await fsp.mkdir(RUNTIME_DIR, { recursive: true });
  restartNumber += 1;
  const runtimeBinary = path.join(
    RUNTIME_DIR,
    `${path.parse(BINARY_NAME).name}-${process.pid}-${restartNumber}${path.extname(BINARY_NAME)}`,
  );
  await fsp.copyFile(BUILD_BINARY, runtimeBinary);
  if (process.platform !== 'win32') await fsp.chmod(runtimeBinary, 0o755);
  runtimeCopies.add(runtimeBinary);

  await stopServer();
  const child = spawn(runtimeBinary, SERVER_ARGS, {
    cwd: ROOT,
    env: process.env,
    stdio: 'inherit',
    windowsHide: true,
  });
  serverChild = child;
  child.once('exit', (code, signal) => {
    if (!shuttingDown && serverChild === child) {
      console.error(`[workbench] server exited (${signal ?? code})`);
      serverChild = null;
    }
  });
}

async function buildAndRestart(reason) {
  if (buildRunning || shuttingDown) return;
  buildRunning = true;
  rebuildQueued = false;
  console.log(`[workbench] building after ${reason}`);

  buildChild = spawn('cargo', [
    'build',
    '-p', 'mayhem-gateway',
    '--features', 'dashboard-workbench',
    '--bin', 'mayhem-dashboard-workbench',
  ], {
    cwd: ROOT,
    env: process.env,
    stdio: 'inherit',
    windowsHide: true,
  });

  const code = await new Promise((resolve) => buildChild.once('exit', resolve));
  buildChild = null;
  buildRunning = false;

  if (code === 0 && !shuttingDown) {
    try {
      await startServer();
    } catch (error) {
      console.error(`[workbench] could not start: ${error.message}`);
    }
  } else if (!shuttingDown) {
    console.error('[workbench] build failed; fix the error and save to retry');
  }

  if (rebuildQueued && !shuttingDown) queueBuild('queued changes');
}

function watchSources() {
  for (const watchPath of WATCH_PATHS) {
    if (!fs.existsSync(watchPath)) continue;
    const recursive = fs.statSync(watchPath).isDirectory();
    const watcher = fs.watch(watchPath, { recursive }, (_event, filename) => {
      queueBuild(filename ? path.join(path.basename(watchPath), filename) : watchPath);
    });
    watcher.on('error', (error) => console.error(`[workbench] watch error: ${error.message}`));
    watchers.push(watcher);
  }
}

async function cleanup() {
  if (shuttingDown) return;
  shuttingDown = true;
  clearTimeout(debounceTimer);
  for (const watcher of watchers) watcher.close();
  if (buildChild) buildChild.kill();
  await stopServer();
  await Promise.allSettled([...runtimeCopies].map((file) => fsp.rm(file, { force: true })));
}

process.once('SIGINT', () => void cleanup().then(() => process.exit(130)));
process.once('SIGTERM', () => void cleanup().then(() => process.exit(143)));
process.once('uncaughtException', (error) => {
  console.error(error);
  void cleanup().then(() => process.exit(1));
});

watchSources();
queueBuild('startup');

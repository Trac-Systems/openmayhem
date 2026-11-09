#!/usr/bin/env node
// Apply the exact upstream patch to the dependency copies used by this install.
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
import { pathToFileURL } from 'node:url';

const root = fs.realpathSync(path.resolve(process.argv[2] || '.'));
const check = process.argv.includes('--check');
for (const name of ['trac-msb', 'trac-peer']) {
  const packageDir = path.join(root, 'node_modules', name);
  const manifestPath = path.join(packageDir, 'package.json');
  const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
  const require = createRequire(manifestPath);
  const hypercoreDir = fs.realpathSync(path.dirname(require.resolve('hypercore/package.json')));
  const { run } = await import(pathToFileURL(path.join(packageDir, 'scripts/hypercore-stallfix/apply.mjs')).href);
  const result = await run({ packageDir, check });
  const primary = result.results.find(copy => copy.dir === hypercoreDir);
  if (!result.ok || !primary || !primary.supported || primary.state !== 'patched' ||
      primary.version !== manifest.dependencies.hypercore) {
    throw new Error(`${name}: pinned Hypercore replication repair is missing or incompatible`);
  }
}

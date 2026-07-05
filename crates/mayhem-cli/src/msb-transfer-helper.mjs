#!/usr/bin/env node
import path from 'path';
import { fileURLToPath, pathToFileURL } from 'url';

const helperPath = fileURLToPath(import.meta.url);
const repoRoot = path.resolve(path.dirname(helperPath), '../../..');
const intercomRoot = path.join(repoRoot, 'intercom');
const { runTransferHelper } = await import(
  pathToFileURL(path.join(intercomRoot, 'trac/msb/src/transferHelper.js')).href
);

try {
  const result = await runTransferHelper(process.argv.slice(2));
  console.log(JSON.stringify(result, null, 2));
} catch (error) {
  console.error(error?.message ?? String(error));
  process.exit(1);
}

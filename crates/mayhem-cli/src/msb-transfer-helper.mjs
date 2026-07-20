#!/usr/bin/env node
import path from 'path';
import { fileURLToPath, pathToFileURL } from 'url';

const helperPath = fileURLToPath(import.meta.url);
const repoRoot = path.resolve(path.dirname(helperPath), '../../..');
const intercomRoot = path.join(repoRoot, 'intercom');
const { runRootMsbTransferHelper } = await import(
  pathToFileURL(path.join(intercomRoot, 'src/msb-transfer-helper.js')).href
);

try {
  const [command, ...args] = process.argv.slice(2);
  const result = await runRootMsbTransferHelper(command, args);
  console.log(JSON.stringify(result, null, 2));
} catch (error) {
  console.error(error?.message ?? String(error));
  process.exit(1);
}

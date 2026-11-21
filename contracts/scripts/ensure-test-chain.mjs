// Installs the test-only local EVM (test-chain/) when it is missing.
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const dir = path.join(path.dirname(fileURLToPath(import.meta.url)), '..', 'test-chain');
if (!fs.existsSync(path.join(dir, 'node_modules', 'ganache', 'package.json'))) {
  execFileSync('npm', ['ci', '--prefix', dir, '--no-audit', '--no-fund'], { stdio: 'inherit' });
}

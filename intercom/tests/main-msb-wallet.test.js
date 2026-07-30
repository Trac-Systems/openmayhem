import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const mainPath = path.resolve(__dirname, '../src/main.js');

test('main peer opens MainSettlementBus with the configured wallet', () => {
  const source = fs.readFileSync(mainPath, 'utf8');

  assert.match(source, /import\s+PeerWallet\s+from\s+['"]trac-wallet['"]/);
  assert.match(source, /const\s+loadPeerWallet\s*=\s*async\s*\(config\)\s*=>/);
  assert.match(source, /await\s+wallet\.importFromFile\s*\(\s*config\.keyPairPath\s*,\s*b4a\.alloc\(0\)\s*\)/);
  assert.match(source, /const\s+msbWallet\s*=\s*await\s+loadPeerWallet\s*\(\s*msbConfig\s*\)/);
  assert.match(source, /new\s+MainSettlementBus\s*\(\s*msbConfig\s*,\s*msbWallet\s*\)/);
});

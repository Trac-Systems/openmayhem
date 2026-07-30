import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

import {
  Peer,
  ENV as PEER_ENV,
  createConfig as createPeerConfig,
} from 'trac-peer/src/index.js';
import {
  ENV as MSB_ENV,
  createConfig as createMsbConfig,
} from 'trac-msb/src/config/env.js';
import 'trac-msb/src/index.js';

function tempStore(prefix) {
  return `${fs.mkdtempSync(path.join(os.tmpdir(), prefix))}/`;
}

test('trac-peer bounds the shared Hyperbee/Corestore cache', async () => {
  const defaultConfig = createPeerConfig(PEER_ENV.DEVELOPMENT, {
    storesDirectory: tempStore('mayhem-peer-cache-default-'),
    storeName: 'peer',
  });
  const defaultPeer = new Peer({
    config: defaultConfig,
    msb: null,
    wallet: {},
    protocol: class {},
    contract: class {},
  });

  assert.equal(defaultConfig.hyperbeeCacheMaxEntries, 65_536);
  assert.equal(defaultPeer.store.globalCache.maxSize, 65_536);
  await defaultPeer.store.close();

  const overrideConfig = createPeerConfig(PEER_ENV.DEVELOPMENT, {
    storesDirectory: tempStore('mayhem-peer-cache-override-'),
    storeName: 'peer',
    hyperbeeCacheMaxEntries: 7,
  });
  const overridePeer = new Peer({
    config: overrideConfig,
    msb: null,
    wallet: {},
    protocol: class {},
    contract: class {},
  });

  assert.equal(overrideConfig.hyperbeeCacheMaxEntries, 7);
  assert.equal(overridePeer.store.globalCache.maxSize, 7);
  await overridePeer.store.close();
});

test('trac-msb bounds the shared Hyperbee/Corestore cache', () => {
  const defaultConfig = createMsbConfig(MSB_ENV.DEVELOPMENT, {
    storesDirectory: tempStore('mayhem-msb-cache-default-'),
    storeName: 'msb',
  });

  assert.equal(defaultConfig.hyperbeeCacheMaxEntries, 65_536);

  const overrideConfig = createMsbConfig(MSB_ENV.DEVELOPMENT, {
    storesDirectory: tempStore('mayhem-msb-cache-override-'),
    storeName: 'msb',
    hyperbeeCacheMaxEntries: 9,
  });

  assert.equal(overrideConfig.hyperbeeCacheMaxEntries, 9);
});

test('Hyperbee/Corestore cache bounds reject unsafe values', () => {
  assert.throws(
    () => createPeerConfig(PEER_ENV.DEVELOPMENT, { hyperbeeCacheMaxEntries: 0 }),
    /hyperbeeCacheMaxEntries must be a positive safe integer/
  );
  assert.throws(
    () => createMsbConfig(MSB_ENV.DEVELOPMENT, { hyperbeeCacheMaxEntries: 0 }),
    /hyperbeeCacheMaxEntries must be a positive safe integer/
  );
});

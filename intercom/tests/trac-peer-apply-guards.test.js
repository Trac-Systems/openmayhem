import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

import b4a from 'b4a';
import { Peer, createConfig, ENV } from 'trac-peer/src/index.js';
import Wallet from 'trac-peer/src/wallet.js';

class TestProtocol {
  async extendApi() {}
  getError(value) {
    return value ?? null;
  }
  txMaxBytes() {
    return 1024 * 1024;
  }
  msgMaxBytes() {
    return 1024 * 1024;
  }
  featMaxBytes() {
    return 1024 * 1024;
  }
}

class TestContract {
  async execute() {
    throw new Error('contract should not execute for rejected MSB references');
  }
}

const rmrf = async (target) => {
  await fs.promises.rm(target, { recursive: true, force: true });
};

const tempStore = () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-trac-peer-apply-'));
  return { root, storesDirectory: `${root}${path.sep}` };
};

const hex32 = (fill) => b4a.alloc(32).fill(fill).toString('hex');

async function prepareWallet(storesDirectory, storeName) {
  const wallet = new Wallet();
  await wallet.generateKeyPair();
  const keypairPath = path.join(storesDirectory, storeName, 'db', 'keypair.json');
  await fs.promises.mkdir(path.dirname(keypairPath), { recursive: true });
  await wallet.exportToFile(keypairPath, b4a.alloc(0));
  return wallet;
}

function makeMsbStub({ signedLength }) {
  const core = {
    signedLength,
    once() {
      throw new Error('apply should not wait for a far-future msbsl');
    },
  };
  return {
    async ready() {},
    config: {
      bootstrap: b4a.alloc(32).fill(7),
      addressPrefix: 'trac',
      networkId: 918,
    },
    network: {},
    state: {
      base: {
        view: {
          core,
          checkout() {
            throw new Error('apply should not checkout MSB for a far-future msbsl');
          },
        },
      },
      getSignedLength() {
        return core.signedLength;
      },
    },
  };
}

test('trac-peer tx apply rejects far-future MSB references before waiting', async () => {
  const { root, storesDirectory } = tempStore();
  const storeName = 'peer-relative-stall-guard';
  const wallet = await prepareWallet(storesDirectory, storeName);
  const peer = new Peer({
    config: createConfig(ENV.DEVELOPMENT, {
      storesDirectory,
      storeName,
      maxMsbSignedLength: 1_000_000_000,
      maxMsbSignedLengthFutureDelta: 10,
    }),
    msb: makeMsbStub({ signedLength: 100 }),
    wallet,
    protocol: TestProtocol,
    contract: TestContract,
  });

  try {
    await peer.ready();
    await Promise.race([
      peer.base.append({
        type: 'tx',
        key: hex32(1),
        value: {
          dispatch: { type: 'ping', value: { msg: 'hi' } },
          msbsl: 111,
          ipk: hex32(2),
          wp: hex32(3),
        },
      }),
      new Promise((_, reject) =>
        setTimeout(() => reject(new Error('append timed out')), 2_000)
      ),
    ]);
    assert.equal(await peer.bee.get('txl'), null);
  } finally {
    try {
      await peer.close();
    } catch (_error) {}
    try {
      await peer.store.close();
    } catch (_error) {}
    await rmrf(root);
  }
});

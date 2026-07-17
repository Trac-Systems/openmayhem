import assert from 'node:assert/strict';
import test from 'node:test';
import b4a from 'b4a';
import {
  attachAutobaseWakeup,
  hydrateAdminWriterViews,
  joinCanonicalPeers,
} from '../src/admin-view-hydration.js';

const fakeCore = (length, contiguousLength) => ({
  length,
  contiguousLength,
});

const fakePeer = ({ writable = true, admin = 'admin', wallet = 'admin' } = {}) => {
  const publicCore = fakeCore(12, 5);
  let scans = 0;
  return {
    wallet: { publicKey: wallet },
    base: {
      writable,
      view: {
        core: publicCore,
        get: async () => ({ value: admin }),
        createReadStream: async function* () {
          scans += 1;
          yield { key: 'admin' };
          yield { key: 'payments/current' };
        },
      },
    },
    scanCount: () => scans,
  };
};

test('admin writer traverses every current-state branch before serving writes', async () => {
  const peer = fakePeer();
  const reports = [];
  const result = await hydrateAdminWriterViews(peer, {
    report: (line) => reports.push(line),
  });

  assert.equal(result.required, true);
  assert.equal(result.entries, 2);
  assert.equal(peer.scanCount(), 1);
  assert.deepEqual(reports, [
    'Admin writer hydrating canonical state: view 5/12',
    'Admin writer canonical state ready: 2 entries',
  ]);
});

test('read-only and non-admin writers never hydrate canonical views', async () => {
  for (const peer of [
    fakePeer({ writable: false }),
    fakePeer({ admin: 'other' }),
  ]) {
    const result = await hydrateAdminWriterViews(peer);
    assert.equal(result.required, false);
    assert.equal(peer.scanCount(), 0);
  }
});

test('canonical peers are joined explicitly without joining self', () => {
  const self = '11'.repeat(32);
  const remote = '22'.repeat(32);
  const joined = [];
  const count = joinCanonicalPeers({
    wallet: { publicKey: self },
    swarm: { joinPeer: (key) => joined.push(b4a.toString(key, 'hex')) },
  }, [self, remote]);
  assert.equal(count, 1);
  assert.deepEqual(joined, [remote]);
});

test('canonical peer joins reject malformed keys', () => {
  assert.throws(
    () => joinCanonicalPeers({ swarm: { joinPeer() {} } }, ['not-a-key']),
    /32-byte public key/
  );
});

test('Autobase wakeup attaches to existing and future replication connections', () => {
  const existing = {};
  const future = {};
  const streams = new Set();
  let onConnection = null;
  const peer = {
    base: {
      wakeupProtocol: {
        hasStream: (connection) => streams.has(connection),
        addStream: (connection) => streams.add(connection),
      },
    },
    swarm: {
      connections: new Set([existing]),
      on(event, listener) {
        if (event === 'connection') onConnection = listener;
      },
    },
  };

  assert.equal(attachAutobaseWakeup(peer), 1);
  assert.equal(streams.has(existing), true);
  onConnection(future);
  onConnection(future);
  assert.equal(streams.has(future), true);
  assert.equal(streams.size, 2);
});

test('Autobase wakeup does not attach a stream already owned by the base', () => {
  const existing = {};
  const peer = {
    base: {
      wakeupProtocol: {
        hasStream: () => true,
        addStream() {
          assert.fail('already-attached wakeup stream must not be added twice');
        },
      },
    },
    swarm: {
      connections: new Set([existing]),
      on() {},
    },
  };

  assert.equal(attachAutobaseWakeup(peer), 0);
});

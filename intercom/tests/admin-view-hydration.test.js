import assert from 'node:assert/strict';
import test from 'node:test';
import b4a from 'b4a';
import {
  canonicalAdminViewCores,
  hydrateAdminWriterViews,
} from '../src/admin-view-hydration.js';

const fakeCore = (keyByte, length, contiguousLength) => ({
  key: b4a.alloc(32, keyByte),
  opened: true,
  length,
  contiguousLength,
  downloads: [],
  download(range) {
    this.downloads.push(range);
    return {
      done: async () => {
        this.contiguousLength = range.end;
      },
      destroy() {},
    };
  },
});

const fakePeer = ({ writable = true, admin = 'admin', wallet = 'admin' } = {}) => {
  const publicCore = fakeCore(1, 12, 5);
  const systemCore = fakeCore(2, 4, 4);
  const viewCore = fakeCore(1, 12, 5);
  return {
    wallet: { publicKey: wallet },
    base: {
      writable,
      view: {
        core: publicCore,
        get: async () => ({ value: admin }),
      },
      _applyState: {
        system: { core: systemCore },
        views: [{ name: 'view', core: viewCore }],
      },
    },
    cores: { publicCore, systemCore, viewCore },
  };
};

test('admin writer hydrates each canonical core before serving writes', async () => {
  const peer = fakePeer();
  const reports = [];
  const result = await hydrateAdminWriterViews(peer, {
    report: (line) => reports.push(line),
  });

  assert.equal(result.required, true);
  assert.equal(result.views.length, 2);
  assert.deepEqual(peer.cores.publicCore.downloads, [{ start: 0, end: 12, linear: true }]);
  assert.equal(peer.cores.systemCore.downloads.length, 0);
  assert.equal(peer.cores.viewCore.downloads.length, 0);
  assert.deepEqual(reports, ['Admin writer hydrating canonical public view: 5/12']);
});

test('read-only and non-admin writers never hydrate canonical views', async () => {
  for (const peer of [
    fakePeer({ writable: false }),
    fakePeer({ admin: 'other' }),
  ]) {
    const result = await hydrateAdminWriterViews(peer);
    assert.equal(result.required, false);
    assert.equal(peer.cores.publicCore.downloads.length, 0);
  }
});

test('canonical core discovery deduplicates sessions sharing one key', () => {
  const peer = fakePeer();
  const cores = canonicalAdminViewCores(peer);
  assert.deepEqual(cores.map(({ name }) => name), ['public', 'system']);
});

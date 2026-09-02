import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { registerHooks } from 'node:module';
import os from 'node:os';
import path from 'node:path';
import { Readable } from 'node:stream';

import { dispatchContainedClientRequest } from '../features/sc-bridge/containment.js';
import {
  addBoundedSubscriptions,
  messageByteLength,
  sidechannelSubscriptionMatches,
  writeBoundedClientPayload,
} from '../features/sc-bridge/bounded-client.js';
import {
  safeHeapSnapshotLabel,
  writeBareHeapSnapshot,
} from '../features/sc-bridge/heap-snapshot.js';
import { closeBridgeSocket } from '../features/sc-bridge/socket.js';
import {
  canOwnSession,
  closeOwnedSessions,
  disownSession,
  ownSession,
  sessionFrameRecipients,
  sessionSubscriptionMatches,
  transferSessionOwnership,
} from '../features/sc-bridge/session-ownership.js';
import { resolveScBridgeToken } from '../features/sc-bridge/token.js';

const loadScBridgeForNodeTest = async () => {
  const hooks = registerHooks({
    resolve(specifier, context, nextResolve) {
      if (specifier === 'bare-ws') {
        return {
          url: 'data:text/javascript,export default { Server: class {} };',
          shortCircuit: true,
        };
      }
      return nextResolve(specifier, context);
    },
  });
  try {
    return (await import('../features/sc-bridge/index.js?node-containment-test')).default;
  } finally {
    hooks.deregister();
  }
};

test('SC-Bridge reads a private token file when Pear does not expose child environment', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-sc-bridge-token-'));
  const tokenFile = path.join(root, 'token');
  fs.writeFileSync(tokenFile, 'file-secret\n', { mode: 0o600 });
  try {
    assert.equal(
      resolveScBridgeToken(
        { 'sc-bridge-token-file': tokenFile },
        { SC_BRIDGE_TOKEN: 'environment-secret' }
      ),
      'file-secret'
    );
    assert.throws(
      () =>
        resolveScBridgeToken({
          'sc-bridge-token': 'argument-secret',
          'sc-bridge-token-file': tokenFile,
        }),
      /either --sc-bridge-token or --sc-bridge-token-file/
    );
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test('failed async client request is contained and the bridge serves the next request', async () => {
  const events = [];
  dispatchContainedClientRequest(
    async () => {
      throw new Error('injected session_open failure');
    },
    (error) => events.push(`error:${error.message}`)
  );
  await new Promise((resolve) => setTimeout(resolve, 0));
  dispatchContainedClientRequest(
    () => events.push('pong'),
    (error) => events.push(`error:${error.message}`)
  );

  assert.deepEqual(events, ['error:injected session_open failure', 'pong']);
});

test('SC-Bridge closes rejected sockets without surfacing an uncaught error object', () => {
  const calls = [];
  const socket = {
    end() {
      calls.push(['end']);
    },
    destroy(...args) {
      calls.push(['destroy', ...args]);
    },
  };

  closeBridgeSocket(socket);

  assert.deepEqual(calls, [['end'], ['destroy']]);
});

test('SC-Bridge heap snapshot helper writes a private sanitized artifact', async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-sc-bridge-heap-'));
  let connected = false;
  let destroyed = false;
  class FakeSession {
    connect() {
      connected = true;
    }

    destroy() {
      destroyed = true;
    }
  }
  class FakeHeapSnapshot extends Readable {
    _read() {
      this.push('heap');
      this.push(null);
    }
  }

  try {
    assert.equal(safeHeapSnapshotLabel('../bad label/?token'), 'bad-label-token');
    const result = await writeBareHeapSnapshot(root, '../bad label/?token', {
      fs,
      path,
      inspector: {
        Session: FakeSession,
        HeapSnapshot: FakeHeapSnapshot,
      },
    });

    assert.equal(connected, true);
    assert.equal(destroyed, true);
    assert.equal(path.dirname(result.path), root);
    assert.equal(path.basename(result.path).includes('bad-label-token'), true);
    assert.equal(fs.readFileSync(result.path, 'utf8'), 'heap');
    assert.equal(result.bytes, 4);
    assert.equal(fs.statSync(result.path).mode & 0o777, 0o600);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test('SC-Bridge heap snapshot helper remains disabled until a directory is configured', async () => {
  await assert.rejects(
    writeBareHeapSnapshot('', 'ignored', {
      fs,
      path,
      inspector: {},
    }),
    /Heap snapshots are disabled/
  );
});

const queuedClient = (id = 1) => {
  const callbacks = [];
  const writes = [];
  let destroyed = false;
  const client = {
    id,
    ready: true,
    authed: true,
    closed: false,
    outboundQueue: [],
    outboundBytes: 0,
    writing: false,
    authTimer: null,
    socket: {
      write(data, callback) {
        writes.push(data);
        callbacks.push(callback);
        return false;
      },
      destroy() {
        destroyed = true;
      },
    },
  };
  return { client, callbacks, writes, destroyed: () => destroyed };
};

test('SC-Bridge applies bounded backpressure without combining valid stream frames', () => {
  const limits = {
    maxMessageBytes: 1024,
    maxOutboundMessages: 4,
    maxOutboundBytes: 4096,
  };
  const { client, callbacks, writes, destroyed } = queuedClient();
  const drop = (reason) => {
    client.closed = true;
    client.socket.destroy(new Error(reason));
  };

  assert.equal(writeBoundedClientPayload(client, { n: 1, d: 'x'.repeat(128) }, limits, drop), true);
  assert.equal(writeBoundedClientPayload(client, { n: 2, d: 'y'.repeat(128) }, limits, drop), true);
  assert.equal(writes.length, 1);
  callbacks.shift()(null);
  assert.equal(writes.length, 2);
  callbacks.shift()(null);
  assert.equal(client.outboundQueue.length, 0);
  assert.equal(client.outboundBytes, 0);
  assert.equal(destroyed(), false);
});

test('SC-Bridge disconnects one stalled consumer when its bounded queue fills', () => {
  const limits = {
    maxMessageBytes: 1024,
    maxOutboundMessages: 2,
    maxOutboundBytes: 4096,
  };
  const { client, destroyed } = queuedClient();
  const drop = (reason) => {
    client.closed = true;
    client.socket.destroy(new Error(reason));
  };

  assert.equal(writeBoundedClientPayload(client, { n: 1 }, limits, drop), true);
  assert.equal(writeBoundedClientPayload(client, { n: 2 }, limits, drop), true);
  assert.equal(writeBoundedClientPayload(client, { n: 3 }, limits, drop), false);
  assert.equal(client.closed, true);
  assert.equal(destroyed(), true);
});

test('SC-Bridge rejects oversized local websocket messages and caps subscriptions', () => {
  assert.equal(addBoundedSubscriptions(new Set(), ['one', 'two', 'three'], 2), false);
  assert.equal(messageByteLength('x'.repeat(65)) > 64, true);
});

test('SC-Bridge explicit empty subscription receives no unrelated sidechannel traffic', () => {
  assert.equal(sidechannelSubscriptionMatches(null, 'mx/room/one'), true);
  assert.equal(sidechannelSubscriptionMatches(new Set(), 'mx/room/one'), false);
  assert.equal(
    sidechannelSubscriptionMatches(new Set(['mx/room/two']), 'mx/room/two'),
    true,
  );
});

test('SC-Bridge send-only subscription stays empty beyond the client queue bound', () => {
  const channels = new Set();
  let emitted = 0;

  for (let sequence = 0; sequence <= 4096; sequence += 1) {
    if (sidechannelSubscriptionMatches(channels, `mx/room/${sequence}`)) emitted += 1;
  }

  assert.equal(emitted, 0);
});

test('SC-Bridge bounds and reclaims every direct session owned by a disconnected client', () => {
  const sessions = new Map();
  const remote = 'ab'.repeat(32);
  const maxSessions = 128;
  for (let index = 0; index < maxSessions; index += 1) {
    const sessionId = index.toString(16).padStart(64, '0');
    assert.equal(canOwnSession(sessions, remote, sessionId, maxSessions), true);
    ownSession(sessions, remote, sessionId);
  }
  assert.equal(sessions.size, maxSessions);
  assert.equal(canOwnSession(sessions, remote, 'ff'.repeat(32), maxSessions), false);

  const first = '0'.repeat(64);
  assert.equal(canOwnSession(sessions, remote, first, maxSessions), true);
  disownSession(sessions, remote, first);
  assert.equal(canOwnSession(sessions, remote, 'ff'.repeat(32), maxSessions), true);

  const closed = [];
  closeOwnedSessions(sessions, (closedRemote, sessionId) => {
    closed.push(`${closedRemote}:${sessionId}`);
  });
  assert.equal(closed.length, maxSessions - 1);
  assert.equal(sessions.size, 0);
});

test('SC-Bridge transfers one live session without closing or duplicating ownership', () => {
  const remote = 'ab'.repeat(32);
  const sessionId = 'cd'.repeat(32);
  const original = { directSessions: new Map() };
  const replacement = { directSessions: new Map() };
  ownSession(original.directSessions, remote, sessionId);

  transferSessionOwnership([original, replacement], replacement, remote, sessionId);

  assert.equal(original.directSessions.size, 0);
  assert.equal(replacement.directSessions.size, 1);
  assert.equal(
    replacement.directSessions.has(`${remote}:${sessionId}`),
    true,
  );
});

test('SC-Bridge isolates concurrent paid sessions when one cancels and disconnects', async () => {
  const ScBridge = await loadScBridgeForNodeTest();
  const remote = 'ab'.repeat(32);
  const sessionA = '01'.repeat(32);
  const sessionB = '02'.repeat(32);
  const opened = [];
  const sent = [];
  const closed = [];
  const sessions = new Map();
  const directSession = {
    sessions,
    async open(openRemote, sessionId) {
      const session = { remote: openRemote, session_id: sessionId, opened: true };
      sessions.set(`${openRemote}:${sessionId}`, session);
      opened.push(sessionId);
      return session;
    },
    async send(sendRemote, sessionId, frame) {
      sent.push({ remote: sendRemote, sessionId, frame });
      return { remote: sendRemote, session_id: sessionId, sent: true };
    },
    close(closeRemote, sessionId) {
      sessions.delete(`${closeRemote}:${sessionId}`);
      closed.push(sessionId);
      return { remote: closeRemote, session_id: sessionId, closed: true };
    },
  };
  const makeClient = (id) => {
    const messages = [];
    const client = {
      id,
      ready: true,
      authed: true,
      closed: false,
      socket: {
        write(data) {
          messages.push(JSON.parse(data));
        },
        destroy() {},
      },
      filter: [],
      channels: null,
      sessionIds: new Set(),
      sessionAll: false,
      directSessions: new Map(),
      heartbeatRelayRooms: new Set(),
      outboundQueue: [],
      outboundBytes: 0,
      writing: false,
      authTimer: null,
    };
    return { client, messages };
  };
  const bridge = new ScBridge({ wallet: {} }, { requireAuth: false });
  bridge.attachDirectSession(directSession);
  const left = makeClient(1);
  const right = makeClient(2);
  bridge.clients.add(left.client);
  bridge.clients.add(right.client);
  const send = (client, message) => bridge._handleSocketData(client, JSON.stringify(message));
  const settle = () => new Promise((resolve) => setImmediate(resolve));

  send(left.client, { id: 1, type: 'session_subscribe', session_id: sessionA });
  send(right.client, { id: 2, type: 'session_subscribe', session_id: sessionB });
  send(left.client, { id: 3, type: 'session_open', remote, session_id: sessionA });
  send(right.client, { id: 4, type: 'session_open', remote, session_id: sessionB });
  await settle();
  send(left.client, {
    id: 5,
    type: 'session_send',
    remote,
    session_id: sessionA,
    frame: { t: 's.open', session_id: sessionA, spend_voucher: { rail: 'tnk' } },
  });
  send(right.client, {
    id: 6,
    type: 'session_send',
    remote,
    session_id: sessionB,
    frame: { t: 's.open', session_id: sessionB, spend_voucher: { rail: 'tnk' } },
  });
  await settle();

  assert.deepEqual(opened, [sessionA, sessionB]);
  assert.equal(left.client.directSessions.has(`${remote}:${sessionA}`), true);
  assert.equal(right.client.directSessions.has(`${remote}:${sessionB}`), true);
  assert.equal(left.client.directSessions.has(`${remote}:${sessionB}`), false);
  assert.equal(right.client.directSessions.has(`${remote}:${sessionA}`), false);

  for (const sessionId of [sessionA, sessionB]) {
    bridge.handleSessionFrame({
      session_id: sessionId,
      remote,
      direct: true,
      frame: { t: 's.accept', session_id: sessionId },
    });
  }
  send(left.client, {
    id: 7,
    type: 'session_send',
    remote,
    session_id: sessionA,
    frame: { t: 's.close', session_id: sessionA, reason: 'cancelled' },
  });
  await settle();
  bridge._dropClient(left.client, 'client disconnected');

  assert.deepEqual(closed, [sessionA]);
  assert.equal(right.client.directSessions.has(`${remote}:${sessionB}`), true);
  assert.equal(sessions.has(`${remote}:${sessionB}`), true);

  bridge.handleSessionFrame({
    session_id: sessionA,
    remote,
    direct: true,
    frame: { t: 's.receipt', session_id: sessionA, amount: '1' },
  });
  bridge.handleSessionFrame({
    session_id: sessionB,
    remote,
    direct: true,
    frame: { t: 's.receipt', session_id: sessionB, amount: '2' },
  });

  const rightFrames = right.messages
    .filter((message) => message.type === 'session_frame')
    .map((message) => [message.session_id, message.frame.t]);
  assert.deepEqual(
    left.messages
      .filter((message) => message.type === 'session_frame')
      .map((message) => [message.session_id, message.frame.t]),
    [[sessionA, 's.accept']],
  );
  assert.deepEqual(rightFrames, [
    [sessionB, 's.accept'],
    [sessionB, 's.receipt'],
  ]);
  assert.equal(
    [...left.messages, ...right.messages].some(
      (message) => message.type === 'session_taken_over'
    ),
    false,
  );

  send(right.client, { id: 8, type: 'session_close', remote, session_id: sessionB });
  assert.deepEqual(closed, [sessionA, sessionB]);
  assert.equal(right.client.directSessions.size, 0);
  assert.equal(sessions.size, 0);
  assert.deepEqual(
    sent.map(({ sessionId, frame }) => [sessionId, frame.t]),
    [
      [sessionA, 's.open'],
      [sessionB, 's.open'],
      [sessionA, 's.close'],
    ],
  );
});

test('SC-Bridge does not leak session frames to an unsubscribed local client', () => {
  const sessionId = 'ab'.repeat(32);
  assert.equal(sessionSubscriptionMatches(false, null, sessionId), false);
  assert.equal(sessionSubscriptionMatches(false, new Set(), sessionId), false);
  assert.equal(sessionSubscriptionMatches(false, new Set([sessionId]), sessionId), true);
  assert.equal(sessionSubscriptionMatches(true, null, sessionId), true);
});

test('SC-Bridge loopback delivers to the other local subscriber but never echoes to its sender', () => {
  const sessionId = 'cd'.repeat(32);
  const sender = {
    id: 1,
    ready: true,
    sessionAll: true,
    sessionIds: new Set(),
  };
  const receiver = {
    id: 2,
    ready: true,
    sessionAll: false,
    sessionIds: new Set([sessionId]),
  };
  const recipients = sessionFrameRecipients(new Set([sender, receiver]), sessionId, sender);
  assert.deepEqual(recipients, [receiver]);
});

test('SC-Bridge keeps concurrent provider enclave lanes independent on one transport peer', () => {
  const provider = '11'.repeat(32);
  const remote = '22'.repeat(32);
  const routes = Array.from({ length: 4 }, (_, index) => ({
    provider,
    enclaveId: (index + 1).toString(16).padStart(2, '0').repeat(32),
    roomId: (index + 17).toString(16).padStart(2, '0').repeat(16),
    sessionId: (index + 33).toString(16).padStart(2, '0').repeat(32),
  }));
  const workers = routes.map((route, index) => ({
    id: index + 1,
    ready: true,
    sessionAll: true,
    sessionIds: new Set(),
    directSessions: new Map(),
    route,
  }));
  const clients = new Set(workers);

  for (const route of routes) {
    const recipients = sessionFrameRecipients(clients, route.sessionId);
    assert.equal(recipients.length, workers.length);

    const activation = {
      t: 'tpm.activate_credential.challenge',
      provider: route.provider,
      enclave_id: route.enclaveId,
      room_id: route.roomId,
    };
    const handlers = recipients.filter(({ route: owned }) => (
      activation.provider === owned.provider
      && activation.enclave_id === owned.enclaveId
      && activation.room_id === owned.roomId
    ));
    assert.deepEqual(handlers.map(({ id }) => id), [
      routes.indexOf(route) + 1,
    ]);

    const owner = handlers[0];
    assert.equal(canOwnSession(owner.directSessions, remote, route.sessionId, 16), true);
    ownSession(owner.directSessions, remote, route.sessionId);
  }

  const disconnected = workers[1];
  const closed = [];
  closeOwnedSessions(disconnected.directSessions, (closedRemote, closedSessionId) => {
    closed.push(`${closedRemote}:${closedSessionId}`);
  });
  clients.delete(disconnected);

  assert.deepEqual(closed, [`${remote}:${routes[1].sessionId}`]);
  assert.equal(
    workers
      .filter((worker) => worker !== disconnected)
      .every((worker) => worker.directSessions.size === 1),
    true
  );
  assert.equal(clients.size, workers.length - 1);
});

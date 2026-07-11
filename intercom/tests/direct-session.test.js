import assert from 'node:assert/strict';
import test from 'node:test';
import b4a from 'b4a';
import DirectSession from '../features/direct-session/index.js';
import { isLocalPeer, loopbackSessionInfo } from '../features/sc-bridge/loopback.js';

const sessionId = 'ab'.repeat(32);
const remote = 'cd'.repeat(32);
const localPeer = '12'.repeat(32);

test('DirectSession exposes raised mx/s rate limits without relay semantics', () => {
  const directSession = new DirectSession({}, {});
  const stats = directSession.stats();

  assert.equal(stats.protocol, 'mx/s');
  assert.equal(stats.maxFrameBytes, 256 * 1024);
  assert.equal(stats.rateBytesPerSecond, 1_000_000);
  assert.equal(stats.rateBurstBytes, 1_000_000);
  assert.equal(stats.sendDrainTimeoutMs, 0);
  assert.equal(stats.connectMaxWaitMs, 120_000);
  assert.equal(stats.connectPollMs, 100);
  assert.equal(stats.sessionCount, 0);
});

test('DirectSession accepts explicit mx/s limiter config and ignores unsafe values', () => {
  const configured = new DirectSession({}, {
    maxFrameBytes: 4096,
    rateBytesPerSecond: 2_000_000,
    rateBurstBytes: 3_000_000,
    sendDrainTimeoutMs: 12_000,
    connectMaxWaitMs: 600_000,
    connectPollMs: 250,
  });

  assert.equal(configured.maxFrameBytes, 4096);
  assert.equal(configured.stats().maxFrameBytes, 4096);
  assert.equal(configured.stats().rateBytesPerSecond, 2_000_000);
  assert.equal(configured.stats().rateBurstBytes, 3_000_000);
  assert.equal(configured.stats().sendDrainTimeoutMs, 12_000);
  assert.equal(configured.stats().connectMaxWaitMs, 600_000);
  assert.equal(configured.stats().connectPollMs, 250);

  const fallback = new DirectSession({}, {
    maxFrameBytes: -1,
    rateBytesPerSecond: -1,
    rateBurstBytes: -1,
    sendDrainTimeoutMs: -1,
  });

  assert.equal(fallback.maxFrameBytes, 256 * 1024);
  assert.equal(fallback.stats().rateBytesPerSecond, 1_000_000);
  assert.equal(fallback.stats().rateBurstBytes, 1_000_000);
  assert.equal(fallback.stats().sendDrainTimeoutMs, 0);
});

test('DirectSession default drain wait ends on transport progress without a wall-clock cutoff', async () => {
  const directSession = new DirectSession({}, {});
  const session = {
    sessionId,
    channel: { drained: false },
    drainWaiters: new Set(),
  };
  let settled = false;
  const waiting = directSession._waitForDrain(session).then(() => {
    settled = true;
  });

  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(settled, false);
  assert.equal(session.drainWaiters.size, 1);
  directSession._resolveDrainWaiters(session);
  await waiting;
  assert.equal(settled, true);
});

test('DirectSession rejects oversized frames before transport send', () => {
  const directSession = new DirectSession({}, {});
  const oversized = {
    t: 's.delta',
    d: 'x'.repeat(directSession.maxFrameBytes),
  };

  assert.throws(
    () => directSession._validateFrame(oversized),
    /Session frame is too large/
  );
});

test('DirectSession receive bucket drops frames beyond the mx/s burst', () => {
  const frames = [];
  const frame = { t: 's.delta', d: 'token'.repeat(8) };
  const frameBytes = new DirectSession({}, {})._frameBytes(frame);
  const directSession = new DirectSession(
    {},
    {
      rateBytesPerSecond: 1,
      rateBurstBytes: frameBytes + 1,
      onFrame: (event) => frames.push(event),
    }
  );
  const receiveLimiter = directSession._newLimiter();

  directSession._handleFrame({ sessionId, remote, receiveLimiter }, frame);
  directSession._handleFrame({ sessionId, remote, receiveLimiter }, frame);

  assert.equal(frames.length, 1);
  assert.equal(frames[0].session_id, sessionId);
  assert.equal(frames[0].channel, `mx/s/${sessionId}`);
  assert.equal(frames[0].direct, true);
  assert.equal(frames[0].relayed, false);
  assert.deepEqual(frames[0].frame, frame);
});

test('DirectSession drops a malformed frame and still accepts the next valid frame', () => {
  const frames = [];
  const directSession = new DirectSession({}, { onFrame: (event) => frames.push(event) });
  const session = {
    sessionId,
    remote,
    receiveLimiter: directSession._newLimiter(),
  };

  assert.doesNotThrow(() => directSession._handleFrame(session, { missing: 'type' }));
  assert.doesNotThrow(() => directSession._handleFrame(session, { t: 's.delta', d: 'healthy' }));
  assert.equal(frames.length, 1);
  assert.equal(frames[0].frame.d, 'healthy');
});

test('DirectSession contains a throwing client frame handler and accepts the next frame', () => {
  let calls = 0;
  const directSession = new DirectSession({}, {
    onFrame: () => {
      calls += 1;
      if (calls === 1) throw new Error('injected frame failure');
    },
  });
  const session = {
    sessionId,
    remote,
    receiveLimiter: directSession._newLimiter(),
  };
  const originalError = console.error;
  console.error = () => {};
  try {
    assert.doesNotThrow(() => directSession._handleFrame(session, { t: 's.delta', d: 'one' }));
    assert.doesNotThrow(() => directSession._handleFrame(session, { t: 's.delta', d: 'two' }));
  } finally {
    console.error = originalError;
  }
  assert.equal(calls, 2);
});

test('DirectSession reuses an inbound session even when the swarm connection list races', async () => {
  const directSession = new DirectSession({ swarm: { connections: [] } }, {});
  const existing = {
    sessionId,
    remote,
    channel: { opened: true },
  };
  directSession.sessions.set(`${remote}:${sessionId}`, existing);

  const opened = await directSession.open(remote, sessionId);

  assert.equal(opened.session_id, sessionId);
  assert.equal(opened.remote, remote);
  assert.equal(opened.opened, true);
  assert.equal(opened.direct, true);
  assert.equal(opened.relayed, false);
});

test('DirectSession pins an already-connected peer for automatic reconnects', async () => {
  const joined = [];
  const connection = { remotePublicKey: b4a.from(remote, 'hex') };
  const directSession = new DirectSession({
    swarm: {
      connections: [connection],
      joinPeer: (key) => joined.push(b4a.toString(key, 'hex')),
    },
  }, {});

  const connected = await directSession.connectPeer(remote, 25);

  assert.deepEqual(joined, [remote]);
  assert.equal(connected.remote, remote);
  assert.equal(connected.connected, true);
  assert.equal(connected.direct, true);
});

test('ScBridge loopback helpers identify local direct session routes', () => {
  const peer = { wallet: { publicKey: localPeer.toUpperCase() } };
  assert.equal(isLocalPeer(localPeer, peer, null), true);
  assert.equal(isLocalPeer(remote, peer, null), false);
  assert.equal(isLocalPeer(localPeer, {}, { peerPubkey: localPeer }), true);

  assert.deepEqual(loopbackSessionInfo(localPeer.toUpperCase(), sessionId.toUpperCase(), {
    opened: true,
  }), {
    session_id: sessionId,
    channel: `mx/s/${sessionId}`,
    protocol: 'mx/s',
    remote: localPeer,
    direct: true,
    relayed: false,
    loopback: true,
    opened: true,
  });

  assert.throws(() => loopbackSessionInfo('bad', sessionId), /Invalid remote peer key/);
  assert.throws(() => loopbackSessionInfo(localPeer, 'bad'), /Invalid session_id/);
});

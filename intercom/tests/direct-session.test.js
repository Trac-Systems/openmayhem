import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { Duplex } from 'node:stream';
import test from 'node:test';
import b4a from 'b4a';
import c from '../node_modules/compact-encoding/index.js';
import DirectSession from '../features/direct-session/index.js';
import {
  boundedJsonEncoding,
  decodedJsonByteLength,
  decodedJsonWasRejected,
  jsonShapeWithinBounds,
} from '../features/bounded-json.js';
import { isLocalPeer, loopbackSessionInfo } from '../features/sc-bridge/loopback.js';

const sessionId = 'ab'.repeat(32);
const remote = 'cd'.repeat(32);
const localPeer = '12'.repeat(32);

const memoryDuplexPair = () => {
  let left = null;
  let right = null;
  left = new Duplex({
    read() {},
    write(chunk, _encoding, callback) {
      right.push(Buffer.from(chunk));
      callback();
    },
    final(callback) {
      right.push(null);
      callback();
    },
  });
  right = new Duplex({
    read() {},
    write(chunk, _encoding, callback) {
      left.push(Buffer.from(chunk));
      callback();
    },
    final(callback) {
      left.push(null);
      callback();
    },
  });
  return [left, right];
};

test('DirectSession exposes raised mx/s rate limits without relay semantics', () => {
  const directSession = new DirectSession({}, {});
  const stats = directSession.stats();

  assert.equal(stats.protocol, 'mx/s');
  assert.equal(stats.maxFrameBytes, 256 * 1024);
  assert.equal(stats.maxStringBytes, 256 * 1024);
  assert.equal(stats.rateBytesPerSecond, 1_000_000);
  assert.equal(stats.rateBurstBytes, 1_000_000);
  assert.equal(stats.receiveRateBurstBytes, 1_000_000 + (256 * 1024));
  assert.equal(stats.receiveBatchHeadroomBytes, 64 * 1024 * 1024);
  assert.equal(
    stats.receiveRateCapacityBytes,
    1_000_000 + (256 * 1024) + (64 * 1024 * 1024)
  );
  assert.equal(stats.sendDrainTimeoutMs, 0);
  assert.equal(stats.connectMaxWaitMs, 120_000);
  assert.equal(stats.connectPollMs, 100);
  assert.equal(stats.openMaxWaitMs, 120_000);
  assert.equal(stats.healthProtocol, 'mx/s-health');
  assert.equal(stats.healthIntervalMs, 5_000);
  assert.equal(stats.healthFreshMs, 15_000);
  assert.equal(stats.healthTimeoutMs, 30_000);
  assert.equal(stats.maxSessions, 1024);
  assert.equal(stats.maxSessionsPerConnection, 128);
  assert.equal(stats.sessionCount, 0);
});

test('DirectSession reports a relay route without changing frame semantics', () => {
  const connection = {};
  const directSession = new DirectSession({}, {
    transportInfo: (candidate, candidateRemote) => ({
      direct: candidate !== connection,
      relayed: candidate === connection,
      relay: candidateRemote === remote ? localPeer : null,
    }),
  });

  assert.deepEqual(directSession._peerInfo(remote, true, connection), {
    remote,
    connected: true,
    direct: false,
    relayed: true,
    relay: localPeer,
  });
  assert.deepEqual(directSession._peerInfo(remote, true, {}), {
    remote,
    connected: true,
    direct: true,
    relayed: false,
    relay: null,
  });
});

test('DirectSession accepts explicit mx/s limiter config and ignores unsafe values', () => {
  const configured = new DirectSession({}, {
    maxFrameBytes: 4096,
    maxStringBytes: 2048,
    rateBytesPerSecond: 2_000_000,
    rateBurstBytes: 3_000_000,
    receiveBatchHeadroomBytes: 8192,
    sendDrainTimeoutMs: 12_000,
    connectMaxWaitMs: 600_000,
    connectPollMs: 250,
  });

  assert.equal(configured.maxFrameBytes, 4096);
  assert.equal(configured.stats().maxFrameBytes, 4096);
  assert.equal(configured.stats().maxStringBytes, 2048);
  assert.equal(configured.stats().rateBytesPerSecond, 2_000_000);
  assert.equal(configured.stats().rateBurstBytes, 3_000_000);
  assert.equal(configured.stats().receiveRateBurstBytes, 3_004_096);
  assert.equal(configured.stats().receiveBatchHeadroomBytes, 8192);
  assert.equal(configured.stats().receiveRateCapacityBytes, 3_012_288);
  assert.equal(configured.stats().sendDrainTimeoutMs, 12_000);
  assert.equal(configured.stats().connectMaxWaitMs, 600_000);
  assert.equal(configured.stats().connectPollMs, 250);

  const fallback = new DirectSession({}, {
    maxFrameBytes: -1,
    rateBytesPerSecond: -1,
    rateBurstBytes: -1,
    receiveBatchHeadroomBytes: -1,
    sendDrainTimeoutMs: -1,
  });

  assert.equal(fallback.maxFrameBytes, 256 * 1024);
  assert.equal(fallback.stats().rateBytesPerSecond, 1_000_000);
  assert.equal(fallback.stats().rateBurstBytes, 1_000_000);
  assert.equal(fallback.stats().receiveBatchHeadroomBytes, 64 * 1024 * 1024);
  assert.equal(fallback.stats().sendDrainTimeoutMs, 0);

  const clamped = new DirectSession({}, {
    receiveBatchHeadroomBytes: Number.MAX_SAFE_INTEGER,
  });
  assert.equal(clamped.stats().receiveBatchHeadroomBytes, 64 * 1024 * 1024);
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

test('DirectSession receive bucket closes an offender instead of silently losing a frame', (t) => {
  let now = 10_000;
  t.mock.method(Date, 'now', () => now);
  const frames = [];
  const frame = { t: 's.delta', d: 'token'.repeat(8) };
  const frameBytes = new DirectSession({}, {})._frameBytes(frame);
  let closed = 0;
  const directSession = new DirectSession(
    {},
    {
      maxFrameBytes: frameBytes + 1,
      rateBytesPerSecond: 1,
      rateBurstBytes: frameBytes + 1,
      receiveBatchHeadroomBytes: frameBytes * 4,
      onFrame: (event) => frames.push(event),
    }
  );
  const connection = {};

  const session = {
    sessionId,
    remote,
    connection,
    receiveLimiter: directSession._newReceiveLimiter(),
    connectionReceiveLimiter: directSession._registerConnectionReceiveLimiter(connection),
    drainWaiters: new Set(),
    channel: { close: () => { closed += 1; } },
    closed: false,
  };
  directSession._handleFrame(session, frame);
  directSession._handleFrame(session, frame);
  directSession._handleFrame(session, frame);

  assert.equal(frames.length, 2);
  assert.equal(closed, 1);
  assert.equal(session.closed, true);
  assert.equal(frames[0].session_id, sessionId);
  assert.equal(frames[0].channel, `mx/s/${sessionId}`);
  assert.equal(frames[0].direct, true);
  assert.equal(frames[0].relayed, false);
  assert.deepEqual(frames[0].frame, frame);
});

test('DirectSession sender throttles a valid multi-frame payload instead of rejecting it', async () => {
  const directSession = new DirectSession({}, {
    maxFrameBytes: 128,
    rateBytesPerSecond: 10_000,
    rateBurstBytes: 128,
  });
  const session = {
    sessionId,
    sendLimiter: directSession._newLimiter(),
    channel: { closed: false, destroyed: false },
    closed: false,
  };

  await directSession._acquireSendRate(session, 100);
  await assert.doesNotReject(directSession._acquireSendRate(session, 100));
});

test('DirectSession receiver starts small and earns only finite batching headroom', (t) => {
  let now = 10_000;
  t.mock.method(Date, 'now', () => now);
  const directSession = new DirectSession({}, {
    maxFrameBytes: 64,
    rateBytesPerSecond: 128,
    rateBurstBytes: 128,
    receiveBatchHeadroomBytes: 256,
  });
  const receiveLimiter = directSession._newReceiveLimiter();

  assert.equal(receiveLimiter.capacity, 448);
  assert.equal(receiveLimiter.tokens, 192);
  assert.equal(directSession._checkRate(receiveLimiter, 193), false);

  now += 2_000;
  assert.equal(directSession._checkRate(receiveLimiter, 448), true);

  now += 60_000;
  assert.equal(directSession._checkRate(receiveLimiter, 449), false);
  assert.equal(receiveLimiter.tokens, 448);
});

test('DirectSession earned receive credit still fails closed on a sustained flood', (t) => {
  let now = 20_000;
  t.mock.method(Date, 'now', () => now);
  const frame = { t: 's.delta', d: 'bounded-rate-frame' };
  const frameBytes = new DirectSession({}, {})._frameBytes(frame);
  const frames = [];
  let closed = 0;
  const directSession = new DirectSession({}, {
    maxFrameBytes: frameBytes,
    rateBytesPerSecond: frameBytes,
    rateBurstBytes: frameBytes,
    receiveBatchHeadroomBytes: frameBytes * 2,
    onFrame: (event) => frames.push(event),
  });
  const connection = {};
  const session = {
    sessionId,
    remote,
    connection,
    receiveLimiter: directSession._newReceiveLimiter(),
    connectionReceiveLimiter: directSession._registerConnectionReceiveLimiter(connection),
    drainWaiters: new Set(),
    channel: { close: () => { closed += 1; } },
    closed: false,
  };

  now += 2_000;
  for (let second = 0; second < 4 && !session.closed; second += 1) {
    now += 1_000;
    directSession._handleFrame(session, frame);
    directSession._handleFrame(session, frame);
  }

  assert.equal(frames.length, 7);
  assert.equal(closed, 1);
  assert.equal(session.closed, true);
});

test('DirectSession shares earned batching headroom across sessions on one connection', (t) => {
  let now = 30_000;
  t.mock.method(Date, 'now', () => now);
  const directSession = new DirectSession({}, {
    maxFrameBytes: 100,
    rateBytesPerSecond: 100,
    rateBurstBytes: 100,
    receiveBatchHeadroomBytes: 200,
  });
  const connection = {};
  const makeSession = () => ({
    connection,
    receiveLimiter: directSession._newReceiveLimiter(),
    connectionReceiveLimiter: directSession._registerConnectionReceiveLimiter(connection),
  });
  const first = makeSession();
  const second = makeSession();

  assert.equal(first.connectionReceiveLimiter, second.connectionReceiveLimiter);
  assert.equal(first.connectionReceiveLimiter.tokens, 400);
  assert.equal(first.connectionReceiveLimiter.capacity, 600);

  now += 2_000;
  for (let frame = 0; frame < 4; frame += 1) {
    assert.equal(directSession._checkReceiveRate(first, 100), true);
  }
  for (let frame = 0; frame < 2; frame += 1) {
    assert.equal(directSession._checkReceiveRate(second, 100), true);
  }
  assert.equal(second.receiveLimiter.tokens, 200);
  assert.equal(directSession._checkReceiveRate(second, 1), false);

  directSession._unregisterConnectionReceiveLimiter(first);
  directSession._unregisterConnectionReceiveLimiter(second);
});

test('DirectSession accepts an idle-earned coalesced Protomux frame batch in order', async () => {
  const [leftConnection, rightConnection] = memoryDuplexPair();
  const leftKey = '31'.repeat(32);
  const rightKey = '32'.repeat(32);
  leftConnection.remotePublicKey = b4a.from(rightKey, 'hex');
  rightConnection.remotePublicKey = b4a.from(leftKey, 'hex');
  const received = [];
  const receiver = new DirectSession({
    swarm: { connections: new Set([leftConnection]), joinPeer: () => {} },
  }, {
    maxFrameBytes: 256,
    rateBytesPerSecond: 1024,
    rateBurstBytes: 256,
    receiveBatchHeadroomBytes: 2048,
    onFrame: ({ frame }) => received.push(frame),
  });
  const sender = new DirectSession({
    swarm: { connections: new Set([rightConnection]), joinPeer: () => {} },
  }, {
    maxFrameBytes: 256,
    rateBytesPerSecond: 0,
    rateBurstBytes: 256,
    receiveBatchHeadroomBytes: 2048,
  });

  try {
    receiver._prepareConnection(leftConnection);
    sender._prepareConnection(rightConnection);
    await receiver.open(rightKey, sessionId);
    const receiverRecord = receiver.sessions.get(`${rightKey}:${sessionId}`);
    const senderRecord = sender.sessions.get(`${leftKey}:${sessionId}`);
    assert.ok(receiverRecord);
    assert.ok(senderRecord);

    const frames = Array.from({ length: 8 }, (_, index) => ({
      t: 's.delta',
      i: index,
      d: 'x'.repeat(100),
    }));
    const batchBytes = frames.reduce(
      (total, frame) => total + receiver._frameBytes(frame),
      0
    );
    assert.ok(batchBytes > receiver._receiveRateBurstBytes());
    assert.ok(batchBytes < receiver._receiveRateCapacityBytes());

    const idleMs = Math.ceil(
      (receiver.receiveBatchHeadroomBytes * 1000) / receiver.rateBytesPerSecond
    ) + 100;
    receiverRecord.receiveLimiter.lastRefill -= idleMs;
    receiverRecord.connectionReceiveLimiter.lastRefill -= idleMs;

    const mux = rightConnection.userData;
    mux.cork();
    for (const frame of frames) {
      assert.equal(senderRecord.message.send(frame), true);
    }
    mux.uncork();
    await new Promise((resolve) => setTimeout(resolve, 0));

    assert.equal(receiverRecord.closed, false);
    assert.deepEqual(received.map((frame) => frame.i), frames.map((frame) => frame.i));
  } finally {
    receiver._dropHealthConnection(leftConnection);
    sender._dropHealthConnection(rightConnection);
    leftConnection.destroy();
    rightConnection.destroy();
  }
});

test('DirectSession preserves every model class across fragmented and coalesced direct delivery', async () => {
  const requestId = 'request-flowrate-matrix';
  const payloads = [
    {
      modelClass: 'text',
      body: { d: 'large text output '.repeat(100) },
    },
    {
      modelClass: 'embeddings',
      body: { embeddings: [Array.from({ length: 128 }, (_, index) => index / 128)] },
    },
    {
      modelClass: 'image',
      body: {
        artifact: {
          id: 'image-1',
          content_type: 'image/png',
          encoding: 'hex',
          data: '89'.repeat(900),
        },
      },
    },
    {
      modelClass: 'speech',
      body: {
        artifact: {
          id: 'speech-1',
          content_type: 'audio/wav',
          encoding: 'hex',
          data: '52'.repeat(900),
        },
      },
    },
    {
      modelClass: 'music',
      body: {
        artifact: {
          id: 'music-1',
          content_type: 'audio/wav',
          encoding: 'hex',
          data: '57'.repeat(900),
        },
      },
    },
    {
      modelClass: 'transcription',
      body: {
        transcription: {
          text: 'recognized speech '.repeat(100),
          language: 'en',
        },
      },
    },
    {
      modelClass: 'video',
      body: {
        artifact: {
          id: 'video-1',
          content_type: 'video/mp4',
          encoding: 'hex',
          data: '00'.repeat(900),
        },
      },
    },
  ];
  const deliveries = [
    { name: 'fragmented-direct', coalesced: false },
    { name: 'coalesced-direct', coalesced: true },
  ];

  for (const [deliveryIndex, delivery] of deliveries.entries()) {
    const [leftConnection, rightConnection] = memoryDuplexPair();
    const leftKey = (40 + deliveryIndex).toString(16).padStart(2, '0').repeat(32);
    const rightKey = (50 + deliveryIndex).toString(16).padStart(2, '0').repeat(32);
    const matrixSessionId = (60 + deliveryIndex).toString(16).padStart(2, '0').repeat(32);
    leftConnection.remotePublicKey = b4a.from(rightKey, 'hex');
    rightConnection.remotePublicKey = b4a.from(leftKey, 'hex');
    const received = [];
    const closes = [];
    const receiver = new DirectSession({
      swarm: { connections: new Set([leftConnection]), joinPeer: () => {} },
    }, {
      maxFrameBytes: 4096,
      rateBytesPerSecond: 4096,
      rateBurstBytes: 1024,
      receiveBatchHeadroomBytes: 64 * 1024,
      onFrame: (event) => received.push(event),
      onClose: (event) => closes.push(event),
    });
    const sender = new DirectSession({
      swarm: { connections: new Set([rightConnection]), joinPeer: () => {} },
    }, {
      maxFrameBytes: 4096,
      rateBytesPerSecond: 0,
      rateBurstBytes: 4096,
      receiveBatchHeadroomBytes: 64 * 1024,
    });

    try {
      receiver._prepareConnection(leftConnection);
      sender._prepareConnection(rightConnection);
      await receiver.open(rightKey, matrixSessionId);
      const receiverRecord = receiver.sessions.get(`${rightKey}:${matrixSessionId}`);
      const senderRecord = sender.sessions.get(`${leftKey}:${matrixSessionId}`);
      assert.ok(receiverRecord, `${delivery.name} receiver session`);
      assert.ok(senderRecord, `${delivery.name} sender session`);

      const frames = payloads.flatMap(({ modelClass, body }, classIndex) => (
        [0, 1].map((part) => ({
          t: 's.delta',
          rid: requestId,
          i: (classIndex * 2) + part,
          d: '',
          model_class: modelClass,
          wire_part: part,
          ...body,
        }))
      ));
      const batchBytes = frames.reduce(
        (total, frame) => total + receiver._frameBytes(frame),
        0
      );
      assert.ok(batchBytes > receiver._receiveRateBurstBytes(), delivery.name);
      assert.ok(batchBytes < receiver._receiveRateCapacityBytes(), delivery.name);
      const idleMs = Math.ceil(
        (receiver.receiveBatchHeadroomBytes * 1000) / receiver.rateBytesPerSecond
      ) + 100;
      receiverRecord.receiveLimiter.lastRefill -= idleMs;
      receiverRecord.connectionReceiveLimiter.lastRefill -= idleMs;

      const mux = rightConnection.userData;
      if (delivery.coalesced) mux.cork();
      for (const frame of frames) {
        assert.equal(senderRecord.message.send(frame), true, delivery.name);
      }
      if (delivery.coalesced) mux.uncork();
      await new Promise((resolve) => setTimeout(resolve, 10));

      assert.equal(
        receiverRecord.closed,
        false,
        `${delivery.name}: ${JSON.stringify(closes)}`
      );
      assert.deepEqual(
        received.map(({ frame }) => frame),
        frames,
        delivery.name
      );
      assert.ok(
        received.every(({ direct }) => direct === true),
        delivery.name
      );
      assert.ok(
        received.every(({ relayed }) => relayed === false),
        delivery.name
      );
      assert.ok(
        received.every(({ relay }) => relay === null),
        delivery.name
      );
    } finally {
      receiver._dropHealthConnection(leftConnection);
      sender._dropHealthConnection(rightConnection);
      leftConnection.destroy();
      rightConnection.destroy();
    }
  }
});

test('DirectSession send failure closes and reclaims only the failed session', async () => {
  const directSession = new DirectSession({}, {});
  let closed = 0;
  const failed = {
    sessionId,
    remote,
    channel: {
      opened: true,
      close: () => { closed += 1; },
    },
    message: {
      send: () => { throw new Error('injected transport send failure'); },
    },
    sendLimiter: directSession._newLimiter(),
    drainWaiters: new Set(),
    closed: false,
  };
  const healthyId = 'ef'.repeat(32);
  const healthy = {
    sessionId: healthyId,
    remote,
    channel: { opened: true },
    message: { send: () => true },
    sendLimiter: directSession._newLimiter(),
    drainWaiters: new Set(),
    closed: false,
  };
  directSession.sessions.set(`${remote}:${sessionId}`, failed);
  directSession.sessions.set(`${remote}:${healthyId}`, healthy);

  await assert.rejects(
    directSession.send(remote, sessionId, { t: 's.delta', d: 'fail' }),
    /injected transport send failure/
  );

  assert.equal(failed.closed, true);
  assert.equal(closed, 1);
  assert.equal(directSession.sessions.has(`${remote}:${sessionId}`), false);
  assert.equal(directSession.sessions.get(`${remote}:${healthyId}`), healthy);
});

test('DirectSession close racing a backpressured send rejects and reclaims promptly', async () => {
  const directSession = new DirectSession({}, {});
  const record = {
    sessionId,
    remote,
    channel: { opened: true, drained: false, close: () => {} },
    message: { send: () => false },
    sendLimiter: directSession._newLimiter(),
    drainWaiters: new Set(),
    closed: false,
  };
  directSession.sessions.set(`${remote}:${sessionId}`, record);

  const sending = directSession.send(remote, sessionId, { t: 's.delta', d: 'wait' });
  await new Promise((resolve) => setTimeout(resolve, 0));
  directSession.close(remote, sessionId);

  await assert.rejects(sending, /closed locally/);
  assert.equal(record.drainWaiters.size, 0);
  assert.equal(directSession.sessions.size, 0);
});

test('DirectSession contains concurrent send-close churn across many hostile sessions', async () => {
  const directSession = new DirectSession({}, {
    maxSessions: 512,
    maxSessionsPerConnection: 512,
  });
  const sends = [];
  const records = [];
  for (let index = 0; index < 256; index += 1) {
    const churnId = index.toString(16).padStart(64, '0');
    const record = {
      sessionId: churnId,
      remote,
      channel: { opened: true, drained: false, close: () => {} },
      message: { send: () => false },
      sendLimiter: directSession._newLimiter(),
      drainWaiters: new Set(),
      closed: false,
    };
    directSession.sessions.set(`${remote}:${churnId}`, record);
    records.push(record);
    sends.push(directSession.send(remote, churnId, { t: 's.delta', d: `frame-${index}` }));
  }
  const outcomes = Promise.allSettled(sends);
  await new Promise((resolve) => setTimeout(resolve, 0));
  for (const record of [...records].reverse()) {
    directSession.close(remote, record.sessionId);
  }

  const settled = await outcomes;
  assert.equal(settled.every((result) => result.status === 'rejected'), true);
  assert.equal(records.every((record) => record.drainWaiters.size === 0), true);
  assert.equal(directSession.sessions.size, 0);
});

test('bounded JSON drops a complete oversized payload before UTF-8 decode', () => {
  const encoding = boundedJsonEncoding(64, 'test frame');
  const state = { buffer: b4a.concat([b4a.from([65]), b4a.alloc(65)]), start: 0, end: 66 };
  const decoded = encoding.decode(state);
  assert.equal(decodedJsonWasRejected(decoded), true);
  assert.equal(decodedJsonByteLength(decoded), 65);
  assert.equal(state.start, 66);
});

test('bounded JSON rejects excessive nesting without a recursive second size pass', () => {
  let nested = { t: 's.delta' };
  for (let depth = 0; depth < 300; depth += 1) nested = { child: nested };
  assert.equal(jsonShapeWithinBounds(nested), false);

  const text = JSON.stringify(nested);
  const buffer = c.encode(c.utf8, text);
  const state = { buffer, start: 0, end: buffer.length };
  const decoded = boundedJsonEncoding(buffer.length, 'nested frame').decode(state);
  assert.equal(decodedJsonWasRejected(decoded), true);
  assert.equal(decodedJsonByteLength(decoded), b4a.byteLength(text, 'utf8'));
});

test('bounded JSON rejects an individually oversized string before dispatch', () => {
  const text = JSON.stringify({ t: 's.delta', d: 'é'.repeat(17) });
  const buffer = c.encode(c.utf8, text);
  const state = { buffer, start: 0, end: buffer.length };
  const decoded = boundedJsonEncoding(
    buffer.length,
    'string-bounded frame',
    { maxStringBytes: 32 }
  ).decode(state);

  assert.equal(decodedJsonWasRejected(decoded), true);
  assert.equal(decodedJsonByteLength(decoded), b4a.byteLength(text, 'utf8'));

  const directSession = new DirectSession({}, {
    maxFrameBytes: 1024,
    maxStringBytes: 32,
  });
  assert.throws(
    () => directSession._validateFrame({ t: 's.delta', d: 'é'.repeat(17) }),
    /string bounds/
  );
});

test('DirectSession dispatch preserves frames while adding route metadata', () => {
  const directConnection = { remotePublicKey: b4a.from(remote, 'hex') };
  const relayConnection = { remotePublicKey: b4a.from(remote, 'hex') };
  const frames = [];
  const directSession = new DirectSession({}, {
    transportInfo: (connection) => ({
      relayed: connection === relayConnection,
      relay: connection === relayConnection ? localPeer : null,
    }),
    onFrame: (event) => frames.push(event),
  });
  const makeSession = (connection, id) => ({
    sessionId: id,
    remote,
    connection,
    receiveLimiter: directSession._newReceiveLimiter(),
    channel: { closed: false, destroyed: false },
    closed: false,
  });
  const frame = { t: 's.delta', i: 0, d: 'same frame', fin: 'stop' };

  directSession._handleFrame(makeSession(directConnection, sessionId), frame);
  directSession._handleFrame(makeSession(relayConnection, 'ef'.repeat(32)), frame);

  assert.equal(frames.length, 2);
  assert.deepEqual(frames[0].frame, frame);
  assert.deepEqual(frames[1].frame, frame);
  assert.equal(frames[0].direct, true);
  assert.equal(frames[0].relayed, false);
  assert.equal(frames[1].direct, false);
  assert.equal(frames[1].relayed, true);
  assert.equal(frames[1].relay, localPeer);
});

test('DirectSession bounds inbound sessions per connection and globally', () => {
  const connection = { remotePublicKey: b4a.from(remote, 'hex') };
  const directSession = new DirectSession({}, {
    maxSessions: 2,
    maxSessionsPerConnection: 1,
  });
  directSession.sessions.set(`${remote}:${sessionId}`, {
    sessionId,
    remote,
    connection,
    closed: false,
  });

  assert.equal(directSession._hasSessionCapacity(connection, sessionId), true);
  assert.equal(directSession._hasSessionCapacity(connection, 'ef'.repeat(32)), false);
});

test('DirectSession reclaims repeated session churn on one persistent connection', () => {
  const directSession = new DirectSession({}, {
    maxSessions: 16,
    maxSessionsPerConnection: 4,
  });
  const connection = { remotePublicKey: b4a.from(remote, 'hex') };
  let channelCloses = 0;
  for (let index = 0; index < 1000; index += 1) {
    const churnSessionId = index.toString(16).padStart(64, '0');
    const record = {
      sessionId: churnSessionId,
      remote,
      connection,
      channel: { close: () => { channelCloses += 1; } },
      drainWaiters: new Set(),
      closed: false,
    };
    directSession.sessions.set(`${remote}:${churnSessionId}`, record);
    directSession._closeRecord(record, new Error('churn close'), true);
  }

  assert.equal(channelCloses, 1000);
  assert.equal(directSession.sessions.size, 0);
});

test('DirectSession closes only the malformed-frame session and keeps other sessions healthy', () => {
  const frames = [];
  const directSession = new DirectSession({}, { onFrame: (event) => frames.push(event) });
  let closed = 0;
  const malformed = {
    sessionId,
    remote,
    receiveLimiter: directSession._newLimiter(),
    drainWaiters: new Set(),
    channel: { close: () => { closed += 1; } },
    closed: false,
  };
  const healthy = {
    sessionId: 'ef'.repeat(32),
    remote,
    receiveLimiter: directSession._newLimiter(),
    drainWaiters: new Set(),
    channel: { closed: false, destroyed: false },
    closed: false,
  };

  assert.doesNotThrow(() => directSession._handleFrame(malformed, { missing: 'type' }));
  assert.equal(malformed.closed, true);
  assert.equal(closed, 1);
  assert.doesNotThrow(() => directSession._handleFrame(healthy, { t: 's.delta', d: 'healthy' }));
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

test('DirectSession moves a session to a replacement Hyperswarm connection', () => {
  const oldConnection = { remotePublicKey: b4a.from(remote, 'hex') };
  const newConnection = { remotePublicKey: b4a.from(remote, 'hex') };
  let oldChannelCloses = 0;
  const oldSession = {
    sessionId,
    remote,
    connection: oldConnection,
    channel: { close: () => { oldChannelCloses += 1; } },
    drainWaiters: new Set(),
    closed: false,
  };
  const directSession = new DirectSession({}, {});
  directSession.sessions.set(`${remote}:${sessionId}`, oldSession);
  directSession._prepareConnection = () => {};
  directSession._muxForConnection = () => ({
    createChannel: () => {
      const channel = {
        opened: true,
        addMessage: () => ({ send: () => true }),
        open: () => {},
      };
      return channel;
    },
  });

  const replacement = directSession._ensureSession(newConnection, sessionId);

  assert.equal(oldSession.closed, true);
  assert.equal(oldChannelCloses, 1);
  assert.equal(replacement.connection, newConnection);
  assert.equal(replacement.closed, false);
  assert.equal(directSession.sessions.get(`${remote}:${sessionId}`), replacement);

  directSession._dropConnection(oldConnection);
  assert.equal(replacement.closed, false);
  assert.equal(directSession.sessions.get(`${remote}:${sessionId}`), replacement);
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
  directSession._prepareConnection = () => {};
  directSession.connectionHealth.set(connection, {
    connection,
    lastAckAt: Date.now(),
  });

  const connected = await directSession.connectPeer(remote, 25);

  assert.deepEqual(joined, [remote]);
  assert.equal(connected.remote, remote);
  assert.equal(connected.connected, true);
  assert.equal(connected.direct, true);
  assert.equal(directSession.stats().lastConnectAttempt.remote, remote);
  assert.equal(directSession.stats().lastConnectAttempt.state, 'connected');
});

test('DirectSession re-arms a disconnected peer already parked as explicit', async () => {
  const left = [];
  const joined = [];
  const connections = [];
  const connection = { remotePublicKey: b4a.from(remote, 'hex') };
  let directSession = null;
  directSession = new DirectSession({
    swarm: {
      connections,
      leavePeer: (key) => left.push(b4a.toString(key, 'hex')),
      joinPeer: (key) => {
        joined.push(b4a.toString(key, 'hex'));
        connections.push(connection);
        directSession.connectionHealth.set(connection, {
          connection,
          lastAckAt: Date.now(),
        });
      },
    },
  }, {});
  directSession._prepareConnection = () => {};
  directSession.explicitPeers.add(remote);

  const connected = await directSession.connectPeer(remote, 25);

  assert.deepEqual(left, [remote]);
  assert.deepEqual(joined, [remote]);
  assert.equal(connected.remote, remote);
  assert.equal(connected.connected, true);
  assert.equal(directSession.stats().lastConnectAttempt.state, 'connected');
});

test('DirectSession prefers the most recently bidirectionally healthy connection', () => {
  const stale = { remotePublicKey: b4a.from(remote, 'hex') };
  const healthy = { remotePublicKey: b4a.from(remote, 'hex') };
  const directSession = new DirectSession({
    swarm: { connections: [stale, healthy] },
  }, {});
  directSession.connectionHealth.set(stale, { connection: stale, lastAckAt: 1 });
  directSession.connectionHealth.set(healthy, { connection: healthy, lastAckAt: Date.now() });

  assert.equal(directSession._findConnection(remote), healthy);
});

test('DirectSession selects a proven feature-owned connection without replacing swarm transport', () => {
  const direct = { remotePublicKey: b4a.from(remote, 'hex') };
  const relayed = { remotePublicKey: b4a.from(remote, 'hex'), destroyed: false, closed: false };
  const unrelated = { remotePublicKey: b4a.from(remote, 'hex') };
  const swarmConnections = new Set([direct]);
  const directSession = new DirectSession({
    swarm: { connections: swarmConnections },
  }, {});
  directSession.connectionHealth.set(direct, { connection: direct, lastAckAt: Date.now() });
  directSession.connectionHealth.set(relayed, {
    connection: relayed,
    lastAckAt: Date.now(),
    proven: true,
  });
  directSession._prepareConnectionUnchecked = () => {};

  assert.equal(directSession.preferConnection(remote, relayed), null);
  assert.equal(directSession._findConnection(remote), relayed);
  assert.deepEqual(directSession._connectionsForRemote(remote), [direct]);
  assert.deepEqual(Array.from(swarmConnections), [direct]);
  assert.equal(directSession.clearPreferredConnection(remote, unrelated), false);
  assert.equal(directSession._findConnection(remote), relayed);
  assert.equal(directSession.clearPreferredConnection(remote, relayed), true);
  assert.equal(directSession._findConnection(remote), direct);
  assert.deepEqual(Array.from(swarmConnections), [direct]);
});

test('DirectSession keeps application sessions disabled until feature-owned health proof succeeds', async () => {
  const connection = new EventEmitter();
  connection.remotePublicKey = b4a.from(remote, 'hex');
  connection.destroyed = false;
  connection.closed = false;
  const pairedProtocols = [];
  const mux = {
    pair({ protocol }) {
      pairedProtocols.push(protocol);
    },
    createChannel({ protocol }) {
      return {
        protocol,
        opened: false,
        closed: false,
        destroyed: false,
        addMessage: () => ({ send: () => true }),
        open() {},
        fullyOpened: async () => true,
      };
    },
  };
  const directSession = new DirectSession({ swarm: { connections: new Set() } }, {});
  directSession._muxForConnection = () => mux;
  let releaseProof = null;
  directSession._ensureConnectionHealthy = async (candidate) => {
    await new Promise((resolve) => { releaseProof = resolve; });
    const health = directSession.connectionHealth.get(candidate);
    health.proven = true;
    health.lastAckAt = Date.now();
  };

  const proving = directSession.proveConnection(connection, 1_000);
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(pairedProtocols, ['mx/s-health']);
  assert.equal(directSession.pairedConnections.has(connection), false);

  releaseProof();
  await proving;
  assert.equal(directSession.pairedConnections.has(connection), false);
  directSession.preferConnection(remote, connection);
  assert.deepEqual(pairedProtocols, ['mx/s-health', 'mx/s']);
  assert.equal(directSession.pairedConnections.has(connection), true);
  directSession._dropHealthConnection(connection);
});

test('DirectSession refuses unproven and stale preferred connections', () => {
  const connection = { remotePublicKey: b4a.from(remote, 'hex') };
  const directSession = new DirectSession({}, { healthFreshMs: 5_000 });
  directSession.connectionHealth.set(connection, {
    connection,
    proven: false,
    lastAckAt: Date.now(),
  });
  assert.throws(
    () => directSession.preferConnection(remote, connection),
    /fresh bidirectional health proof/
  );

  directSession.connectionHealth.set(connection, {
    connection,
    proven: true,
    lastAckAt: Date.now() - 5_001,
  });
  assert.throws(
    () => directSession.preferConnection(remote, connection),
    /fresh bidirectional health proof/
  );
});

test('DirectSession health frames prove both transport directions', async () => {
  const sent = [];
  const directSession = new DirectSession({}, {});
  const health = {
    connection: { remotePublicKey: b4a.from(remote, 'hex') },
    opened: true,
    message: { send: (frame) => sent.push(frame) },
    probes: new Map(),
    waiters: new Map(),
    lastAckAt: 0,
    proven: false,
  };

  directSession._handleHealthFrame(health, { t: 'ping', n: 'probe-one' });
  assert.deepEqual(sent, [{ t: 'pong', n: 'probe-one' }]);

  let resolved = false;
  const nonce = directSession._sendHealthPing(health, {
    resolve: () => { resolved = true; },
    reject: () => {},
    timer: null,
  });
  directSession._handleHealthFrame(health, { t: 'pong', n: nonce });
  await new Promise((resolve) => setTimeout(resolve, 0));

  assert.equal(resolved, true);
  assert.equal(health.waiters.size, 0);
  assert.equal(health.proven, true);
  assert.equal(directSession._healthIsFresh(health), true);
});

test('DirectSession ignores an unsolicited pong before health promotion', () => {
  const directSession = new DirectSession({}, {});
  const health = {
    connection: { remotePublicKey: b4a.from(remote, 'hex') },
    opened: true,
    message: { send: () => true },
    probes: new Map(),
    waiters: new Map(),
    lastAckAt: 0,
    unhealthySince: 42,
    proven: false,
  };

  directSession._handleHealthFrame(health, { t: 'pong', n: 'forged-by-peer' });

  assert.equal(health.lastAckAt, 0);
  assert.equal(health.unhealthySince, 42);
  assert.equal(health.proven, false);
  assert.equal(directSession.stats().connections.length, 0);
});

test('DirectSession completes a real bidirectional Protomux health probe', async () => {
  const [leftConnection, rightConnection] = memoryDuplexPair();
  const leftKey = '11'.repeat(32);
  const rightKey = '22'.repeat(32);
  leftConnection.remotePublicKey = b4a.from(rightKey, 'hex');
  rightConnection.remotePublicKey = b4a.from(leftKey, 'hex');
  const left = new DirectSession({
    swarm: { connections: new Set([leftConnection]), joinPeer: () => {} },
  }, {});
  const right = new DirectSession({
    swarm: { connections: new Set([rightConnection]), joinPeer: () => {} },
  }, {});
  left._prepareConnection(leftConnection);
  right._prepareConnection(rightConnection);

  await Promise.all([
    left._ensureConnectionHealthy(leftConnection, 1_000),
    right._ensureConnectionHealthy(rightConnection, 1_000),
  ]);

  assert.equal(left.stats().connections[0].healthy, true);
  assert.equal(right.stats().connections[0].healthy, true);
  leftConnection.destroy();
  rightConnection.destroy();
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

test('DirectSession keeps and uses a legacy connection when health retirement is disabled', async () => {
  let destroyed = 0;
  const connection = {
    remotePublicKey: b4a.from(remote, 'hex'),
    destroy: () => { destroyed += 1; },
  };
  const directSession = new DirectSession({
    swarm: {
      connections: [connection],
      joinPeer: () => {},
    },
  }, { healthTimeoutMs: 0 });
  directSession._prepareConnection = () => {};

  const connected = await directSession.connectPeer(remote, 200);

  assert.equal(connected.connected, true);
  assert.equal(destroyed, 0);
  const attempt = directSession.stats().lastConnectAttempt;
  assert.equal(attempt.state, 'connected');
  assert.equal(attempt.verified, false);
});

test('DirectSession accepts a late health acknowledgement from a previously verified peer', async () => {
  const connection = {
    remotePublicKey: b4a.from(remote, 'hex'),
    destroyed: false,
    closed: false,
  };
  const directSession = new DirectSession({
    swarm: {
      connections: [connection],
      joinPeer: () => {},
    },
  }, {
    connectPollMs: 1,
    healthFreshMs: 100,
    healthTimeoutMs: 10,
  });
  const health = {
    connection,
    opened: true,
    lastAckAt: 0,
    unhealthySince: 0,
    proven: true,
  };
  directSession.connectionHealth.set(connection, health);
  directSession._prepareConnection = () => {};
  directSession._ensureConnectionHealthy = async () => {
    setTimeout(() => {
      health.lastAckAt = Date.now();
    }, 2);
    throw new Error('initial probe raced channel startup');
  };

  const connected = await directSession.connectPeer(remote, 100);

  assert.equal(connected.connected, true);
  assert.equal(directSession.lastConnectAttempt.state, 'connected');
  assert.equal(directSession.lastConnectAttempt.verified, true);
  assert.equal(health.proven, true);
});

test('DirectSession retires a verified dead connection despite stale active session state', () => {
  let destroyed = 0;
  const connection = {
    remotePublicKey: b4a.from(remote, 'hex'),
    destroy: () => { destroyed += 1; },
  };
  const directSession = new DirectSession({}, { healthTimeoutMs: 10 });
  const health = {
    connection,
    channel: { closed: false, destroyed: false },
    message: null,
    opened: false,
    lastAckAt: 0,
    unhealthySince: Date.now() - 11,
    proven: true,
    timer: null,
    probes: new Map(),
    waiters: new Map(),
  };
  directSession.explicitPeers.add(remote);
  directSession.connectionHealth.set(connection, health);

  directSession.sessions.set(`${remote}:${sessionId}`, {
    sessionId,
    remote,
    connection,
    channel: { close: () => {} },
    drainWaiters: new Set(),
    closed: false,
  });
  directSession._healthTick(health);
  assert.equal(destroyed, 1);
  assert.equal(directSession.connectionHealth.has(connection), false);
});

test('DirectSession retires a stale preferred relay without requiring explicit-peer ownership', () => {
  let destroyed = 0;
  const connection = {
    remotePublicKey: b4a.from(remote, 'hex'),
    destroy: () => { destroyed += 1; },
  };
  const directSession = new DirectSession({}, { healthTimeoutMs: 10 });
  const health = {
    connection,
    channel: { closed: false, destroyed: false },
    message: null,
    opened: false,
    lastAckAt: 0,
    unhealthySince: Date.now() - 11,
    proven: true,
    timer: null,
    probes: new Map(),
    waiters: new Map(),
  };
  directSession.preferredConnections.set(remote, connection);
  directSession.connectionHealth.set(connection, health);

  directSession._healthTick(health);

  assert.equal(destroyed, 1);
  assert.equal(directSession.connectionHealth.has(connection), false);
});

test('DirectSession does not retire an explicit legacy connection without proven health support', () => {
  let destroyed = 0;
  const connection = {
    remotePublicKey: b4a.from(remote, 'hex'),
    destroy: () => { destroyed += 1; },
  };
  const directSession = new DirectSession({}, { healthTimeoutMs: 10 });
  const health = {
    connection,
    channel: { closed: false, destroyed: false },
    message: null,
    opened: false,
    lastAckAt: 0,
    unhealthySince: Date.now() - 100,
    proven: false,
    timer: null,
    probes: new Map(),
    waiters: new Map(),
  };
  directSession.explicitPeers.add(remote);
  directSession.connectionHealth.set(connection, health);

  directSession._healthTick(health);

  assert.equal(destroyed, 0);
  assert.equal(directSession.connectionHealth.has(connection), true);
});

test('DirectSession re-arms a pinned peer after its retired connection closes', () => {
  const left = [];
  const joined = [];
  const directSession = new DirectSession({
    swarm: {
      leavePeer: (key) => left.push(b4a.toString(key, 'hex')),
      joinPeer: (key) => joined.push(b4a.toString(key, 'hex')),
    },
  });
  directSession.explicitPeers.add(remote);

  assert.equal(directSession._rejoinExplicitPeer(remote), true);
  assert.deepEqual(left, [remote]);
  assert.deepEqual(joined, [remote]);
  assert.equal(directSession._rejoinExplicitPeer('ff'.repeat(32)), false);
});

test('DirectSession suspends only the relayed peer reconnect intent and resumes it', () => {
  const left = [];
  const joined = [];
  const reconnecting = [];
  const directSession = new DirectSession({
    swarm: {
      leavePeer: (key) => left.push(b4a.toString(key, 'hex')),
      joinPeer: (key) => joined.push(b4a.toString(key, 'hex')),
      peers: new Map([[remote, { reconnect: (enabled) => reconnecting.push(enabled) }]]),
    },
  });
  directSession.explicitPeers.add(remote);

  assert.equal(directSession.suspendReconnect(remote), true);
  assert.deepEqual(left, [remote]);
  assert.deepEqual(reconnecting, [false]);
  assert.deepEqual(joined, []);
  assert.equal(directSession._rejoinExplicitPeer(remote), false);
  assert.deepEqual(directSession.stats().reconnectSuspended, [remote]);

  assert.equal(directSession.resumeReconnect(remote), true);
  assert.deepEqual(left, [remote, remote]);
  assert.deepEqual(joined, [remote]);
  assert.deepEqual(reconnecting, [false, true]);
  assert.deepEqual(directSession.stats().reconnectSuspended, []);
  assert.equal(directSession.resumeReconnect(remote), false);
});

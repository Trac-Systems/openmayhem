import assert from 'node:assert/strict';
import test from 'node:test';
import DirectSession from '../features/direct-session/index.js';

const sessionId = 'ab'.repeat(32);
const remote = 'cd'.repeat(32);

test('DirectSession exposes raised mx/s rate limits without relay semantics', () => {
  const directSession = new DirectSession({}, {});
  const stats = directSession.stats();

  assert.equal(stats.protocol, 'mx/s');
  assert.equal(stats.maxFrameBytes, 256 * 1024);
  assert.equal(stats.rateBytesPerSecond, 1_000_000);
  assert.equal(stats.rateBurstBytes, 1_000_000);
  assert.equal(stats.sendDrainTimeoutMs, 30_000);
  assert.equal(stats.sessionCount, 0);
});

test('DirectSession accepts explicit mx/s limiter config and ignores unsafe values', () => {
  const configured = new DirectSession({}, {
    maxFrameBytes: 4096,
    rateBytesPerSecond: 2_000_000,
    rateBurstBytes: 3_000_000,
    sendDrainTimeoutMs: 12_000,
  });

  assert.equal(configured.maxFrameBytes, 4096);
  assert.equal(configured.stats().maxFrameBytes, 4096);
  assert.equal(configured.stats().rateBytesPerSecond, 2_000_000);
  assert.equal(configured.stats().rateBurstBytes, 3_000_000);
  assert.equal(configured.stats().sendDrainTimeoutMs, 12_000);

  const fallback = new DirectSession({}, {
    maxFrameBytes: -1,
    rateBytesPerSecond: -1,
    rateBurstBytes: -1,
    sendDrainTimeoutMs: -1,
  });

  assert.equal(fallback.maxFrameBytes, 256 * 1024);
  assert.equal(fallback.stats().rateBytesPerSecond, 1_000_000);
  assert.equal(fallback.stats().rateBurstBytes, 1_000_000);
  assert.equal(fallback.stats().sendDrainTimeoutMs, 30_000);
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

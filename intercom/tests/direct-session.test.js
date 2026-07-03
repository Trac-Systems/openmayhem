import assert from 'node:assert/strict';
import test from 'node:test';
import DirectSession from '../features/direct-session/index.js';

const sessionId = 'ab'.repeat(32);
const remote = 'cd'.repeat(32);

test('DirectSession exposes raised mx/s rate limits without relay semantics', () => {
  const directSession = new DirectSession({}, {});
  const stats = directSession.stats();

  assert.equal(stats.protocol, 'mx/s');
  assert.equal(stats.rateBytesPerSecond, 1_000_000);
  assert.equal(stats.rateBurstBytes, 1_000_000);
  assert.equal(stats.sessionCount, 0);
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
  const directSession = new DirectSession(
    {},
    {
      onFrame: (event) => frames.push(event),
    }
  );
  const frame = { t: 's.delta', d: 'token'.repeat(8) };
  const frameBytes = directSession._frameBytes(frame);
  directSession.rateBytesPerSecond = 1;
  directSession.rateBurstBytes = frameBytes + 1;
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

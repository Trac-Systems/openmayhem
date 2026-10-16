import assert from 'node:assert/strict';
import test from 'node:test';
import { registerHooks } from 'node:module';
import b4a from 'b4a';
import Sidechannel from '../features/sidechannel/index.js';

const remote = 'ab'.repeat(32);
const connection = () => ({ remotePublicKey: b4a.from(remote, 'hex') });

test('receive throttle diagnostics preserve refill, strikes and block expiry without changing state on reads', () => {
  let now = 10_000;
  const sidechannel = new Sidechannel({}, {
    rateBytesPerSecond: 100, rateBurstBytes: 100,
    maxStrikes: 3, strikeWindowMs: 5000, blockMs: 30_000,
  });
  sidechannel._now = () => now;
  const conn = connection();
  assert.equal(sidechannel.receiveThrottleStats(conn).dropped_messages, 0);
  assert.equal(sidechannel.rateLimits.size, 0, 'stats must not allocate a limiter');
  assert.equal(sidechannel._checkRate(conn, 100), true);
  const state = sidechannel.rateLimits.get(conn);
  const before = structuredClone(state);
  now += 500;
  sidechannel.receiveThrottleStats(conn);
  assert.deepEqual(state, before, 'stats must not refill, reset strikes or modify counters');
  assert.equal(sidechannel._checkRate(conn, 50), true);
  assert.equal(state.tokens, 0);
  for (let strike = 1; strike <= 3; strike += 1) {
    assert.equal(sidechannel._checkRate(conn, 1), false);
    assert.equal(state.strikes, strike);
    assert.equal(sidechannel._isBlocked(conn), strike === 3);
  }
  assert.equal(sidechannel._checkRate(conn, 10), false);
  assert.deepEqual(sidechannel.receiveThrottleStats(conn), {
    enabled: true, rate_bytes_per_second: 100, burst_bytes: 100,
    allowed_messages: 2, allowed_bytes: 150,
    dropped_messages: 4, dropped_bytes: 13,
    unmeasured_dropped_messages: 0, blocked_dropped_messages: 1,
    blocks: 1, strikes: 3, blocked_remaining_ms: 30_000,
    last_drop_at_ms: 10_500, last_drop_reason: 'blocked',
  });
  const blockedState = structuredClone(state);
  now += 30_000;
  assert.equal(sidechannel.receiveThrottleStats(conn).blocked_remaining_ms, 0);
  assert.deepEqual(state, blockedState, 'expiry observation must not reset the bucket');
  assert.equal(sidechannel._isBlocked(conn), false);
  assert.equal(sidechannel._checkRate(conn, 100), true);
  assert.equal(state.strikes, 0);
  assert.equal(state.tokens, 0);
  assert.equal(sidechannel.receiveThrottleStats(conn).allowed_bytes, 250);
  assert.equal(sidechannel.receiveThrottleStats(conn).blocks, 1);
});

test('blocked receive callback counts decoder bytes without inspecting or dispatching content', async t => {
  const channelName = '0000mayhem-relay';
  let delivered = 0;
  let incoming;
  let encoding;
  const sidechannel = new Sidechannel({}, {
    channels: [channelName], rateBytesPerSecond: 100, rateBurstBytes: 100,
    maxStrikes: 1, onMessage: () => { delivered += 1; },
  });
  sidechannel._now = () => 1000;
  t.after(() => sidechannel.stop());
  const conn = connection();
  conn.userData = {
    pair() {},
    createChannel: () => ({
      opened: true, open() {}, close() {}, fullyOpened: async () => true,
      addMessage(options) {
        incoming = options.onmessage;
        encoding = options.encoding;
        return { send: () => true };
      },
    }),
  };
  sidechannel._openChannelForConnection(conn, sidechannel.channels.get(channelName));
  assert.equal(sidechannel._checkRate(conn, 101), false);
  const value = { control: 'fixture' };
  const state = { buffer: null, start: 0, end: 0 };
  encoding.preencode(state, value);
  state.buffer = b4a.alloc(state.end);
  encoding.encode(state, value);
  state.start = 0;
  incoming(encoding.decode(state));
  incoming({ toJSON() { throw new Error('blocked content must not be serialized'); } });
  const stats = sidechannel.receiveThrottleStats(conn);
  assert.equal(delivered, 0);
  assert.equal(stats.dropped_messages, 3);
  assert.equal(stats.blocked_dropped_messages, 2);
  assert.equal(stats.dropped_bytes, 101 + b4a.byteLength(JSON.stringify(value)));
  assert.equal(stats.unmeasured_dropped_messages, 1);
  assert.equal(stats.strikes, 1, 'blocked arrivals do not add strikes');
  assert.equal(stats.blocks, 1, 'blocked arrivals do not extend or restart the block');
});

test('receive counters saturate and are reclaimed with each connection', async () => {
  const sidechannel = new Sidechannel({}, { rateBytesPerSecond: 1, rateBurstBytes: 100 });
  sidechannel._now = () => 1000;
  const conn = connection();
  const state = sidechannel._getLimiter(conn);
  for (const field of [
    'allowedMessages', 'allowedBytes', 'droppedMessages', 'droppedBytes',
    'blockedDroppedMessages', 'blocks', 'unmeasuredDroppedMessages',
  ]) state.receiveStats[field] = Number.MAX_SAFE_INTEGER;
  assert.equal(sidechannel._checkRate(conn, 1), true);
  state.strikes = sidechannel.maxStrikes - 1;
  assert.equal(sidechannel._checkRate(conn, 100), false);
  assert.equal(sidechannel._checkRate(conn, 100), false);
  sidechannel._recordRateDrop(state, null, 'blocked');
  for (const [field, value] of Object.entries(state.receiveStats)) {
    if (!field.startsWith('last')) assert.equal(value, Number.MAX_SAFE_INTEGER, field);
  }
  sidechannel._dropConnection(conn);
  assert.equal(sidechannel.rateLimits.size, 0);
  assert.equal(sidechannel.receiveThrottleStats(conn).last_drop_at_ms, null);
  for (let index = 0; index < 1000; index += 1) {
    const next = connection();
    sidechannel._checkRate(next, 101);
    sidechannel._dropConnection(next);
  }
  assert.equal(sidechannel.rateLimits.size, 0);
  sidechannel._checkRate(connection(), 101);
  await sidechannel.stop();
  assert.equal(sidechannel.rateLimits.size, 0);
  const disabled = new Sidechannel({}, { rateBytesPerSecond: 0 });
  assert.equal(disabled._checkRate(connection(), Number.MAX_SAFE_INTEGER), true);
  assert.equal(disabled.rateLimits.size, 0);
  assert.equal(disabled.receiveThrottleStats(connection()).enabled, false);
});

test('authenticated bridge stats expose only detached receive metadata for live connections', async () => {
  const hooks = registerHooks({
    resolve(specifier, context, nextResolve) {
      return specifier === 'bare-ws'
        ? { url: 'data:text/javascript,export default { Server: class {} };', shortCircuit: true }
        : nextResolve(specifier, context);
    },
  });
  let ScBridge;
  try {
    ScBridge = (await import('../features/sc-bridge/index.js?receive-stats-test')).default;
  } finally {
    hooks.deregister();
  }
  const sidechannel = new Sidechannel({}, { rateBytesPerSecond: 100, rateBurstBytes: 100 });
  const conn = connection();
  sidechannel.connections.set(conn, new Map([
    ['0000mayhem-relay', { message: {}, channel: { opened: true } }],
  ]));
  sidechannel._checkRate(conn, 101);
  const bridge = new ScBridge({}, { requireAuth: true });
  bridge.sidechannel = sidechannel;
  const replies = [];
  const client = { authed: false, ready: false };
  bridge._broadcastToClient = (_, payload) => replies.push(payload);
  bridge._handleClientMessage(client, { id: 1, type: 'stats' });
  assert.equal(replies.pop().type, 'error');
  client.authed = client.ready = true;
  const before = structuredClone(sidechannel.rateLimits.get(conn));
  bridge._handleClientMessage(client, { id: 2, type: 'stats' });
  const reply = replies.pop();
  assert.equal(reply.type, 'stats');
  assert.deepEqual(reply.sidechannelConnections, [{
    remote, openChannels: ['0000mayhem-relay'],
    receiveThrottle: sidechannel.receiveThrottleStats(conn),
  }]);
  assert.deepEqual(sidechannel.rateLimits.get(conn), before);
  reply.sidechannelConnections[0].receiveThrottle.dropped_messages = 900;
  assert.equal(sidechannel.receiveThrottleStats(conn).dropped_messages, 1);
  assert.equal('tokens' in reply.sidechannelConnections[0].receiveThrottle, false);
  sidechannel._dropConnection(conn);
  bridge._handleClientMessage(client, { id: 3, type: 'stats' });
  assert.deepEqual(replies.pop().sidechannelConnections, []);
});

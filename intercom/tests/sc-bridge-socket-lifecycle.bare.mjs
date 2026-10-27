// Real Bare/WebSocket integration; no peer, ledger, credentials, or network beyond loopback.
// Run: bare intercom/tests/sc-bridge-socket-lifecycle.bare.mjs
import ws from 'bare-ws';
const { default: ScBridge } = await import(Bare.argv[2] || '../features/sc-bridge/index.js');

const assert = (condition, message) => { if (!condition) throw new Error(message); };
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const until = async (condition, label, timeout = 3000) => {
  const deadline = Date.now() + timeout;
  while (!condition()) {
    if (Date.now() >= deadline) throw new Error(`Timed out: ${label}`);
    await pause(5);
  }
};
const hardStop = setTimeout(() => { console.error('Socket lifecycle test exceeded 30s'); Bare.exit(1); }, 30000);
const closedSessions = [];
const bridge = new ScBridge({ wallet: {} }, {
  host: '127.0.0.1', port: 0, token: 'local-test-only', maxClients: 4, authTimeoutMs: 200,
  maxOutboundBytesPerClient: 16 * 1024 * 1024,
});
bridge.attachSidechannel({ channels: new Map(), connections: new Map() });
bridge.attachDirectSession({ close(remote, sessionId) { closedSessions.push(sessionId); } });
bridge.start();
await until(() => bridge.server.listening, 'bridge listening');
const url = `ws://127.0.0.1:${bridge.server.address().port}`;
let sequence = 0;
const clients = [];
const connect = async (token = 'local-test-only') => {
  const before = new Set(bridge.clients);
  const socket = new ws.Socket(url);
  const received = [];
  const errors = [];
  socket.on('data', (data) => received.push(JSON.parse(data.toString())));
  socket.on('error', (error) => errors.push(error));
  const client = { socket, received, errors };
  clients.push(client);
  await until(() => received.some((value) => value.type === 'hello') || errors.length, 'WebSocket hello');
  assert(errors.length === 0, 'Unexpected connection error');
  client.server = [...bridge.clients].find((entry) => !before.has(entry));
  socket.write(JSON.stringify({ id: ++sequence, type: 'auth', token }));
  await until(() => received.some((value) => value.type === 'auth_ok' || value.type === 'error'), 'authentication result');
  return client;
};
const stats = async (client) => {
  const id = ++sequence;
  client.socket.write(JSON.stringify({ id, type: 'stats' }));
  await until(() => client.received.some((value) => value.id === id), 'control client response');
  const result = client.received.find((value) => value.id === id);
  assert(result.type === 'stats', 'Healthy client stopped receiving stats');
  return result;
};
const end = async (client) => {
  client.socket.end();
  await until(() => !bridge.clients.has(client.server), 'graceful client cleanup');
};

try {
  const control = await connect();
  control.server.directSessions.set('control', { remote: 'aa'.repeat(32), sessionId: 'control-session' });
  assert((await stats(control)).clientCount === 1, 'Initial client count');

  const bad = await connect('wrong-token');
  await until(() => !bridge.clients.has(bad.server), 'authentication rejection cleanup');
  assert(bridge.clients.size === 1, 'Rejected auth leaked a client');

  const held = [await connect(), await connect(), await connect()];
  const rejected = new ws.Socket(url);
  rejected.on('error', () => {});
  rejected.on('data', () => {});
  await until(() => rejected.destroying || rejected.destroyed, 'over-capacity connection rejected');
  assert(bridge.clients.size === 4, 'Connection limit changed');
  await end(held.pop());
  const replacement = await connect();
  await end(replacement);
  for (const client of held) await end(client);
  assert(bridge.clients.size === 1, 'Normal sessions did not release slots');

  let pressured = 0;
  for (let round = 0; round < 12; round++) {
    const slow = await connect();
    const server = slow.server;
    const owned = `disconnect-${round}`;
    server.directSessions.set(owned, { remote: 'bb'.repeat(32), sessionId: owned });
    // Pause the real receiving TCP socket, then fill its sender until Bare has
    // an outstanding write. These private fields are test instrumentation only.
    slow.socket._socket.pause();
    for (let frame = 0; frame < 8 && !server.socket._pendingWrite; frame++) {
      bridge._broadcastToClient(server, { type: 'test_payload', data: 'x'.repeat(1024 * 1024) });
      await pause(5);
    }
    assert(server.socket._pendingWrite, 'Test did not exercise backpressure');
    pressured++;
    assert(server.outboundBytes <= 16 * 1024 * 1024, 'Output queue exceeded bound');
    slow.socket._socket.destroy();
    await until(() => !bridge.clients.has(server), 'abrupt-close registration cleanup');
    assert(server.outboundBytes === 0 && server.outboundQueue.length === 0, 'Abandoned output retained');
    assert(server.directSessions.size === 0, 'Abandoned session retained');
    assert(closedSessions.filter((id) => id === owned).length === 1, 'Owned session not closed exactly once');
    assert(!closedSessions.includes('control-session'), 'Unrelated session was closed');
    assert((await stats(control)).clientCount === 1, 'Slot was not reusable after disconnect');
  }

  const halfClosed = await connect();
  halfClosed.socket._socket.end();
  await until(() => !bridge.clients.has(halfClosed.server), 'TCP half-close cleanup');
  assert((await stats(control)).clientCount === 1, 'Half-close affected another client');
  await end(control);
  assert(bridge.clients.size === 0, 'Clients retained after test');
  assert(closedSessions.filter((id) => id === 'control-session').length === 1, 'Control session cleanup');
  console.log(JSON.stringify({ ok: true, pressuredDisconnects: pressured, remainingClients: bridge.clients.size,
    checks: ['authentication rejection', 'capacity limit', 'graceful close', 'backpressured disconnect', 'slot reuse', 'session isolation', 'TCP half-close'] }));
} finally {
  for (const client of clients) client.socket.destroy();
  bridge.stop();
  clearTimeout(hardStop);
}
// PearRuntime.run keeps its supervisor IPC channel alive after the fixture has
// closed every bridge client. A successful standalone fixture may exit here.
if (Bare.IPC) Bare.exit(0);

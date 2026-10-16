import assert from 'node:assert/strict';
import test from 'node:test';
import b4a from 'b4a';
import PeerWallet from 'trac-wallet';
import Sidechannel from '../features/sidechannel/index.js';

const channel = '0000mayhem-relay';
const control = 'mayhem_service_request';
const config = {
  channels: [channel], welcomeRequired: false,
  powEnabled: true, powDifficulty: 1, powRequiredChannels: [channel],
  powExemptControlsByChannel: { [channel]: [control] },
};
const key = (peer) => b4a.toString(peer.wallet.publicKey, 'hex');
const makePeer = async () => {
  const wallet = new PeerWallet();
  await wallet.ready;
  await wallet.generateKeyPair();
  return { wallet };
};
const addReady = (sidechannel, remote, send) => {
  const connection = { remotePublicKey: b4a.from(remote, 'hex') };
  const record = { channel: { opened: true, drained: true }, message: { send } };
  sidechannel.connections.set(connection, new Map([[channel, record]]));
  return { connection, record };
};
const connect = (sender, receiver) => {
  let incoming;
  let encoding;
  const from = { remotePublicKey: sender.peer.wallet.publicKey };
  from.userData = { pair() {}, createChannel: () => ({
    opened: true, open() {}, close() {}, fullyOpened: async () => true,
    addMessage(options) {
      incoming = options.onmessage;
      encoding = options.encoding;
      return { send: () => true };
    },
  }) };
  receiver._openChannelForConnection(from, receiver.channels.get(channel));
  const frames = [];
  const outbound = addReady(sender, key(receiver.peer), (payload) => {
    frames.push(payload);
    const state = { start: 0, end: 0, buffer: null };
    encoding.preencode(state, payload);
    state.buffer = b4a.alloc(state.end);
    encoding.encode(state, payload);
    state.start = 0;
    incoming(encoding.decode(state));
    return true;
  });
  return { ...outbound, from, frames };
};

test('directed delivery avoids unrelated triangle traffic while preserving the default receive budget', async (t) => {
  const peers = await Promise.all([makePeer(), makePeer(), makePeer()]);
  const run = (directed) => {
    let now = 1000;
    const delivered = [[], [], []];
    const nodes = peers.map((peer, i) => new Sidechannel(peer, {
      ...config, onMessage: (_name, payload) => delivered[i].push(payload),
    }));
    for (const node of nodes) { node._now = () => now; t.after(() => node.stop()); }
    const [sender, recipient, observer] = nodes;
    const toRecipient = connect(sender, recipient);
    const toObserver = connect(sender, observer);
    const relayed = connect(recipient, observer);
    const send = (destination, message) => {
      const addressed = { ...message, value: { admin: key(destination.peer) } };
      if (directed) return sender.broadcast(channel, addressed);
      // Simulate the pre-routing sender with the unchanged legacy envelope and
      // broadcast path, including its one-hop relay behavior at the receiver.
      const payload = sender._buildPayload(channel, addressed);
      for (const perConn of sender.connections.values()) perConn.get(channel).message.send(payload);
      return true;
    };
    for (let n = 0; n < 4; n += 1) {
      assert.equal(send(recipient, { control, recipient: key(recipient.peer), n, data: 'x'.repeat(60_000) }), true);
    }
    for (let n = 0; n < 3; n += 1) {
      assert.equal(send(observer, { control, recipient: key(observer.peer), n, data: 'y'.repeat(20_000) }), true);
    }
    if (directed) {
      for (; now <= 8000; now += 50) {
        for (const node of nodes) for (const connection of node.connections.keys()) node._drainOutbound(connection);
      }
    }
    return { nodes, delivered, toRecipient, toObserver, relayed };
  };
  const broadcast = run(false);
  const oldObserverStats = broadcast.nodes[2].receiveThrottleStats(broadcast.toObserver.from);
  assert.equal(oldObserverStats.strikes, 3);
  assert.equal(oldObserverStats.blocks, 1);
  assert.equal(oldObserverStats.blocked_remaining_ms, 30_000);
  const directed = run(true);
  assert.equal(directed.toRecipient.frames.length, 4);
  assert.equal(directed.toObserver.frames.length, 3);
  assert.equal(directed.relayed.frames.length, 0, 'existing receiver relay path obeys TTL zero');
  assert.equal(directed.delivered[1].length, 4);
  assert.equal(directed.delivered[2].length, 3);
  for (const payload of [...directed.toRecipient.frames, ...directed.toObserver.frames]) {
    assert.equal(payload.ttl, 0);
    assert.equal(directed.nodes[0].verifyPayload(payload), true);
    assert.equal(directed.nodes[0]._payloadStructureValid(payload, channel), true);
  }
  const stats = directed.nodes[2].receiveThrottleStats(directed.toObserver.from);
  assert.equal(stats.rate_bytes_per_second, 64_000);
  assert.equal(stats.burst_bytes, 256_000);
  assert.equal(stats.dropped_messages, 0);
  // Further addressed traffic shares the paced connection, retaining the same
  // receiver limit without turning a legitimate sender burst into a block.
  for (let n = 0; n < 3; n += 1) {
    assert.equal(directed.nodes[0].sendTo(channel, key(peers[1]), {
      control, n, data: 'z'.repeat(20_000),
    }), true);
  }
  assert.equal(directed.nodes[1].receiveThrottleStats(directed.toRecipient.from).blocks, 0);
});

test('transparent routing is restricted to the five addressed Mayhem controls on their canonical channel', async (t) => {
  const [peer, remote, observer] = await Promise.all([makePeer(), makePeer(), makePeer()]);
  const controls = [control, 'mayhem_feature_result', 'mayhem_feature_result_ack',
    'mayhem_service_result', 'mayhem_service_result_ack'];
  const sender = new Sidechannel(peer, { ...config, powEnabled: false });
  t.after(() => sender.stop());
  const frames = [];
  const unrelated = [];
  const { record } = addReady(sender, key(remote), (payload) => { frames.push(payload); return true; });
  const other = addReady(sender, key(observer), (payload) => { unrelated.push(payload); return true; });
  for (const type of controls) {
    const message = type === control ? { control: type, value: { admin: key(remote) } }
      : { control: type, to: key(remote) };
    assert.equal(sender.broadcast(channel, message), true);
    assert.equal(frames.at(-1).ttl, 0);
    assert.equal(sender.broadcast(channel, { control: type, to: 'not-a-key', value: { admin: 'not-a-key' } }), false);
    assert.equal(sender.broadcast(channel, { control: type }), false);
  }
  assert.equal(frames.length, 5);
  assert.equal(unrelated.length, 0);
  record.channel.opened = false;
  assert.equal(sender.broadcast(channel, { control, value: { admin: key(remote) } }), false);
  assert.equal(unrelated.length, 0, 'a cold addressed channel must never use another peer');
  record.channel.opened = true;
  assert.equal(sender.broadcast(channel, { control, value: { admin: key(remote) }, data: 'x'.repeat(256_000) }), false);
  assert.equal(frames.length, 5);
  // Neither an unrelated app's `to` property nor unaddressed feature requests
  // acquire new routing semantics.
  assert.equal(sender.broadcast(channel, { control: 'mayhem_feature_request', to: key(remote) }), true);
  assert.equal(sender.broadcast(channel, { control: 'other', to: key(remote) }), true);
  assert.equal(unrelated.length, 2);
  assert.equal(unrelated[0].ttl, 1);
  const otherChannel = 'another-channel';
  sender._registerChannel(otherChannel);
  for (const perConn of sender.connections.values()) perConn.set(otherChannel, perConn.get(channel));
  assert.equal(sender.broadcast(otherChannel, { control, value: { admin: key(remote) } }), true);
  assert.equal(unrelated.length, 3);
  assert.equal(other.record.channel.opened, true);
});

test('sendTo selects one exact ready authenticated remote and never joins or falls back', async (t) => {
  const [peer, remote, unrelated] = await Promise.all([makePeer(), makePeer(), makePeer()]);
  const sender = new Sidechannel(peer, config);
  t.after(() => sender.stop());
  let sent = 0;
  addReady(sender, key(unrelated), () => assert.fail('unrelated peer received a directed frame'));
  const { connection, record } = addReady(sender, key(remote), () => { sent += 1; return true; });
  addReady(sender, key(remote), () => assert.fail('duplicate connection was also used'));
  assert.equal(sender.sendTo('unjoined', key(remote), { control }), false);
  assert.equal(sender.channels.has('unjoined'), false);
  assert.equal(sender.sendTo(channel, 'not-a-key', { control }), false);
  assert.equal(sender.sendTo(channel, 'ff'.repeat(32), { control }), false);
  assert.equal(sender.sendTo(channel, key(remote).toUpperCase(), { control }), true);
  assert.equal(sent, 1);
  // Remove the duplicate and verify each exact transport lifecycle fence.
  for (const candidate of sender.connections.keys()) {
    if (candidate !== connection && sender._getRemoteKey(candidate) === key(remote)) sender.connections.delete(candidate);
  }
  for (const field of ['closed', 'destroyed']) {
    connection[field] = true;
    assert.equal(sender.sendTo(channel, key(remote), { control }), false);
    connection[field] = false;
    record.channel[field] = true;
    assert.equal(sender.sendTo(channel, key(remote), { control }), false);
    record.channel[field] = false;
  }
  record.channel.opened = false;
  assert.equal(sender.sendTo(channel, key(remote), { control }), false);
  record.channel.opened = true;
  sender.closedConnections.add(connection);
  assert.equal(sender.sendTo(channel, key(remote), { control }), false);
  assert.equal(sent, 1);
});

test('sendTo retains invite and owner policy and rejects frames beyond the receive burst before writing', async (t) => {
  const [peer, remote] = await Promise.all([makePeer(), makePeer()]);
  const sender = new Sidechannel(peer, { ...config, inviteRequired: true });
  t.after(() => sender.stop());
  sender._now = () => 1000;
  let sent = 0;
  addReady(sender, key(remote), () => { sent += 1; return true; });
  assert.equal(sender.sendTo(channel, key(remote), { control }), false);
  sender._rememberInvite(channel, key(remote), 2000);
  assert.equal(sender.sendTo(channel, key(remote), { control }), true);
  sender.ownerWriteOnly = true;
  sender.ownerKeys.set(channel, key(remote));
  assert.equal(sender.sendTo(channel, key(remote), { control }), false);
  sender.ownerKeys.set(channel, key(peer));
  assert.equal(sender.sendTo(channel, key(remote), { control }), true);
  assert.throws(() => sender.sendTo(channel, key(remote), { control, data: 'x'.repeat(256_000) }), {
    code: 'SIDECHANNEL_SEND_TOO_LARGE',
  });
  assert.equal(sent, 2, 'oversized final envelope is never sent');
  sender.maxMessageBytesByChannel.set(channel, 1000);
  assert.throws(() => sender.sendTo(channel, key(remote), { control, data: 'x'.repeat(1000) }), {
    code: 'SIDECHANNEL_SEND_TOO_LARGE',
  });
  sender._now = () => 2001;
  assert.equal(sender.sendTo(channel, key(remote), { control }), false);
  assert.equal(sent, 2);
});

test('send(false) is accepted once and subsequent sends respect shared transport backpressure', async (t) => {
  const [peer, remote] = await Promise.all([makePeer(), makePeer()]);
  const sender = new Sidechannel(peer, config);
  t.after(() => sender.stop());
  let sent = 0;
  const { record } = addReady(sender, key(remote), () => {
    sent += 1;
    record.channel.drained = false;
    return false;
  });
  assert.equal(sender.sendTo(channel, key(remote), { control }), true);
  assert.equal(sent, 1);
  for (let n = 0; n < 1000; n += 1) assert.equal(sender.sendTo(channel, key(remote), { control }), false);
  assert.equal(sent, 1);
  assert.equal(sender.powQueueDepth, 0);
  record.channel.drained = true;
  assert.equal(sender.sendTo(channel, key(remote), { control }), true);
  assert.equal(sent, 2);
});

test('required PoW stays bounded and directed, and stale channels cannot receive queued work', async (t) => {
  const [peer, remote, unrelated] = await Promise.all([makePeer(), makePeer(), makePeer()]);
  const sender = new Sidechannel(peer, { ...config, maxPowQueue: 1 });
  t.after(() => sender.stop());
  const frames = [];
  const { connection } = addReady(sender, key(remote), (payload) => { frames.push(payload); return true; });
  addReady(sender, key(unrelated), () => assert.fail('PoW work was broadcast'));
  assert.equal(sender.sendTo(channel, key(remote), { control: 'generic' }), true);
  assert.equal(sender.sendTo(channel, key(remote), { control: 'generic' }), false);
  await sender.powQueue;
  assert.equal(sender.powQueueDepth, 0);
  assert.equal(frames.length, 1);
  assert.equal(frames[0].ttl, 0);
  assert.equal(sender._checkPow(frames[0], channel), true);
  assert.equal(sender.verifyPayload(frames[0]), true);
  assert.equal(sender.sendTo(channel, key(remote), { control: 'generic' }), true);
  sender.connections.delete(connection);
  await sender.powQueue;
  assert.equal(frames.length, 1);
  assert.equal(sender.powQueueDepth, 0);
});

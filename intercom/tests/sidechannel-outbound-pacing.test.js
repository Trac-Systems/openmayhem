import assert from 'node:assert/strict';
import test from 'node:test';
import b4a from 'b4a';
import PeerWallet from 'trac-wallet';
import Sidechannel from '../features/sidechannel/index.js';

const channel = '0000mayhem-relay';
const key = peer => b4a.toString(peer.wallet.publicKey, 'hex');
const peer = async () => {
  const wallet = new PeerWallet();
  await wallet.ready;
  await wallet.generateKeyPair();
  return { wallet };
};
const link = (sender, receiver, name = channel, existing = null) => {
  let incoming;
  let encoding;
  const from = existing?.from ?? { remotePublicKey: sender.peer.wallet.publicKey };
  from.userData = { pair() {}, createChannel: () => ({
    opened: true, open() {}, close() {}, fullyOpened: async () => true,
    addMessage(options) {
      incoming = options.onmessage;
      encoding = options.encoding;
      return { send: () => true };
    },
  }) };
  receiver._openChannelForConnection(from, receiver.channels.get(name));
  const connection = existing?.connection ?? { remotePublicKey: receiver.peer.wallet.publicKey };
  const frames = [];
  const record = { channel: { opened: true, drained: true }, message: { send(payload) {
    frames.push(payload);
    const state = { start: 0, end: 0, buffer: null };
    encoding.preencode(state, payload);
    state.buffer = b4a.alloc(state.end);
    encoding.encode(state, payload);
    state.start = 0;
    incoming(encoding.decode(state));
    return true;
  } } };
  if (!sender.connections.has(connection)) sender.connections.set(connection, new Map());
  sender.connections.get(connection).set(name, record);
  return { connection, from, frames, record };
};

test('default receive limiter admits mixed financial and native bursts on one paced connection', async t => {
  const delivered = [];
  const [admin, website] = await Promise.all([peer(), peer()]);
  const settings = { channels: [channel, 'native'], powEnabled: false, welcomeRequired: false };
  const sender = new Sidechannel(admin, settings);
  const receiver = new Sidechannel(website, { ...settings, onMessage: (name, payload) => delivered.push({ name, payload, at: now }) });
  let now = 1000;
  sender._now = receiver._now = () => now;
  t.after(async () => { await sender.stop(); await receiver.stop(); });
  const financial = link(sender, receiver);
  const native = link(sender, receiver, 'native', financial);
  for (let n = 0; n < 4; n += 1) assert.equal(sender.broadcast('native', { control: 'native', n, data: 'x'.repeat(60_000) }), true);
  for (let n = 0; n < 3; n += 1) assert.equal(sender.broadcast(channel, {
    control: 'mayhem_service_result', to: key(website), n, data: 'y'.repeat(20_000),
  }), true);
  for (; now <= 12_000; now += 50) sender._drainOutbound?.(financial.connection);
  const stats = receiver.receiveThrottleStats(financial.from);
  assert.equal(stats.blocks, 0, 'mixed burst must not block the shared admin connection');
  assert.equal(stats.dropped_messages, 0);
  assert.equal(financial.frames.length, 3);
  assert.equal(native.frames.length, 4);
  assert.equal(delivered.length, 7);
  assert.ok(delivered.filter(item => item.name === channel).every(item => item.at < 16_000));
});

test('legacy addressed frames stop at their recipient or relay only to the exact destination; native gossip remains', async t => {
  const peers = await Promise.all([peer(), peer(), peer(), peer()]);
  const nodes = peers.map(p => new Sidechannel(p, {
    channels: [channel], welcomeRequired: false, powEnabled: true, onMessage() {},
    powDifficulty: 1, powRequiredChannels: [channel],
  }));
  t.after(async () => { for (const node of nodes) await node.stop(); });
  const [legacy, admin, website, other] = nodes;
  const toAdmin = link(legacy, admin);
  const toWebsite = link(admin, website);
  const toOther = link(admin, other);
  const deliverLegacy = message => {
    const payload = legacy._buildPayload(channel, message);
    toAdmin.record.message.send(payload); // old peer's unpaced broadcast wire
  };
  for (const control of ['mayhem_service_request', 'mayhem_service_result', 'mayhem_service_result_ack',
    'mayhem_feature_result', 'mayhem_feature_result_ack']) {
    const before = toWebsite.frames.length;
    deliverLegacy({ control, value: { admin: key(admin.peer) }, to: key(admin.peer) });
    assert.equal(toWebsite.frames.length, before);
    assert.equal(toOther.frames.length, 0);
    deliverLegacy({ control, value: { admin: key(website.peer) }, to: key(website.peer) });
    assert.equal(toOther.frames.length, 0);
    assert.equal(toWebsite.frames.at(-1).message.control, control);
    assert.equal(toWebsite.frames.at(-1).ttl, 0);
    assert.equal(website.verifyPayload(toWebsite.frames.at(-1)), true);
  }
  const addressedCount = toWebsite.frames.length;
  deliverLegacy({ control: 'mayhem_service_result', to: 'invalid' });
  assert.equal(toWebsite.frames.length, addressedCount);
  deliverLegacy({ control: 'mayhem_feature_request', value: { op: 'native' } });
  assert.equal(toWebsite.frames.length, addressedCount + 1);
  assert.equal(toOther.frames.length, 1);
});

test('reserved priority capacity and byte fairness keep both service controls and native gossip moving', async t => {
  const [admin, website] = await Promise.all([peer(), peer()]);
  const sender = new Sidechannel(admin, { channels: [channel], welcomeRequired: false });
  const receiver = new Sidechannel(website, { channels: [channel], welcomeRequired: false, onMessage() {} });
  let now = 1000;
  sender._now = receiver._now = () => now;
  t.after(async () => { await sender.stop(); await receiver.stop(); });
  const wire = link(sender, receiver);
  assert.equal(sender.broadcast(channel, { control: 'mayhem_feature_request', n: -1, data: 'n'.repeat(60_000) }), true);
  let acceptedNative = 0;
  for (let n = 0; n < 20; n += 1) {
    if (sender.broadcast(channel, { control: 'mayhem_feature_request', n, data: 'n'.repeat(60_000) })) acceptedNative += 1;
  }
  assert.equal(acceptedNative, 4);
  for (let n = 0; n < 8; n += 1) assert.equal(sender.broadcast(channel, {
    control: 'mayhem_service_result', to: key(website), n, data: 's'.repeat(20_000),
  }), true);
  assert.ok(sender.outboundBytes <= 512_000);
  assert.ok(sender.outboundMessages <= 128);
  for (; now <= 10_000; now += 25) sender._drainOutbound(wire.connection);
  const controls = wire.frames.map(frame => frame.message.control);
  assert.equal(controls.filter(control => control === 'mayhem_service_result').length, 8);
  assert.equal(controls.filter(control => control === 'mayhem_feature_request').length, 5);
  assert.equal(controls[1], 'mayhem_service_result');
  assert.ok(controls.slice(1, 7).includes('mayhem_feature_request'), 'native gossip receives a turn before all service traffic drains');
  assert.equal(receiver.receiveThrottleStats(wire.from).blocks, 0);
  assert.equal(sender.outboundBytes, 0);
  assert.equal(sender.outboundMessages, 0);
});

test('large frames incur full envelope debt, queue snapshots stay immutable, and timers drain without manual pumping', async t => {
  const [admin, website] = await Promise.all([peer(), peer()]);
  const sender = new Sidechannel(admin, { channels: [channel], welcomeRequired: false });
  const receiver = new Sidechannel(website, { channels: [channel], welcomeRequired: false, onMessage() {} });
  t.after(async () => { await sender.stop(); await receiver.stop(); });
  t.mock.timers.enable({ apis: ['setTimeout', 'Date'], now: 1000 });
  const wire = link(sender, receiver);
  const message = { control: 'mayhem_service_result', to: key(website), data: 'x'.repeat(240_000) };
  assert.equal(sender.broadcast(channel, message), true);
  const queued = { ...message, data: 'kept' };
  assert.equal(sender.broadcast(channel, queued), true);
  queued.data = 'mutated';
  assert.equal(wire.frames.length, 1);
  t.mock.timers.tick(5000);
  assert.equal(wire.frames.length, 2);
  assert.equal(wire.frames[1].message.data, 'kept');
  assert.equal(receiver.receiveThrottleStats(wire.from).dropped_messages, 0);
  assert.equal(sender.outboundBytes, 0);
});

test('bounded queue age, transport backpressure, stale records, and connection teardown reclaim admitted work', async t => {
  const [admin, website] = await Promise.all([peer(), peer()]);
  const sender = new Sidechannel(admin, { channels: [channel], welcomeRequired: false });
  const receiver = new Sidechannel(website, { channels: [channel], welcomeRequired: false, onMessage() {} });
  let now = 1000;
  sender._now = receiver._now = () => now;
  t.after(async () => { await sender.stop(); await receiver.stop(); });
  const wire = link(sender, receiver);
  const message = { control: 'mayhem_service_result', to: key(website), data: 'x'.repeat(60_000) };
  assert.equal(sender.broadcast(channel, message), true);
  assert.equal(sender.broadcast(channel, message), true);
  assert.equal(wire.frames.length, 1);
  wire.record.channel.drained = false;
  assert.equal(sender.broadcast(channel, message), false, 'not accepted under transport backpressure');
  now += 5000;
  sender._drainOutbound(wire.connection);
  assert.equal(wire.frames.length, 1);
  wire.record.channel.drained = true;
  sender._drainOutbound(wire.connection);
  assert.equal(wire.frames.length, 2);
  assert.equal(sender.broadcast(channel, message), true);
  wire.record.channel.opened = false;
  now += 10_000;
  sender._drainOutbound(wire.connection);
  assert.equal(sender.outboundMessages, 0);
  assert.equal(wire.frames.length, 2, 'expired work never reaches the transport');
  wire.record.channel.opened = true;
  assert.equal(sender.broadcast(channel, message), true);
  assert.equal(sender.broadcast(channel, message), true);
  assert.ok(sender.outboundMessages > 0);
  sender._dropConnection(wire.connection);
  assert.equal(sender.outboundMessages, 0);
  assert.equal(sender.outboundBytes, 0);
  assert.equal(sender.outbound.has(wire.connection), false);
});

test('global admission stays bounded and reserved service work survives background saturation', async t => {
  const [admin, website] = await Promise.all([peer(), peer()]);
  const sender = new Sidechannel(admin, { channels: [channel], welcomeRequired: false });
  sender._now = () => 1000;
  t.after(() => sender.stop());
  const entry = sender.channels.get(channel);
  const connections = [];
  for (let n = 0; n < 24; n += 1) {
    const connection = { remotePublicKey: website.wallet.publicKey };
    const record = { channel: { opened: false }, message: { send() {} } };
    sender.connections.set(connection, new Map([[channel, record]]));
    connections.push({ connection, record });
    for (let k = 0; k < 5; k += 1) sender._queueOutbound(connection, record, entry,
      sender._buildPayload(channel, { control: 'mayhem_feature_request', data: 'x'.repeat(60_000) }), false);
  }
  assert.ok(sender.outboundBytes <= 4_096_000);
  assert.ok(sender.outboundMessages <= 1024);
  const reserved = connections.at(-1);
  assert.equal(sender._queueOutbound(reserved.connection, reserved.record, entry,
    sender._buildPayload(channel, { control: 'mayhem_service_result', to: key(website), data: 'priority' }), false), true);
  assert.ok(sender.outboundBytes <= 8_192_000);
  for (const { connection } of connections) sender._dropConnection(connection);
  assert.equal(sender.outboundBytes, 0);
  assert.equal(sender.outboundMessages, 0);
});

test('queue count is bounded and synchronous teardown during drain cannot resend or corrupt accounting', async t => {
  const [admin, website] = await Promise.all([peer(), peer()]);
  const sender = new Sidechannel(admin, { channels: [channel], welcomeRequired: false });
  const receiver = new Sidechannel(website, { channels: [channel], welcomeRequired: false, onMessage() {} });
  let now = 1000;
  sender._now = receiver._now = () => now;
  t.after(async () => { await sender.stop(); await receiver.stop(); });
  const wire = link(sender, receiver);
  const message = { control: 'mayhem_service_result', to: key(website) };
  assert.equal(sender.broadcast(channel, { ...message, data: 'x'.repeat(240_000) }), true);
  for (let n = 0; n < 128; n += 1) assert.equal(sender.broadcast(channel, { ...message, n }), true);
  assert.equal(sender.broadcast(channel, message), false);
  assert.equal(sender.outboundMessages, 128);
  let sends = 0;
  wire.record.message.send = () => {
    sends += 1;
    sender._dropConnection(wire.connection);
    return false;
  };
  now += 5000;
  sender._drainOutbound(wire.connection);
  assert.equal(sends, 1);
  assert.equal(sender.outboundMessages, 0);
  assert.equal(sender.outboundBytes, 0);
});

test('an immediate transport exception remains not accepted', async t => {
  const [admin, website] = await Promise.all([peer(), peer()]);
  const sender = new Sidechannel(admin, { channels: [channel], welcomeRequired: false });
  const receiver = new Sidechannel(website, { channels: [channel], welcomeRequired: false, onMessage() {} });
  t.after(async () => { await sender.stop(); await receiver.stop(); });
  const wire = link(sender, receiver);
  sender._reportEventError = () => {};
  wire.record.message.send = () => { throw new Error('fixture transport failure'); };
  assert.equal(sender.broadcast(channel, { control: 'mayhem_service_result', to: key(website) }), false);
  assert.equal(sender.outboundMessages, 0);
  assert.equal(sender.outboundBytes, 0);
});

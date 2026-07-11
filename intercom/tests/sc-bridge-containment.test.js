import test from 'node:test';
import assert from 'node:assert/strict';

import { dispatchContainedClientRequest } from '../features/sc-bridge/containment.js';
import {
  addBoundedSubscriptions,
  messageByteLength,
  writeBoundedClientPayload,
} from '../features/sc-bridge/bounded-client.js';

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

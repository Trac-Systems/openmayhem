import b4a from 'b4a';

const messageByteLength = (data) => {
  if (typeof data === 'string') return b4a.byteLength(data, 'utf8');
  if (b4a.isBuffer(data)) return data.byteLength;
  return b4a.byteLength(String(data), 'utf8');
};

const addBoundedSubscriptions = (target, values, maxSubscriptions) => {
  const additions = [];
  for (const value of values) {
    const normalized = String(value);
    if (!target.has(normalized)) additions.push(normalized);
  }
  if (target.size + additions.length > maxSubscriptions) return false;
  for (const value of additions) target.add(value);
  return true;
};

const sidechannelSubscriptionMatches = (subscriptions, channel) =>
  !(subscriptions instanceof Set) || subscriptions.has(channel);

const flushBoundedClient = (client, onDrop) => {
  if (!client || client.closed || client.writing || client.outboundQueue.length === 0) return;
  const entry = client.outboundQueue[0];
  client.writing = true;
  let completed = false;
  const done = (error = null) => {
    if (completed) return;
    completed = true;
    if (client.outboundQueue[0] === entry) client.outboundQueue.shift();
    client.outboundBytes = Math.max(0, client.outboundBytes - entry.bytes);
    client.writing = false;
    if (error) {
      onDrop(error?.message ?? String(error));
      return;
    }
    flushBoundedClient(client, onDrop);
  };
  try {
    if (client.socket.write.length >= 2) client.socket.write(entry.data, done);
    else {
      client.socket.write(entry.data);
      done();
    }
  } catch (error) {
    done(error);
  }
};

const writeBoundedClientPayload = (client, payload, limits, onDrop) => {
  if (!client || client.closed) return false;
  let data = null;
  try {
    data = JSON.stringify(payload);
  } catch (error) {
    onDrop(error?.message ?? String(error));
    return false;
  }
  const bytes = b4a.byteLength(data, 'utf8');
  if (bytes > limits.maxMessageBytes) {
    onDrop(`outbound message exceeds ${limits.maxMessageBytes} bytes`);
    return false;
  }
  if (
    client.outboundQueue.length >= limits.maxOutboundMessages ||
    client.outboundBytes + bytes > limits.maxOutboundBytes
  ) {
    onDrop('outbound backpressure queue limit reached');
    return false;
  }
  client.outboundQueue.push({ data, bytes });
  client.outboundBytes += bytes;
  flushBoundedClient(client, onDrop);
  return true;
};

export {
  addBoundedSubscriptions,
  flushBoundedClient,
  messageByteLength,
  sidechannelSubscriptionMatches,
  writeBoundedClientPayload,
};

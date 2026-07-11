import Feature from 'trac-peer/src/artifacts/feature.js';
import b4a from 'b4a';
import ws from 'bare-ws';
import { dispatchContainedClientRequest } from './containment.js';
import {
  addBoundedSubscriptions,
  messageByteLength,
  writeBoundedClientPayload,
} from './bounded-client.js';
import {
  isLocalPeer,
  keyHex,
  localPeerKey,
  loopbackSessionInfo,
  normalizePeerKey,
} from './loopback.js';

const DEFAULT_MAX_CLIENTS = 64;
const DEFAULT_MAX_MESSAGE_BYTES = 2 * 1024 * 1024;
const DEFAULT_MAX_SUBSCRIPTIONS_PER_CLIENT = 4096;
const DEFAULT_MAX_OUTBOUND_MESSAGES_PER_CLIENT = 4096;
const DEFAULT_MAX_OUTBOUND_BYTES_PER_CLIENT = 64 * 1024 * 1024;
const DEFAULT_AUTH_TIMEOUT_MS = 10_000;
const DEFAULT_MAX_CLI_QUEUE = 64;

const safePositiveInteger = (value, fallback) => (
  Number.isSafeInteger(value) && value > 0 ? value : fallback
);

const normalizeText = (value) => {
  if (value === null || value === undefined) return '';
  if (typeof value === 'string') return value;
  try {
    return JSON.stringify(value);
  } catch (_e) {
    return String(value);
  }
};

const parseJsonOrBase64 = (value) => {
  if (!value) return null;
  if (typeof value === 'object') return value;
  if (typeof value !== 'string') return null;
  const trimmed = value.trim();
  if (!trimmed) return null;
  if (trimmed.startsWith('{')) {
    try {
      return JSON.parse(trimmed);
    } catch (_e) {
      return null;
    }
  }
  try {
    const text = b4a.toString(b4a.from(trimmed, 'base64'), 'utf8');
    return JSON.parse(text);
  } catch (_e) {
    return null;
  }
};

const parseFilter = (raw) => {
  if (!raw) return [];
  return String(raw)
    .split('|')
    .map((group) =>
      group
        .trim()
        .split(/[+,\s]+/)
        .map((word) => word.trim())
        .filter(Boolean)
        .map((word) => word.toLowerCase())
    )
    .filter((group) => group.length > 0);
};

const matchesFilter = (filter, text) => {
  if (!filter || filter.length === 0) return true;
  const haystack = text.toLowerCase();
  return filter.some((group) => group.every((word) => haystack.includes(word)));
};

class ScBridge extends Feature {
  constructor(peer, config = {}) {
    super(peer, config);
    this.key = 'sc-bridge';
    this.sidechannel = null;
    this.directSession = null;
    this.server = null;
    this.started = false;
    this.clients = new Set();
    this.cliHandlers = null;
    this.cliQueue = Promise.resolve();
    this.cliQueued = 0;

    this.host = typeof config.host === 'string' ? config.host : '127.0.0.1';
    this.port = Number.isSafeInteger(config.port) ? config.port : 49222;
    this.token = typeof config.token === 'string' && config.token.length > 0 ? config.token : null;
    this.requireAuth = config.requireAuth !== false;
    this.cliEnabled = config.cliEnabled === true;
    this.debug = config.debug === true;
    this.maxClients = safePositiveInteger(config.maxClients, DEFAULT_MAX_CLIENTS);
    this.maxMessageBytes = safePositiveInteger(
      config.maxMessageBytes,
      DEFAULT_MAX_MESSAGE_BYTES
    );
    this.maxSubscriptionsPerClient = safePositiveInteger(
      config.maxSubscriptionsPerClient,
      DEFAULT_MAX_SUBSCRIPTIONS_PER_CLIENT
    );
    this.maxOutboundMessagesPerClient = safePositiveInteger(
      config.maxOutboundMessagesPerClient,
      DEFAULT_MAX_OUTBOUND_MESSAGES_PER_CLIENT
    );
    this.maxOutboundBytesPerClient = safePositiveInteger(
      config.maxOutboundBytesPerClient,
      DEFAULT_MAX_OUTBOUND_BYTES_PER_CLIENT
    );
    this.authTimeoutMs = safePositiveInteger(config.authTimeoutMs, DEFAULT_AUTH_TIMEOUT_MS);
    this.maxCliQueue = safePositiveInteger(config.maxCliQueue, DEFAULT_MAX_CLI_QUEUE);

    this.defaultFilterRaw = typeof config.filter === 'string' ? config.filter : '';
    this.defaultFilter = parseFilter(this.defaultFilterRaw);
    this.filterChannels = Array.isArray(config.filterChannels)
      ? new Set(config.filterChannels.map((c) => String(c)))
      : null;
    this.info = config.info && typeof config.info === 'object' ? config.info : null;
    this.nextClientId = 1;
  }

  attachSidechannel(sidechannel) {
    this.sidechannel = sidechannel;
  }

  attachDirectSession(directSession) {
    this.directSession = directSession;
  }

  _broadcastToClient(client, payload) {
    return writeBoundedClientPayload(
      client,
      payload,
      {
        maxMessageBytes: this.maxMessageBytes,
        maxOutboundMessages: this.maxOutboundMessagesPerClient,
        maxOutboundBytes: this.maxOutboundBytesPerClient,
      },
      (reason) => this._dropClient(client, reason, true)
    );
  }

  _dropClient(client, reason, destroySocket = false) {
    if (!client || client.closed) return;
    client.closed = true;
    if (client.authTimer) clearTimeout(client.authTimer);
    client.outboundQueue.length = 0;
    client.outboundBytes = 0;
    this.clients.delete(client);
    if (this.debug) {
      console.log(`[sc-bridge] client ${client.id} disconnected: ${reason}`);
    }
    if (destroySocket) {
      try {
        client.socket.destroy?.(new Error(reason));
      } catch (_e) {}
    }
  }

  _shouldEmit(client, channel, messageText) {
    if (client.sidechannelMuted === true) return false;
    if (client.channels && client.channels.size > 0 && !client.channels.has(channel)) {
      return false;
    }
    const filterApplies = this.filterChannels ? this.filterChannels.has(channel) : true;
    if (!filterApplies) return true;
    return matchesFilter(client.filter, messageText);
  }

  handleSidechannelMessage(channel, payload, _connection) {
    const messageText = normalizeText(payload?.message ?? payload);
    const event = {
      type: 'sidechannel_message',
      channel,
      id: payload?.id ?? null,
      from: payload?.from ?? null,
      origin: payload?.origin ?? null,
      // Message signatures are not secret; exposing them helps devs verify authenticity/debug drops.
      sig: payload?.sig ?? payload?.signature ?? null,
      relayedBy: payload?.relayedBy ?? null,
      ttl: payload?.ttl ?? null,
      ts: payload?.ts ?? Date.now(),
      message: payload?.message ?? payload,
    };
    if (this.debug) {
      console.log(`[sc-bridge] recv ${channel}:`, messageText);
    }
    if (this.debug) {
      console.log(`[sc-bridge] clients ${this.clients.size}`);
    }
    for (const client of this.clients) {
      if (!client.ready) continue;
      if (!this._shouldEmit(client, channel, messageText)) {
        if (this.debug) console.log('[sc-bridge] filtered');
        continue;
      }
      if (this.debug) console.log('[sc-bridge] emit');
      this._broadcastToClient(client, event);
    }
  }

  handleSessionFrame(event) {
    const payload = {
      type: 'session_frame',
      session_id: event?.session_id ?? null,
      channel: event?.channel ?? null,
      protocol: event?.protocol ?? null,
      remote: event?.remote ?? null,
      direct: event?.direct === true,
      relayed: event?.relayed === true,
      frame: event?.frame ?? null,
      ts: Date.now(),
    };
    if (this.debug) {
      console.log(
        `[sc-bridge] session_frame ${payload.frame?.t || 'frame'} ` +
          `${payload.session_id || ''} from ${payload.remote || ''}; clients ${this.clients.size}`
      );
    }
    for (const client of this.clients) {
      if (!client.ready) continue;
      if (
        !client.sessionAll &&
        client.sessionIds &&
        client.sessionIds.size > 0 &&
        !client.sessionIds.has(payload.session_id)
      ) {
        if (this.debug) {
          console.log(
            `[sc-bridge] skip client ${client.id} for session ${payload.session_id || ''}`
          );
        }
        continue;
      }
      if (this.debug) {
        console.log(`[sc-bridge] emit session_frame to client ${client.id}`);
      }
      this._broadcastToClient(client, payload);
    }
  }

  _localPeerKey() {
    return localPeerKey(this.peer, this.info);
  }

  _isLocalPeer(remote) {
    return isLocalPeer(remote, this.peer, this.info);
  }

  _loopbackSessionInfo(remote, sessionId, extra = {}) {
    return loopbackSessionInfo(remote, sessionId, extra);
  }

  _emitLoopbackSessionFrame(remote, sessionId, frame) {
    if (typeof this.directSession?._validateFrame === 'function') {
      this.directSession._validateFrame(frame);
    }
    const session = this._loopbackSessionInfo(remote, sessionId, { opened: true });
    this.handleSessionFrame({
      session_id: session.session_id,
      channel: session.channel,
      protocol: session.protocol,
      remote: session.remote,
      direct: true,
      relayed: false,
      frame,
    });
    return session;
  }

  _sendError(client, error) {
    this._broadcastToClient(client, { type: 'error', error });
  }

  _handleClientMessage(client, message) {
    if (!message || typeof message !== 'object') {
      this._sendError(client, 'Invalid message.');
      return;
    }
    const reqId = Number.isInteger(message.id) ? message.id : null;
    const reply = (payload) => {
      if (reqId !== null) {
        this._broadcastToClient(client, { id: reqId, ...payload });
      } else {
        this._broadcastToClient(client, payload);
      }
    };
    const sendError = (error) => reply({ type: 'error', error });

    if (message.type === 'auth') {
      if (!this.token) {
        sendError('Auth not enabled.');
        return;
      }
      if (message.token === this.token) {
        client.authed = true;
        client.ready = true;
        if (client.authTimer) {
          clearTimeout(client.authTimer);
          client.authTimer = null;
        }
        reply({ type: 'auth_ok' });
        return;
      }
      sendError('Unauthorized.');
      return;
    }

    if (this.requireAuth && !client.authed) {
      sendError('Unauthorized.');
      return;
    }

    switch (message.type) {
      case 'cli': {
        if (!this.cliEnabled) {
          sendError('CLI over WS is disabled.');
          return;
        }
        const command = typeof message.command === 'string' ? message.command.trim() : '';
        if (!command) {
          sendError('Missing command.');
          return;
        }
        this._enqueueCli(command)
          .then((result) => {
            reply({
              type: 'cli_result',
              command,
              ok: result.ok,
              output: result.output,
              error: result.error,
              result: result.result,
            });
          })
          .catch((err) => {
            reply({
              type: 'cli_result',
              command,
              ok: false,
              output: [],
              error: err?.message ?? String(err),
              result: null,
            });
          });
        return;
      }
      case 'ping':
        reply({ type: 'pong', ts: Date.now() });
        return;
      case 'set_filter': {
        const filter = String(message.filter || '');
        if (b4a.byteLength(filter, 'utf8') > this.maxMessageBytes) {
          sendError('Filter is too large.');
          return;
        }
        client.filter = parseFilter(filter);
        reply({ type: 'filter_set', filter });
        return;
      }
      case 'clear_filter': {
        client.filter = [];
        reply({ type: 'filter_set', filter: '' });
        return;
      }
      case 'subscribe': {
        const channels = Array.isArray(message.channels)
          ? message.channels
          : message.channel
            ? [message.channel]
            : [];
        if (!client.channels) client.channels = new Set();
        if (!this._addSubscriptions(client.channels, channels)) {
          sendError('Sidechannel subscription limit reached.');
          return;
        }
        client.sidechannelMuted = false;
        reply({ type: 'subscribed', channels: Array.from(client.channels) });
        return;
      }
      case 'unsubscribe': {
        const channels = Array.isArray(message.channels)
          ? message.channels
          : message.channel
            ? [message.channel]
            : [];
        if (!client.channels) client.channels = new Set();
        for (const ch of channels) client.channels.delete(String(ch));
        reply({ type: 'subscribed', channels: Array.from(client.channels) });
        return;
      }
      case 'session_subscribe': {
        const sessionIds = Array.isArray(message.session_ids)
          ? message.session_ids
          : message.session_id
            ? [message.session_id]
            : [];
        if (!client.sessionIds) client.sessionIds = new Set();
        if (sessionIds.some((sessionId) => String(sessionId) === '*')) {
          client.sessionAll = true;
        }
        if (!this._addSubscriptions(client.sessionIds, sessionIds)) {
          sendError('Session subscription limit reached.');
          return;
        }
        if (message.sidechannel === false) {
          client.sidechannelMuted = true;
        } else if (message.sidechannel === true) {
          client.sidechannelMuted = false;
        }
        if (this.debug) {
          console.log(
            `[sc-bridge] client ${client.id} session_subscribe ` +
              `${Array.from(client.sessionIds).join(',') || '(none)'} ` +
              `all=${client.sessionAll === true}`
          );
        }
        reply({
          type: 'session_subscribed',
          session_ids: Array.from(client.sessionIds),
          all_sessions: client.sessionAll === true,
        });
        return;
      }
      case 'session_unsubscribe': {
        const sessionIds = Array.isArray(message.session_ids)
          ? message.session_ids
          : message.session_id
            ? [message.session_id]
            : [];
        if (!client.sessionIds) client.sessionIds = new Set();
        for (const sessionId of sessionIds) {
          if (String(sessionId) === '*') client.sessionAll = false;
          client.sessionIds.delete(String(sessionId));
        }
        reply({
          type: 'session_subscribed',
          session_ids: Array.from(client.sessionIds),
          all_sessions: client.sessionAll === true,
        });
        return;
      }
      case 'peer_connect': {
        if (!this.directSession) {
          sendError('Direct session feature not ready.');
          return;
        }
        const remote = String(message.remote || '').trim();
        const waitMs = Number.isSafeInteger(message.wait_ms) ? message.wait_ms : 10_000;
        if (this.debug) {
          console.log(`[sc-bridge] client ${client.id} peer_connect ${remote}`);
        }
        if (this._isLocalPeer(remote)) {
          reply({
            type: 'peer_connected',
            remote: normalizePeerKey(remote),
            connected: true,
            direct: true,
            relayed: false,
            loopback: true,
          });
          return;
        }
        this.directSession
          .connectPeer(remote, waitMs)
          .then((peer) => reply({ type: 'peer_connected', ...peer }))
          .catch((err) => {
            sendError(err?.message ? `Peer connect failed: ${err.message}` : 'Peer connect failed.');
          });
        return;
      }
      case 'session_open': {
        if (!this.directSession) {
          sendError('Direct session feature not ready.');
          return;
        }
        const remote = String(message.remote || '').trim();
        const sessionId = String(message.session_id || '').trim();
        if (this.debug) {
          console.log(`[sc-bridge] client ${client.id} session_open ${sessionId} -> ${remote}`);
        }
        if (this._isLocalPeer(remote)) {
          try {
            reply({
              type: 'session_opened',
              ...this._loopbackSessionInfo(remote, sessionId, { opened: true }),
            });
          } catch (err) {
            sendError(err?.message ? `Session open failed: ${err.message}` : 'Session open failed.');
          }
          return;
        }
        this.directSession
          .open(remote, sessionId)
          .then((session) => reply({ type: 'session_opened', ...session }))
          .catch((err) => {
            sendError(err?.message ? `Session open failed: ${err.message}` : 'Session open failed.');
          });
        return;
      }
      case 'session_send': {
        if (!this.directSession) {
          sendError('Direct session feature not ready.');
          return;
        }
        const remote = String(message.remote || '').trim();
        const sessionId = String(message.session_id || '').trim();
        if (this.debug) {
          console.log(
            `[sc-bridge] client ${client.id} session_send ` +
              `${message.frame?.t || 'frame'} ${sessionId} -> ${remote}`
          );
        }
        if (this._isLocalPeer(remote)) {
          try {
            const session = this._emitLoopbackSessionFrame(remote, sessionId, message.frame);
            reply({ type: 'session_sent', ...session });
          } catch (err) {
            sendError(err?.message ? `Session send failed: ${err.message}` : 'Session send failed.');
          }
          return;
        }
        this.directSession
          .send(remote, sessionId, message.frame)
          .then((session) => reply({ type: 'session_sent', ...session }))
          .catch((err) => {
            sendError(err?.message ? `Session send failed: ${err.message}` : 'Session send failed.');
          });
        return;
      }
      case 'session_close': {
        if (!this.directSession) {
          sendError('Direct session feature not ready.');
          return;
        }
        try {
          if (this._isLocalPeer(message.remote)) {
            reply({
              type: 'session_closed',
              ...this._loopbackSessionInfo(message.remote, message.session_id, { closed: true }),
            });
            return;
          }
          const result = this.directSession.close(message.remote, message.session_id);
          reply({ type: 'session_closed', ...result });
        } catch (err) {
          sendError(err?.message ? `Session close failed: ${err.message}` : 'Session close failed.');
        }
        return;
      }
      case 'session_stats': {
        if (!this.directSession) {
          sendError('Direct session feature not ready.');
          return;
        }
        reply({ type: 'session_stats', ...this.directSession.stats() });
        return;
      }
      case 'send': {
        if (!this.sidechannel) {
          sendError('Sidechannel not ready.');
          return;
        }
        const channel = String(message.channel || '').trim();
        if (!channel) {
          sendError('Missing channel.');
          return;
        }
        const payload = message.message;
        const invite = parseJsonOrBase64(message.invite);
        const welcome = parseJsonOrBase64(message.welcome);
        if (message.invite && !invite) {
          sendError('Invalid invite (expected JSON or base64).');
          return;
        }
        if (message.welcome && !welcome) {
          sendError('Invalid welcome (expected JSON or base64).');
          return;
        }
        let invitePayload = invite;
        if (invitePayload && welcome && !invitePayload.welcome) {
          invitePayload = { ...invitePayload, welcome };
        }
        if (welcome && !invitePayload) {
          this.sidechannel.acceptInvite(channel, null, welcome);
        }
        const ok = this.sidechannel.broadcast(
          channel,
          payload,
          invitePayload ? { invite: invitePayload } : undefined
        );
        if (!ok) {
          sendError('Send denied (invite required or invalid).');
          return;
        }
        this.handleSidechannelMessage(channel, {
          message: payload,
          origin: 'local',
          ts: Date.now(),
        });
        reply({ type: 'sent', channel });
        return;
      }
      case 'join': {
        if (!this.sidechannel) {
          sendError('Sidechannel not ready.');
          return;
        }
        const channel = String(message.channel || '').trim();
        if (!channel) {
          sendError('Missing channel.');
          return;
        }
        const invite = parseJsonOrBase64(message.invite);
        const welcome = parseJsonOrBase64(message.welcome);
        if (message.invite && !invite) {
          sendError('Invalid invite (expected JSON or base64).');
          return;
        }
        if (message.welcome && !welcome) {
          sendError('Invalid welcome (expected JSON or base64).');
          return;
        }
        if (invite || welcome) {
          this.sidechannel.acceptInvite(channel, invite, welcome);
        }
        this.sidechannel
          .addChannel(channel)
          .then((ok) => {
            if (!ok) {
              sendError('Join denied (invite required or invalid).');
              return;
            }
            reply({ type: 'joined', channel });
          })
          .catch((err) => {
            sendError(err?.message ? `Join failed: ${err.message}` : 'Join failed.');
          });
        return;
      }
      case 'leave': {
        if (!this.sidechannel) {
          sendError('Sidechannel not ready.');
          return;
        }
        const channel = String(message.channel || '').trim();
        if (!channel) {
          sendError('Missing channel.');
          return;
        }
        this.sidechannel
          .removeChannel(channel)
          .then((ok) => {
            // Leaving a non-joined channel is treated as a no-op.
            if (client.channels) client.channels.delete(channel);
            reply({ type: 'left', channel, ok });
          })
          .catch((err) => {
            sendError(err?.message ? `Leave failed: ${err.message}` : 'Leave failed.');
          });
        return;
      }
      case 'open': {
        if (!this.sidechannel) {
          sendError('Sidechannel not ready.');
          return;
        }
        const channel = String(message.channel || '').trim();
        if (!channel) {
          sendError('Missing channel.');
          return;
        }
        const via = message.via ? String(message.via) : null;
        const invite = parseJsonOrBase64(message.invite);
        const welcome = parseJsonOrBase64(message.welcome);
        if (message.invite && !invite) {
          sendError('Invalid invite (expected JSON or base64).');
          return;
        }
        if (message.welcome && !welcome) {
          sendError('Invalid welcome (expected JSON or base64).');
          return;
        }
        const ok = this.sidechannel.requestOpen(channel, via, invite, welcome);
        if (!ok) {
          sendError('Open request denied (invalid input or missing invite/welcome).');
          return;
        }
        reply({ type: 'open_requested', channel, via: via || null });
        return;
      }
      case 'stats': {
        if (!this.sidechannel) {
          sendError('Sidechannel not ready.');
          return;
        }
        const channels = Array.from(this.sidechannel.channels.keys());
        const peers = Array.from(this.sidechannel.connections.keys())
          .map((connection) => keyHex(connection?.remotePublicKey))
          .filter(Boolean);
        const swarmPeers = Array.from(this.peer?.swarm?.connections || [])
          .map((connection) => keyHex(connection?.remotePublicKey))
          .filter(Boolean);
        const connectionCount = this.sidechannel.connections.size;
        reply({
          type: 'stats',
          channels,
          connectionCount,
          peers,
          swarmPeers,
          sidechannelStarted: this.sidechannel.started === true,
        });
        return;
      }
      case 'info': {
        if (!this.info) {
          sendError('Info not available.');
          return;
        }
        reply({ type: 'info', info: this.info });
        return;
      }
      default:
        sendError(`Unknown type: ${message.type}`);
    }
  }

  _handleSocketData(client, data) {
    const bytes = messageByteLength(data);
    if (bytes > this.maxMessageBytes) {
      this._dropClient(client, `inbound message exceeds ${this.maxMessageBytes} bytes`, true);
      return;
    }
    let text = '';
    if (typeof data === 'string') text = data;
    else if (b4a.isBuffer(data)) text = b4a.toString(data, 'utf8');
    else text = String(data);

    let msg = null;
    try {
      msg = JSON.parse(text);
    } catch (_e) {
      this._sendError(client, 'Invalid JSON.');
      return;
    }
    dispatchContainedClientRequest(() => this._handleClientMessage(client, msg), (error) => {
      console.error(
        `[sc-bridge] client ${client?.id ?? '?'} request failed without stopping the bridge:`,
        error?.message ?? error
      );
      this._sendError(client, error?.message ?? 'Request failed.');
    });
  }

  _formatLogArgs(args) {
    return args
      .map((value) => {
        if (typeof value === 'string') return value;
        try {
          return JSON.stringify(value);
        } catch (_e) {
          return String(value);
        }
      })
      .join(' ');
  }

  async _withConsoleCapture(fn) {
    const output = [];
    const original = {
      log: console.log,
      error: console.error,
      warn: console.warn,
    };
    console.log = (...args) => {
      output.push(this._formatLogArgs(args));
      original.log(...args);
    };
    console.error = (...args) => {
      output.push(this._formatLogArgs(args));
      original.error(...args);
    };
    console.warn = (...args) => {
      output.push(this._formatLogArgs(args));
      original.warn(...args);
    };
    try {
      const result = await fn();
      return { ok: true, output, result, error: null };
    } catch (err) {
      return { ok: false, output, result: null, error: err?.message ?? String(err) };
    } finally {
      console.log = original.log;
      console.error = original.error;
      console.warn = original.warn;
    }
  }

  _enqueueCli(command) {
    if (this.cliQueued >= this.maxCliQueue) {
      return Promise.reject(new Error('SC-Bridge CLI queue limit reached.'));
    }
    this.cliQueued += 1;
    const run = async () => this._withConsoleCapture(() => this._dispatchCli(command));
    const queued = this.cliQueue.then(run, run);
    this.cliQueue = queued.catch(() => null);
    return queued.finally(() => {
      this.cliQueued = Math.max(0, this.cliQueued - 1);
    });
  }

  _addSubscriptions(target, values) {
    return addBoundedSubscriptions(target, values, this.maxSubscriptionsPerClient);
  }

  async _ensureCliHandlers() {
    if (!this.cliHandlers) {
      const { TerminalHandlers } = await import('trac-peer/src/terminal/handlers.js');
      this.cliHandlers = new TerminalHandlers(this.peer);
    }
    return this.cliHandlers;
  }

  async _dispatchCli(input) {
    const cliHandlers = await this._ensureCliHandlers();
    const handlers = [
      { rule: (line) => line === '/stats', handler: (line) => cliHandlers.verifyDag(line) },
      { rule: (line) => line === '/help', handler: () => this._printHelpToConsole() },
      { rule: (line) => line === '/exit', handler: () => cliHandlers.exit({}) },
      { rule: (line) => line === '/get_keys', handler: () => cliHandlers.getKeys() },
      { rule: (line) => line.startsWith('/tx'), handler: (line) => cliHandlers.tx(line) },
      { rule: (line) => line.startsWith('/add_indexer'), handler: (line) => cliHandlers.addIndexer(line) },
      { rule: (line) => line.startsWith('/add_writer'), handler: (line) => cliHandlers.addWriter(line) },
      { rule: (line) => line.startsWith('/remove_writer'), handler: (line) => cliHandlers.removeWriter(line) },
      { rule: (line) => line.startsWith('/remove_indexer'), handler: (line) => cliHandlers.removeIndexer(line) },
      { rule: (line) => line.startsWith('/add_admin'), handler: (line) => cliHandlers.addAdmin(line) },
      { rule: (line) => line.startsWith('/update_admin'), handler: (line) => cliHandlers.updateAdmin(line) },
      { rule: (line) => line.startsWith('/enable_transactions'), handler: (line) => cliHandlers.enableTransactions(line) },
      { rule: (line) => line.startsWith('/set_auto_add_writers'), handler: (line) => cliHandlers.setAutoAddWriters(line) },
      { rule: (line) => line.startsWith('/set_chat_status'), handler: (line) => cliHandlers.setChatStatus(line) },
      { rule: (line) => line.startsWith('/post'), handler: (line) => cliHandlers.postMessage(line) },
      { rule: (line) => line.startsWith('/set_nick'), handler: (line) => cliHandlers.setNick(line) },
      { rule: (line) => line.startsWith('/mute_status'), handler: (line) => cliHandlers.muteStatus(line) },
      { rule: (line) => line.startsWith('/pin_message'), handler: (line) => cliHandlers.pinMessage(line) },
      { rule: (line) => line.startsWith('/unpin_message'), handler: (line) => cliHandlers.unpinMessage(line) },
      { rule: (line) => line.startsWith('/set_mod'), handler: (line) => cliHandlers.setMod(line) },
      { rule: (line) => line.startsWith('/delete_message'), handler: (line) => cliHandlers.deleteMessage(line) },
      { rule: (line) => line.startsWith('/enable_whitelist'), handler: (line) => cliHandlers.enableWhitelist(line) },
      { rule: (line) => line.startsWith('/set_whitelist_status'), handler: (line) => cliHandlers.setWhitelistStatus(line) },
      { rule: (line) => line.startsWith('/deploy_subnet'), handler: (line) => cliHandlers.deploySubnet(line) },
      { rule: () => true, handler: (line) => this.peer?.protocol?.instance?.customCommand(line) },
    ];

    for (const { rule, handler } of handlers) {
      if (!rule(input)) continue;
      return handler(input);
    }
    return null;
  }

  _printHelpToConsole() {
    // Mirror Terminal.printHelp content without needing readline.
    console.log('Node started. Available commands:');
    console.log(' ');
    console.log('- Setup Commands:');
    console.log('- /add_admin | Works only once and only on the bootstrap node. Enter a peer public key (hex) to assign admin rights: \'/add_admin --address "<hex>"\'.');
    console.log('- /update_admin | Existing admins may transfer admin ownership. Enter "null" as address to waive admin rights for this peer entirely: \'/update_admin --address "<address>"\'.');
    console.log('- /add_indexer | Only admin. Enter a peer writer key to get included as indexer for this network: \'/add_indexer --key "<key>"\'.');
    console.log('- /add_writer | Only admin. Enter a peer writer key to get included as writer for this network: \'/add_writer --key "<key>"\'.');
    console.log('- /remove_writer | Only admin. Enter a peer writer key to get removed as writer or indexer for this network: \'/remove_writer --key "<key>"\'.');
    console.log('- /remove_indexer | Only admin. Alias of /remove_writer (removes indexer as well): \'/remove_indexer --key "<key>"\'.');
    console.log('- /set_auto_add_writers | Only admin. Allow any peer to join as writer automatically: \'/set_auto_add_writers --enabled 1\'');
    console.log('- /enable_transactions | Enable transactions.');
    console.log(' ');
    console.log('- Chat Commands:');
    console.log('- /set_chat_status | Only admin. Enable/disable the built-in chat system: \'/set_chat_status --enabled 1\'. The chat system is disabled by default.');
    console.log('- /post | Post a message: \'/post --message "Hello"\'. Chat must be enabled. Optionally use \'--reply_to <message id>\' to respond to a desired message.');
    console.log('- /set_nick | Change your nickname like this \'/set_nick --nick "Peter"\'. Chat must be enabled. Can be edited by admin and mods using the optional --user <address> flag.');
    console.log('- /mute_status | Only admin and mods. Mute or unmute a user by their address: \'/mute_status --user "<address>" --muted 1\'.');
    console.log('- /set_mod | Only admin. Set a user as mod: \'/set_mod --user "<address>" --mod 1\'.');
    console.log('- /delete_message | Delete a message: \'/delete_message --id 1\'. Chat must be enabled.');
    console.log('- /pin_message | Set the pin status of a message: \'/pin_message --id 1 --pin 1\'. Chat must be enabled.');
    console.log('- /unpin_message | Unpin a message by its pin id: \'/unpin_message --pin_id 1\'. Chat must be enabled.');
    console.log('- /enable_whitelist | Only admin. Enable/disable chat whitelists: \'/enable_whitelist --enabled 1\'.');
    console.log('- /set_whitelist_status | Only admin. Add/remove users to/from the chat whitelist: \'/set_whitelist_status --user "<address>" --status 1\'.');
    console.log(' ');
    console.log('- System Commands:');
    console.log('- /tx | Perform a contract transaction. The command flag contains contract commands (format is protocol dependent): \'/tx --command "<string>"\'. To simulate a tx, additionally use \'--sim 1\'.');
    console.log('- /deploy_subnet | Register this subnet in the MSB (required before TX settlement): \'/deploy_subnet\'.');
    console.log('- /stats | check system properties such as writer key, DAG, etc.');
    console.log('- /get_keys | prints your public and private keys. Be careful and never share your private key!');
    console.log('- /exit | Exit the program');
    console.log('- /help | This help text');
    if (this.peer?.protocol?.instance?.printOptions) {
      this.peer.protocol.instance.printOptions();
    }
  }

  start() {
    if (this.started) return;
    if (this.requireAuth && !this.token) {
      throw new Error('SC-Bridge requires --sc-bridge-token when auth is required.');
    }
    this.started = true;
    this.server = new ws.Server({ host: this.host, port: this.port }, (socket) => {
      if (this.clients.size >= this.maxClients) {
        try {
          socket.destroy?.(new Error('SC-Bridge client limit reached.'));
        } catch (_e) {}
        return;
      }
      const client = {
        id: this.nextClientId++,
        socket,
        ready: !this.requireAuth,
        authed: !this.requireAuth,
        filter: this.defaultFilter,
        channels: null,
        sessionIds: null,
        sessionAll: false,
        outboundQueue: [],
        outboundBytes: 0,
        writing: false,
        closed: false,
        authTimer: null,
      };
      this.clients.add(client);
      if (this.requireAuth) {
        client.authTimer = setTimeout(() => {
          if (!client.authed) this._dropClient(client, 'authentication timed out', true);
        }, this.authTimeoutMs);
      }
      if (this.debug) {
        console.log(`[sc-bridge] client ${client.id} connected`);
      }

      const hello = {
        type: 'hello',
        peer: this.peer?.wallet?.publicKey ?? null,
        address: this.peer?.wallet?.address ?? null,
        entryChannel: this.sidechannel?.entryChannel ?? null,
        filter: this.defaultFilterRaw || '',
        requiresAuth: this.requireAuth,
      };
      this._broadcastToClient(client, hello);

      socket.on('data', (data) => this._handleSocketData(client, data));
      const cleanup = () => {
        this._dropClient(client, 'socket closed', false);
      };
      socket.on('close', cleanup);
      socket.on('end', cleanup);
      socket.on('error', cleanup);
    });
  }

  stop() {
    if (!this.server) return;
    try {
      this.server.close();
    } catch (_e) {}
    this.server = null;
    this.started = false;
    for (const client of this.clients) this._dropClient(client, 'bridge stopped', true);
    this.clients.clear();
  }
}

export default ScBridge;

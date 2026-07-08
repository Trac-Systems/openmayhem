import b4a from 'b4a';

export const SESSION_PROTOCOL = 'mx/s';
export const SESSION_CHANNEL_PREFIX = 'mx/s/';

export const keyHex = (value) => {
  if (!value) return null;
  if (b4a.isBuffer(value)) return b4a.toString(value, 'hex');
  if (typeof value === 'string') return value.trim().toLowerCase();
  if (typeof value === 'object' && value.type === 'Buffer' && Array.isArray(value.data)) {
    try {
      return b4a.toString(b4a.from(value.data), 'hex');
    } catch (_e) {
      return null;
    }
  }
  return String(value).trim().toLowerCase();
};

export const normalizePeerKey = (value) => {
  const key = keyHex(value);
  return key && /^[0-9a-f]{64}$/.test(key) ? key : null;
};

export const normalizeSessionId = (value) => {
  const sessionId = String(value || '').trim().toLowerCase();
  return /^[0-9a-f]{64}$/.test(sessionId) ? sessionId : null;
};

export const localPeerKey = (peer, info) => (
  normalizePeerKey(peer?.wallet?.publicKey) ||
  normalizePeerKey(info?.peerPubkey) ||
  normalizePeerKey(info?.peer_pubkey) ||
  normalizePeerKey(info?.peer)
);

export const isLocalPeer = (remote, peer, info) => {
  const local = localPeerKey(peer, info);
  const normalizedRemote = normalizePeerKey(remote);
  return Boolean(local && normalizedRemote && local === normalizedRemote);
};

export const loopbackSessionInfo = (remote, sessionId, extra = {}) => {
  const normalizedRemote = normalizePeerKey(remote);
  const normalizedSessionId = normalizeSessionId(sessionId);
  if (!normalizedRemote) throw new Error('Invalid remote peer key.');
  if (!normalizedSessionId) throw new Error('Invalid session_id.');
  return {
    session_id: normalizedSessionId,
    channel: `${SESSION_CHANNEL_PREFIX}${normalizedSessionId}`,
    protocol: SESSION_PROTOCOL,
    remote: normalizedRemote,
    direct: true,
    relayed: false,
    loopback: true,
    ...extra,
  };
};

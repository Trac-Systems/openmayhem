export const sessionOwnershipKey = (remote, sessionId) => (
  `${String(remote || '').trim().toLowerCase()}:${String(sessionId || '').trim().toLowerCase()}`
);

export const canOwnSession = (sessions, remote, sessionId, maxSessions) => {
  const key = sessionOwnershipKey(remote, sessionId);
  return sessions.has(key) || sessions.size < maxSessions;
};

export const ownSession = (sessions, remote, sessionId) => {
  sessions.set(sessionOwnershipKey(remote, sessionId), {
    remote: String(remote || '').trim(),
    sessionId: String(sessionId || '').trim(),
  });
};

export const disownSession = (sessions, remote, sessionId) => {
  sessions.delete(sessionOwnershipKey(remote, sessionId));
};

export const closeOwnedSessions = (sessions, close) => {
  for (const session of sessions.values()) {
    try {
      close(session.remote, session.sessionId);
    } catch (_e) {}
  }
  sessions.clear();
};

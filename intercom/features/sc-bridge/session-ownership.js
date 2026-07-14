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

export const sessionSubscriptionMatches = (sessionAll, sessionIds, sessionId) => (
  sessionAll === true || (sessionIds instanceof Set && sessionIds.has(sessionId))
);

export const sessionFrameRecipients = (clients, sessionId, excludedClient = null) => (
  Array.from(clients).filter((client) => (
    client !== excludedClient &&
    client.ready === true &&
    sessionSubscriptionMatches(client.sessionAll, client.sessionIds, sessionId)
  ))
);

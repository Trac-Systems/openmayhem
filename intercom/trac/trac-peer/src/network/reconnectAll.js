// Safety net for the Hypercore replication stall (the install-time patch in scripts/hypercore-stallfix/ is the
// actual fix). Called with the subnet stall watch's events: once a read-only follower has been stalled with a known
// signed lead for `afterMs`, close ALL connections of the swarm once (pinned peers included) and let discovery
// reconnect, which is what a restart does to the transport. At most once per `minGapMs`. Connections only: never
// roles, transactions, balances or state.
export function createReconnectAll({ swarm, discovery = null, label, afterMs = 600000, minGapMs = 600000,
  now = Date.now, log = console.log }) {
  let lastAt = null, count = 0, lastClosed = 0;
  const emit = (event, extra) => {
    try { log(`[trac-peer] ${JSON.stringify({ chain: label, event, ...extra })}`); } catch {}
  };
  const reconnect = event => {
    if (!Number.isSafeInteger(afterMs) || afterMs <= 0) return false;
    if (!swarm || swarm.destroyed || swarm.suspended || !swarm.connections) return false;
    const stalledMs = event?.stalledMs;
    if (!Number.isSafeInteger(stalledMs) || stalledMs < afterMs) return false;
    const at = now();
    if (lastAt !== null && at >= lastAt && at - lastAt < minGapMs) return false;
    let closed = 0;
    for (const connection of [...swarm.connections]) {
      if (connection.destroyed || connection.destroying) continue;
      try { connection.destroy(); closed++; } catch {}
    }
    lastAt = at;
    lastClosed = closed;
    count++;
    emit('reconnect_all', { closedConnections: closed, stalledMs, reconnects: count });
    if (discovery) Promise.resolve().then(() => discovery.refresh()).catch(() => {});
    return true;
  };
  reconnect.status = () => ({ enabled: Number.isSafeInteger(afterMs) && afterMs > 0, afterMs, reconnects: count,
    lastReconnectAt: lastAt, lastReconnectClosed: lastClosed });
  return reconnect;
}

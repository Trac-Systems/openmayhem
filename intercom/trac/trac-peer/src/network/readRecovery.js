import b4a from 'b4a';

// Repair the replication transport, not validator membership or protocol selection.
// Discovery can keep returning the same replicas (or remain in flight). A known
// peer can also exhaust the swarm's implicit retries without leaving its map.
export function createReadRecovery({ base, swarm, topic, discovery = swarm?.status?.(topic), label,
  rotate = false, repairMs = 30000, now = Date.now, log = console.log }) {
  let repairedAt = -Infinity, cursor = '', refreshing = null, retiring = null;
  const stopped = () => base.closing || base.closed || !swarm || swarm.destroyed || swarm.suspended;
  const connections = () => [...(swarm.connections ?? [])].filter(c => !c.destroyed && !c.destroying);
  const emit = (event, extra = {}) => {
    try { log(`[trac-peer] ${JSON.stringify({ chain: label, event,
    signedLength: base.view?.core?.signedLength ?? null, connected: connections().length,
    known: swarm.peers?.size ?? 0, ...extra })}`); } catch {}
  };
  const retry = excluded => {
    if (stopped()) return;
    const live = connections();
    // Explicit joins bypass the swarm's normal connection limits. Do not borrow
    // a join unless there is a free slot, including already connecting sockets.
    const limit = Math.min(swarm.maxPeers ?? Infinity, swarm.maxClientConnections ?? Infinity);
    if (live.length + (swarm.connecting ?? 0) >= limit) return;
    const connected = new Set(live.map(c => c.remotePublicKey && b4a.toString(c.remotePublicKey, 'hex')));
    const candidates = [...(swarm.peers?.values() ?? [])].filter(peer => {
      const key = peer.publicKey && b4a.toString(peer.publicKey, 'hex');
      return key && key !== excluded && !connected.has(key) && !peer.banned && !peer.queued && !peer.explicit &&
        (peer.proven || peer.topics?.some(t => b4a.equals(t, topic))) &&
        (!swarm.keyPair?.publicKey || !b4a.equals(peer.publicKey, swarm.keyPair.publicKey));
    });
    if (!candidates.length) return;
    candidates.sort((a, b) => b4a.compare(a.publicKey, b.publicKey));
    const peer = candidates.find(p => b4a.toString(p.publicKey, 'hex') > cursor) ?? candidates[0];
    cursor = b4a.toString(peer.publicKey, 'hex');
    // joinPeer re-arms exhausted retries through the public API. Release our
    // temporary pin immediately: the queued/current attempt remains, but we do
    // not accumulate permanent explicit peers or change existing owners' pins.
    try {
      swarm.joinPeer(peer.publicKey);
      emit('retry', { peer: cursor });
    } finally { swarm.leavePeer(peer.publicKey); }
  };
  const recover = error => {
    if (stopped() || retiring || now() - repairedAt < repairMs) return false;
    repairedAt = now();
    emit('stalled', { reason: error?.code ?? 'read unavailable', discoveryPending: refreshing !== null,
      coreKey: error?.coreKey ?? null, waitingSeq: error?.waitingSeq ?? null,
      blockReads: error?.blockReads ?? null, elapsedMs: error?.elapsedMs ?? null,
      corePeers: base.view?.core?.peers?.length ?? 0 });
    if (!refreshing && discovery) {
      refreshing = Promise.resolve().then(() => stopped() ? undefined : discovery.refresh()).then(ok => {
        if (ok === false && !stopped()) emit('discovery_failed');
      }).catch(error => { if (!stopped()) emit('discovery_failed', { error: error.message }); })
        .finally(() => { refreshing = null; });
    }
    const live = connections();
    const limit = Math.min(swarm.maxPeers ?? Infinity, swarm.maxClientConnections ?? Infinity);
    // Connection rotation is opt-in and only replaces one socket in a full set.
    const connection = rotate && live.length >= limit ? live[0] : null;
    if (connection) {
      retiring = connection;
      const excluded = connection.remotePublicKey && b4a.toString(connection.remotePublicKey, 'hex');
      connection.once('close', () => {
        retiring = null;
        // Run after all transport close handlers have freed their slots.
        Promise.resolve().then(() => retry(excluded)).catch(error => {
          if (!stopped()) emit('retry_failed', { error: error.message });
        });
      });
      try { connection.destroy(); } catch (error) { retiring = null; throw error; }
    } else retry();
    return true;
  };
  return event => {
    try { return recover(event); }
    catch (error) {
      emit('recovery_failed', { error: error?.message });
      return false;
    }
  };
}

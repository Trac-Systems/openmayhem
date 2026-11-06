import b4a from 'b4a';

// Transport-only recovery. Never modifies validator roles, signed lengths,
// transaction contents, or application state. All MSB consumers share this instance.
export function createReadRecovery({ base, swarm, topic, discovery, repairMs = 30000,
    retireMs = 120000, now = Date.now, log = console.log }) {
    let lastRepair = -Infinity, lastRetire = -Infinity, refreshing = null;
    let retired = null, closed = false, cursor = null, blocked = null;
    const stopped = () => closed || base.closing || base.closed || swarm.destroyed || swarm.suspended;
    const hex = key => key ? b4a.toString(key, 'hex') : null;
    const emit = (event, fields = {}) => {
        try { log(`[MSB] ${JSON.stringify({ event, signedLength: base.view?.core?.signedLength ?? null, ...fields })}`); } catch {}
    };
    const live = () => [...swarm.connections].filter(c => !c.destroyed && !c.destroying);
    const limit = () => Math.min(swarm.maxPeers ?? Infinity, swarm.maxClientConnections ?? Infinity);
    const candidate = connections => {
        const connected = new Set(connections.map(c => hex(c.remotePublicKey)));
        // Bounded scan; preserve position so a large peer cache is neither scanned
        // on each request nor permanently biased toward its first entries.
        if (!cursor) cursor = swarm.peers.values();
        for (let i = 0; i < Math.min(256, swarm.peers.size); i++) {
            let next = cursor.next();
            if (next.done) { cursor = swarm.peers.values(); next = cursor.next(); }
            const p = next.value;
            if (!p) break;
            if (p.publicKey && !connected.has(hex(p.publicKey)) && !p.banned && !p.queued && !p.explicit &&
                (p.proven || p.topics?.some(t => b4a.equals(t, topic))) &&
                (!swarm.keyPair?.publicKey || !b4a.equals(p.publicKey, swarm.keyPair.publicKey))) return p;
        }
        return null;
    };
    const retry = peer => {
        if (stopped() || !peer || peer.banned || peer.queued || peer.explicit) return;
        const connections = live();
        if (connections.length + (swarm.connecting ?? 0) >= limit() ||
            connections.some(c => hex(c.remotePublicKey) === hex(peer.publicKey))) return;
        try {
            swarm.joinPeer(peer.publicKey);
            emit('msb_replication_retry', { peer: hex(peer.publicKey) });
        } finally { swarm.leavePeer(peer.publicKey); }
    };
    const recover = () => {
        const time = now();
        if (stopped() || retired || time - lastRepair < repairMs) return false;
        lastRepair = time;
        const connections = live();
        const confirmed = base.view?.core?.signedLength ?? null;
        const failure = base.fastForwardLastFailure;
        // Only consider replacing a socket after repeated timeouts on the SAME
        // block, no confirmation progress and no data received on that core/peer.
        // A quiet node or a connection count alone is never a retirement reason.
        let stuckPeer = null;
        if (failure?.code === 'REQUEST_TIMEOUT' && Number.isSafeInteger(failure.index) &&
            time >= failure.at && time - failure.at <= retireMs && Array.isArray(failure.peers)) {
            const key = `${failure.coreKey}:${failure.index}`;
            if (!blocked || blocked.key !== key || blocked.confirmed !== confirmed) {
                blocked = { key, confirmed, peers: new Map() };
            }
            const seen = new Set();
            for (const p of failure.peers.slice(0, 4)) {
                if (!/^[a-f0-9]{64}$/.test(p.remoteKey ?? '') || !Number.isSafeInteger(p.dataRx)) continue;
                seen.add(p.remoteKey);
                let prior = blocked.peers.get(p.remoteKey);
                if (!prior || prior.dataRx !== p.dataRx) prior = { dataRx: p.dataRx, since: time };
                blocked.peers.set(p.remoteKey, prior);
                if (time - prior.since >= retireMs && !stuckPeer) stuckPeer = p.remoteKey;
            }
            for (const key of blocked.peers.keys()) if (!seen.has(key)) blocked.peers.delete(key);
        } else blocked = null;

        emit('msb_replication_recovery', { connected: connections.length, known: swarm.peers.size,
            discoveryPending: refreshing !== null, failedRead: failure && time - failure.at <= retireMs ? failure : null });
        if (!refreshing && discovery) {
            refreshing = Promise.resolve().then(() => stopped() ? undefined : discovery.refresh())
                .then(ok => { if (ok === false && !stopped()) emit('msb_discovery_failed'); })
                .catch(() => { if (!stopped()) emit('msb_discovery_failed'); })
                .finally(() => { refreshing = null; });
        }
        const replacement = candidate(connections);
        if (connections.length + (swarm.connecting ?? 0) < limit()) retry(replacement);
        else if (replacement && stuckPeer && connections.length > 1 && time - lastRetire >= retireMs) {
            const connection = connections.find(c => hex(c.remotePublicKey) === stuckPeer);
            if (connection) {
                const onClose = () => {
                    retired = null;
                    Promise.resolve().then(() => retry(replacement)).catch(() => {});
                };
                retired = { connection, onClose };
                lastRetire = time;
                blocked = null;
                connection.once('close', onClose);
                emit('msb_replication_replace_stalled', { peer: stuckPeer });
                try { connection.destroy(); }
                catch (error) { connection.removeListener('close', onClose); retired = null; throw error; }
            }
        }
        return true;
    };
    const handler = () => {
        try { return recover(); }
        catch { emit('msb_replication_recovery_failed'); return false; }
    };
    handler.close = () => {
        closed = true;
        if (retired) retired.connection.removeListener('close', retired.onClose);
        retired = null; blocked = null; cursor = null;
    };
    emit('msb_replication_recovery_ready', { repairMs, retireMs });
    return handler;
}

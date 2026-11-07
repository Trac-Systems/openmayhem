import b4a from 'b4a';

// Transport-only recovery. Never modifies validator roles, signed lengths,
// transaction contents, or application state. All MSB consumers share this instance.
export function createReadRecovery({ base, swarm, topic, discovery, repairMs = 30000,
    retireMs = 120000, now = Date.now, log = console.log }) {
    let lastRepair = -Infinity, lastRetire = -Infinity, refreshing = null;
    let retired = null, closed = false, cursor = null, blocked = null, demand = null;
    let observedConnections = new WeakMap();
    // Allow observation/scheduling jitter without treating an old error as an
    // indefinitely active failure. At least two distinct failures are required.
    const failureAgeMs = retireMs + 2 * repairMs;
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
    // A replacement consumes the slot just released. Explicit validator/pinned
    // connections can legitimately put Hyperswarm above its ordinary maxPeers.
    // Never require destroying all of that excess before replacing one reader.
    const retry = (peer, replacementCeiling = null) => {
        if (stopped() || !peer || peer.banned || peer.queued || peer.explicit) return;
        const connections = live();
        if (connections.length + (swarm.connecting ?? 0) >= (replacementCeiling ?? limit()) ||
            (swarm.connecting ?? 0) >= (swarm.maxParallel ?? Infinity) ||
            connections.some(c => hex(c.remotePublicKey) === hex(peer.publicKey))) return;
        try {
            swarm.joinPeer(peer.publicKey);
            emit('msb_replication_retry', { peer: hex(peer.publicKey) });
        } finally { swarm.leavePeer(peer.publicKey); }
    };
    const recover = event => {
        const time = now();
        if (stopped()) return false;
        const confirmed = base.view?.core?.signedLength ?? null;
        const localLength = base.view?.core?.length;
        const systemLength = base.core?.length;
        const systemSignedLength = base.core?.core?.state?.length;
        const requested = event?.code === 'MSB_CONFIRMATION_STALLED' && event.signedLength === confirmed &&
            Number.isSafeInteger(event.msbsl) && event.msbsl > confirmed ? event.msbsl : null;
        const workPending = Number.isSafeInteger(confirmed) && (
            (Number.isSafeInteger(localLength) && localLength > confirmed) ||
            (Number.isSafeInteger(systemLength) && Number.isSafeInteger(systemSignedLength) && systemSignedLength > systemLength) ||
            base.isFastForwarding?.() === true);
        // Observe locally: caller elapsed times cannot accelerate retirement.
        // One record serves every waiter and expires when demand stops arriving.
        if (demand && (demand.confirmed !== confirmed || time < demand.seenAt || time - demand.seenAt > failureAgeMs)) demand = null;
        if (Number.isSafeInteger(confirmed) && (requested !== null || workPending)) {
            if (!demand) demand = { confirmed, since: time, seenAt: time, target: requested, observations: 1 };
            else {
                if (time > demand.seenAt) demand.observations = 2;
                demand.seenAt = time;
                if (requested !== null) demand.target = Math.max(demand.target ?? requested, requested);
            }
        }
        if (retired || time - lastRepair < repairMs) return false;
        lastRepair = time;
        const connections = live();
        const failure = base.fastForwardLastFailure;
        for (const connection of connections) {
            if (!observedConnections.has(connection)) observedConnections.set(connection, time);
        }
        // Follow confirmed progress, not the requested tip: new blocks and a
        // different diagnostic peer sample do not mean a stuck reader recovered.
        // Receiving unrelated blocks is also not completion of the failed read.
        // Only a long-lived connection implicated in the current failed read is
        // eligible. Quiet/idle nodes and a lone old error cannot trigger this.
        let stuckPeer = null;
        const fresh = failure?.code === 'REQUEST_TIMEOUT' && Number.isSafeInteger(confirmed) &&
            Number.isSafeInteger(failure.index) && failure.index >= 0 &&
            /^[a-f0-9]{64}$/.test(failure.coreKey ?? '') && Number.isFinite(failure.at) &&
            time >= failure.at && time - failure.at <= failureAgeMs && Array.isArray(failure.peers);
        if (fresh) {
            if (!blocked || blocked.confirmed !== confirmed || failure.at < blocked.lastFailureAt ||
                failure.at - blocked.lastFailureAt > failureAgeMs) {
                blocked = { confirmed, since: time, lastFailureAt: failure.at, failures: 1 };
            } else if (failure.at > blocked.lastFailureAt) {
                blocked.lastFailureAt = failure.at;
                blocked.failures = 2;
            }
            if (blocked.failures >= 2 && time - blocked.since >= retireMs) {
                for (const p of failure.peers.slice(0, 4)) {
                    if (!/^[a-f0-9]{64}$/.test(p.remoteKey ?? '')) continue;
                    if (swarm.peers.get(p.remoteKey)?.explicit) continue;
                    const connection = connections.find(c => hex(c.remotePublicKey) === p.remoteKey);
                    if (connection && time - observedConnections.get(connection) >= retireMs) {
                        stuckPeer = p.remoteKey;
                        break;
                    }
                }
            }
        } else blocked = null;

        const confirmationStalled = demand?.observations === 2 && time - demand.since >= retireMs;
        if (!stuckPeer && confirmationStalled) {
            // Only MSB replication participants, never arbitrary/pinned sockets.
            // No failed-block record exists when head propagation or an in-flight
            // fast-forward stalls before a timed read can finish.
            const participating = new Set();
            for (const core of [base.core, base.view?.core]) {
                for (const p of core?.peers ?? []) {
                    const remote = hex(p.remotePublicKey);
                    if (remote) participating.add(remote);
                }
            }
            for (const c of connections) {
                const remote = hex(c.remotePublicKey);
                if (participating.has(remote) && !swarm.peers.get(remote)?.explicit &&
                    time - observedConnections.get(c) >= retireMs) { stuckPeer = remote; break; }
            }
        }

        emit('msb_replication_recovery', { connected: connections.length, known: swarm.peers.size,
            discoveryPending: refreshing !== null, failedRead: fresh ? failure : null,
            stalledMs: blocked ? time - blocked.since : 0, repeatedFailure: blocked?.failures === 2,
            confirmationTarget: demand?.target ?? null,
            confirmationStalledMs: demand ? time - demand.since : 0,
            systemLength: Number.isSafeInteger(systemLength) ? systemLength : null,
            systemSignedLength: Number.isSafeInteger(systemSignedLength) ? systemSignedLength : null,
            fastForwarding: base.isFastForwarding?.() === true });
        if (!refreshing && discovery) {
            refreshing = Promise.resolve().then(() => stopped() ? undefined : discovery.refresh())
                .then(ok => { if (ok === false && !stopped()) emit('msb_discovery_failed'); })
                .catch(() => { if (!stopped()) emit('msb_discovery_failed'); })
                .finally(() => { refreshing = null; });
        }
        const replacement = candidate(connections);
        if (connections.length + (swarm.connecting ?? 0) < limit()) retry(replacement);
        else if (replacement && stuckPeer && connections.length > 1 && time - lastRetire >= retireMs &&
            (swarm.connecting ?? 0) < (swarm.maxParallel ?? Infinity)) {
            const connection = connections.find(c => hex(c.remotePublicKey) === stuckPeer);
            if (connection) {
                const ceiling = connections.length + (swarm.connecting ?? 0);
                const onClose = () => {
                    retired = null;
                    Promise.resolve().then(() => retry(replacement, ceiling)).catch(() => {});
                };
                retired = { connection, onClose };
                lastRetire = time;
                blocked = null;
                connection.once('close', onClose);
                emit('msb_replication_replace_stalled', { peer: stuckPeer,
                    reason: confirmationStalled ? 'confirmation_demand' : 'failed_block', replacementCeiling: ceiling });
                try { connection.destroy(); }
                catch (error) { connection.removeListener('close', onClose); retired = null; throw error; }
            }
        }
        return true;
    };
    const handler = event => {
        try { return recover(event); }
        catch { emit('msb_replication_recovery_failed'); return false; }
    };
    handler.close = () => {
        closed = true;
        if (retired) retired.connection.removeListener('close', retired.onClose);
        retired = null; blocked = null; demand = null; cursor = null; observedConnections = new WeakMap();
    };
    emit('msb_replication_recovery_ready', { repairMs, retireMs });
    return handler;
}

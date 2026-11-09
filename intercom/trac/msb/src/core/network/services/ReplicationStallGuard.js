const DEFAULT_CHECK_INTERVAL = 15_000;
const DEFAULT_MIN_GAP = 600_000;

/**
 * Safety net for the Hypercore replication stall (block requests that a peer never answers; the install-time
 * patch in scripts/hypercore-stallfix/ is the actual fix). Signal: at every check for `timeout` ms this node
 * knew signed system data it had not applied (base.core.core.state.length > base.core.length) while its
 * confirmed length (base.view.core.signedLength) did not move. Then it closes ALL connections of the MSB swarm
 * once, pinned validator links included, and lets discovery reconnect: the transport reset a restart does,
 * without a restart. At most once per `minGap`, and only after a new full `timeout` window.
 *
 * Never runs on an indexer. It only closes connections: roles, transactions, balances and state are untouched.
 */
export class ReplicationStallGuard {
    #base;
    #getSwarm;
    #refresh;
    #timeout;
    #interval;
    #minGap;
    #now;
    #log;
    #timer = null;
    #mark = null;
    #stalledSince = null;
    #lastReconnectAt = null;
    #lastClosed = 0;
    #reconnects = 0;
    #last = null;

    /**
     * @param {object} base Autobase instance of MSB
     * @param {() => object|null} getSwarm returns the MSB Hyperswarm (or null)
     * @param {object} options
     */
    constructor(base, getSwarm, {
        timeout,
        interval = DEFAULT_CHECK_INTERVAL,
        minGap = DEFAULT_MIN_GAP,
        refresh = null,
        now = Date.now,
        log = console.log
    } = {}) {
        this.#base = base;
        this.#getSwarm = getSwarm;
        this.#refresh = refresh;
        this.#timeout = timeout;
        this.#interval = Number.isSafeInteger(timeout) && timeout > 0 ? Math.min(interval, timeout) : interval;
        this.#minGap = minGap;
        this.#now = now;
        this.#log = log;
    }

    get enabled() {
        return Number.isSafeInteger(this.#timeout) && this.#timeout > 0;
    }

    start() {
        if (!this.enabled || this.#timer !== null) return;
        this.#timer = setInterval(() => { this.check(); }, this.#interval);
        this.#timer.unref?.();
    }

    stop() {
        if (this.#timer !== null) clearInterval(this.#timer);
        this.#timer = null;
    }

    status() {
        const last = this.#last;
        let state = 'ok';
        if (this.#stalledSince !== null) state = 'stalled';
        else if (this.#mark !== null) state = 'behind';
        return {
            enabled: this.enabled,
            state,
            confirmedLength: last?.confirmed ?? null,
            systemLength: last?.systemLength ?? null,
            systemSignedLength: last?.systemSignedLength ?? null,
            behindSince: this.#mark?.since ?? null,
            stalledSince: this.#stalledSince,
            reconnects: this.#reconnects,
            lastReconnectAt: this.#lastReconnectAt,
            lastReconnectClosed: this.#lastClosed
        };
    }

    #emit(event, fields) {
        try { this.#log(`[MSB] ${JSON.stringify({ event, ...fields })}`); } catch {}
    }

    #sample() {
        const base = this.#base;
        const confirmed = base?.view?.core?.signedLength;
        const systemLength = base?.core?.length;
        const systemSignedLength = base?.core?.core?.state?.length;
        if (![confirmed, systemLength, systemSignedLength].every(Number.isSafeInteger)) return null;
        return { confirmed, systemLength, systemSignedLength };
    }

    /** @returns {boolean} true when the connections were closed */
    check() {
        const base = this.#base;
        if (!this.enabled || !base || base.closing || base.closed || base.isIndexer) {
            this.#mark = null;
            return false;
        }
        const s = this.#sample();
        if (s === null) return false;
        this.#last = s;

        const now = this.#now();
        const ahead = s.systemSignedLength > s.systemLength;
        const moved = this.#mark !== null && this.#mark.confirmed !== s.confirmed;

        if (this.#stalledSince !== null && (!ahead || moved)) {
            this.#emit('msb_replication_recovered', {
                confirmedLength: s.confirmed, systemLength: s.systemLength,
                systemSignedLength: s.systemSignedLength, stalledMs: now - this.#stalledSince
            });
            this.#stalledSince = null;
        }
        if (!ahead) {
            this.#mark = null;
            return false;
        }
        if (this.#mark === null || moved || now < this.#mark.since) {
            this.#mark = { confirmed: s.confirmed, since: now };
            return false;
        }

        const elapsed = now - this.#mark.since;
        if (elapsed < this.#timeout) return false;

        if (this.#stalledSince === null) {
            this.#stalledSince = this.#mark.since;
            this.#emit('msb_replication_stalled', {
                confirmedLength: s.confirmed, systemLength: s.systemLength,
                systemSignedLength: s.systemSignedLength, stalledMs: elapsed
            });
        }
        if (this.#lastReconnectAt !== null && now - this.#lastReconnectAt < this.#minGap) return false;

        const closed = this.#reconnectAll();
        this.#lastReconnectAt = now;
        this.#lastClosed = closed;
        this.#reconnects++;
        this.#emit('msb_replication_reconnect_all', {
            closedConnections: closed, confirmedLength: s.confirmed, systemLength: s.systemLength,
            systemSignedLength: s.systemSignedLength, stalledMs: now - this.#stalledSince, reconnects: this.#reconnects
        });
        // the next action needs a new full window
        this.#mark = { confirmed: s.confirmed, since: now };
        return true;
    }

    #reconnectAll() {
        const swarm = this.#getSwarm?.();
        if (!swarm || swarm.destroyed || swarm.suspended || !swarm.connections) return 0;
        let closed = 0;
        for (const connection of [...swarm.connections]) {
            if (connection.destroyed || connection.destroying) continue;
            try {
                connection.destroy();
                closed++;
            } catch {}
        }
        if (this.#refresh !== null) {
            try {
                Promise.resolve(this.#refresh()).catch(() => {});
            } catch {}
        }
        return closed;
    }
}

export default ReplicationStallGuard;

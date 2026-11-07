const DEFAULT_CHECK_INTERVAL = 15_000;

/**
 * Autobase starts a follower's fast-forward only when the network's signed length is at least
 * 16 entries past the follower's total length, unconfirmed entries included. After a writer is
 * removed in the middle of a batch, a follower that already holds unconfirmed entries can stay
 * above that mark: its confirmed length then stops for good, and a restart does not help.
 *
 * This watch forces one fast-forward when the confirmed length has not moved for `timeout` ms
 * while unconfirmed entries wait. It never runs on an indexer and does not change how entries are
 * applied.
 */
export class ConfirmationWatch {
    #base;
    #timeout;
    #interval;
    #now;
    #log;
    #timer = null;
    #mark = null;
    #running = false;

    constructor(base, { timeout, interval = DEFAULT_CHECK_INTERVAL, now = Date.now, log = console.log } = {}) {
        this.#base = base;
        this.#timeout = timeout;
        this.#interval = Math.min(interval, timeout);
        this.#now = now;
        this.#log = log;
    }

    get enabled() {
        return Number.isFinite(this.#timeout) && this.#timeout > 0;
    }

    start() {
        if (!this.enabled || this.#timer !== null) return;
        this.#timer = setInterval(() => { this.check().catch(() => {}) }, this.#interval);
        this.#timer.unref?.();
    }

    stop() {
        if (this.#timer !== null) clearInterval(this.#timer);
        this.#timer = null;
    }

    /** @returns {Promise<boolean>} true when a fast-forward was forced */
    async check() {
        const base = this.#base;
        if (this.#running || base.closing || base.isIndexer) return false;
        if (base.isFastForwarding?.()) {
            this.#mark = null;
            return false;
        }

        const confirmed = base.view.core.signedLength;
        const total = base.view.core.length;
        const now = this.#now();

        if (total <= confirmed) {
            this.#mark = null;
            return false;
        }
        if (this.#mark === null || this.#mark.confirmed !== confirmed) {
            this.#mark = { confirmed, since: now };
            return false;
        }
        if (now - this.#mark.since < this.#timeout) return false;

        this.#running = true;
        try {
            this.#log(`[MSB] Confirmed length stuck at ${confirmed} (local ${total}) for ${Math.round((now - this.#mark.since) / 1000)} s; forcing a fast-forward`);
            await base.forceFastForward();
            this.#log(`[MSB] Fast-forward finished: confirmed ${base.view.core.signedLength}, local ${base.view.core.length}`);
        } catch (error) {
            this.#log(`[MSB] Fast-forward failed: ${error?.message ?? error}`);
        } finally {
            this.#running = false;
            this.#mark = { confirmed: base.view.core.signedLength, since: this.#now() };
        }
        return true;
    }
}

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
    #stopped = false;
    #onStall;

    constructor(base, { timeout, interval = DEFAULT_CHECK_INTERVAL, now = Date.now, log = console.log, onStall = null } = {}) {
        this.#base = base;
        this.#timeout = timeout;
        this.#interval = Math.min(interval, timeout);
        this.#now = now;
        this.#log = log;
        this.#onStall = onStall;
    }

    get enabled() {
        return Number.isFinite(this.#timeout) && this.#timeout > 0;
    }

    start() {
        if (!this.enabled || this.#timer !== null) return;
        this.#stopped = false;
        this.#timer = setInterval(() => { this.check().catch(() => {}) }, this.#interval);
        this.#timer.unref?.();
    }

    stop() {
        if (this.#timer !== null) clearInterval(this.#timer);
        this.#timer = null;
        this.#stopped = true;
    }

    /** @returns {Promise<boolean>} true when a fast-forward was forced */
    async check() {
        const base = this.#base;
        if (!this.enabled || this.#stopped || base.closing || base.closed || base.isIndexer) return false;

        const confirmed = base.view.core.signedLength;
        const total = base.view.core.length;
        const now = this.#now();
        const forwarding = base.isFastForwarding?.() === true;
        const systemLength = base.core?.length;
        const systemSignedLength = base.core?.core?.state?.length;
        const headPending = Number.isSafeInteger(systemLength) && Number.isSafeInteger(systemSignedLength) &&
            systemSignedLength > systemLength;

        if (total <= confirmed && !forwarding && !headPending) {
            this.#mark = null;
            return false;
        }
        if (this.#mark === null || this.#mark.confirmed !== confirmed) {
            this.#mark = { confirmed, since: now };
            return false;
        }
        if (now - this.#mark.since < this.#timeout) return false;

        if (this.#running || forwarding) {
            // A hung fast-forward must not disable its own transport watchdog.
            // Do not start/cancel another apply operation: request only recovery.
            try { this.#onStall?.({ code: 'MSB_CONFIRMATION_STALLED', signedLength: confirmed,
                msbsl: total > confirmed ? total : undefined, stalledMs: now - this.#mark.since,
                localLength: total, systemLength, systemSignedLength, fastForwarding: forwarding })?.catch?.(() => {}); } catch {}
            return false;
        }

        this.#running = true;
        const stalledSince = this.#mark.since;
        const failedAt = base.fastForwardFailedAt;
        let failure = null;
        try {
            this.#log(`[MSB] Confirmed length stuck at ${confirmed} (local ${total}) for ${Math.round((now - this.#mark.since) / 1000)} s; forcing a fast-forward`);
            await base.forceFastForward();
        } catch (error) {
            failure = error;
        } finally {
            const advanced = base.view.core.signedLength > confirmed;
            const failed = failure !== null || base.fastForwardFailedAt > (failedAt ?? 0);
            if (!this.#stopped && !base.closing && !base.closed) {
                // A normally returned call may still have failed inside Autobase.
                // A scheduled fast-forward is not proof of signed progress either.
                const outcome = advanced ? 'advanced' : failed ? 'failed' : 'no_progress';
                const event = { event: 'msb_confirmation_recovery', outcome,
                    code: 'MSB_CONFIRMATION_STALLED', signedLength: base.view.core.signedLength,
                    previousSignedLength: confirmed, localLength: base.view.core.length,
                    msbsl: total > base.view.core.signedLength ? total : undefined,
                    stalledMs: Math.max(0, this.#now() - stalledSince) };
                try { this.#log(`[MSB] ${JSON.stringify(event)}`); } catch {}
                if (!advanced) {
                    try { this.#onStall?.(event)?.catch?.(() => {}); } catch {}
                }
            }
            this.#running = false;
            this.#mark = { confirmed: base.view.core.signedLength, since: this.#now() };
        }
        return true;
    }
}

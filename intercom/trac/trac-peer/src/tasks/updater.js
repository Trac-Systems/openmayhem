import Scheduler from '../utils/scheduler.js';

const PROCESS_INTERVAL_MS = 10_000
const ACK_OPERATION_TYPE = '_trac_peer_ack_v1'

// MAYHEM PATCH: ACKs must be ordinary signed operations. append(null) can crash
// under the Pear/Bare hypercore-storage encoder.
const createAckOperation = () => ({
    type: ACK_OPERATION_TYPE,
    value: { version: 1 }
})

class Updater {
    #base
    #scheduler
    #isInterrupted
    #processIntervalMs

    constructor({ base }, config = {}) {
        this.#base = base
        this.#processIntervalMs = Number(config?.updaterIntervalMs ?? PROCESS_INTERVAL_MS)
    }

    async start() {
        if (this.#scheduler?.isRunning) {
            console.info('Updater is already started');
            return;
        }

        if (!this.#scheduler) {
            this.#scheduler = this.#createScheduler();
        }
        this.#scheduler.start();
    }

    async #worker(next) {
        await this.#update();
        next(this.#processIntervalMs);
    }

    async #update() {
        if (!this.#shouldRun()) return

        if (this.#base.view.core.length > this.#base.view.core.signedLength)
            await this.#base.append(createAckOperation())
    }

    #createScheduler() {
        return new Scheduler((next) => this.#worker(next), this.#processIntervalMs);
    }

    #sleep(ms) {
        return new Promise(resolve => setTimeout(resolve, ms));
    }

    #shouldRun() {
        return this.#base.isIndexer && !this.#isInterrupted
    }

    async stop(waitForCurrent = true) {
        this.#isInterrupted = true;
        await this.#scheduler.stop(waitForCurrent);
        console.info('Updater: closing gracefully...');
    }
}

export { Updater, ACK_OPERATION_TYPE, createAckOperation }

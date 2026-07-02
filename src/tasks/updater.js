import Scheduler from '../utils/scheduler.js';

const PROCESS_INTERVAL_MS = 10_000

class Updater {
    #base
    #scheduler
    #isInterrupted

    constructor({ base }) {
        this.#base = base
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
        next(PROCESS_INTERVAL_MS);
    }

    async #update() {
        if (!this.#shouldRun()) return

        if (this.#base.view.core.length > this.#base.view.core.signedLength)
            await this.#base.append(null)
    }

    #createScheduler() {
        return new Scheduler((next) => this.#worker(next), PROCESS_INTERVAL_MS);
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

export { Updater }

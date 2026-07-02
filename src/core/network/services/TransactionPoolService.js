// PoolService.js
import { BATCH_SIZE, PROCESS_INTERVAL_MS } from '../../../utils/constants.js';
import Scheduler from '../../../utils/Scheduler.js';

class TransactionPoolService {
    #state;
    #address;
    #config;
    #tx_pool = [];
    #scheduler = null;

    /**
     * @param {State} state
     * @param {string} address
     * @param {object} config
     **/
    constructor(state, address, config) {
        this.#state = state;
        this.#address = address;
        this.#config = config;
    }

    get tx_pool() {
        return this.#tx_pool;
    }

    get state() {
        return this.#state;
    }

    get address() {
        return this.#address;
    }

    async start() {
        if (!this.#config.enableWallet) {
            console.info('TransactionPoolService can not start. Wallet is not enabled');
            return;
        }
        if (this.#scheduler && this.#scheduler.isRunning) {
            console.info('TransactionPoolService is already started');
            return;
        }

        this.#scheduler = this.#createScheduler();
        this.#scheduler.start();
    }

    async #worker(next) {
        try {
            await this.#processTransactions();
            if (this.#tx_pool.length > 0) {
                next(0);
            } else {
                next(PROCESS_INTERVAL_MS);
            }
        } catch (error) {
            throw new Error(`TransactionPoolService worker error: ${error.message}`);
        }
    }

    #createScheduler() {
        return new Scheduler((next) => this.#worker(next), PROCESS_INTERVAL_MS);
    }

    async #processTransactions() {
        const canValidate = await this.#checkValidationPermissions();
        if (canValidate && this.#tx_pool.length > 0) {
            const batch = this.#prepareBatch();
            await this.#state.append(batch);
        }
    }

    async #checkValidationPermissions() {
        const isAdminAllowedToValidate = await this.state.isAdminAllowedToValidate();
        const isNodeAllowedToValidate = await this.state.allowedToValidate(this.address);
        return isNodeAllowedToValidate || isAdminAllowedToValidate;
    }

    #prepareBatch() {
        const length = Math.min(this.tx_pool.length, BATCH_SIZE);
        const batch = this.tx_pool.slice(0, length);
        this.tx_pool.splice(0, length);
        return batch;
    }

    addTransaction(tx) {
        this.tx_pool.push(tx);
    }

    async stopPool(waitForCurrent = true) {
        if (!this.#scheduler) return;
        await this.#scheduler.stop(waitForCurrent);
        this.#scheduler = null;
        console.info('TransactionPoolService: closing gracefully...');
    }
}

export default TransactionPoolService;

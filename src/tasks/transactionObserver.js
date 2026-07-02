import b4a from 'b4a';
import { safeDecodeApplyOperation } from 'trac-msb/src/utils/protobuf/operationHelpers.js';
import Scheduler from '../utils/scheduler.js';

const PROCESS_INTERVAL_MS = 50

class TransactionObserver {
    #base
    #config
    #scheduler
    #msbClient
    #txPool
    #isInterrupted

    constructor({ base, msbClient, txPool }, config) {
        this.#base = base
        this.#msbClient = msbClient
        this.#txPool = txPool
        this.#config = config
        this.#scheduler = null
    }

    async start() {
        if (this.#scheduler?.isRunning) {
            console.info('TransactionObserver is already started');
            return;
        }

        if (!this.#scheduler) {
            this.#scheduler = this.#createScheduler();
        }
        this.#scheduler.start();
    }

    async #worker(next) {
        await this.#processTransactions();
        next(PROCESS_INTERVAL_MS);
    }

    async #processTransactions() {
        if (!this.#shouldRun()) return

        const ts = Math.floor(Date.now() / 1000)
        for (const tx of this.#txPool) {
            const entry = this.#txPool.get(tx);
            if (entry && ts - entry.ts > this.#config.maxTxDelay) {
                console.log('Dropping TX', tx)
                this.#txPool.delete(tx)
            } else {
                const msbsl = this.#msbClient.getSignedLength()
                const msbTx = await this.#msbClient.getSignedAtLength(tx, msbsl)
 
                if (b4a.isBuffer(msbTx?.value)) {
                    const decoded = safeDecodeApplyOperation(msbTx.value)
                    if (decoded?.type !== 12 || decoded?.txo === undefined) continue
                    if (decoded?.txo?.tx?.toString('hex') !== tx) continue
                    const invokerAddress = decoded?.address ? decoded.address.toString('ascii') : null
                    const validatorAddress = decoded?.txo?.va ? decoded.txo.va.toString('ascii') : null
                    const ipk = invokerAddress ? this.#msbClient.addressToPubKeyHex(invokerAddress) : null
                    const wp = validatorAddress ? this.#msbClient.addressToPubKeyHex(validatorAddress) : null
                    if (null === ipk || null === wp) continue
                    if (entry?.prepared === undefined) continue
                    const subnetTx = { msbsl, dispatch: entry.prepared.dispatch, ipk, wp }
                    this.#txPool.delete(tx);
                    await this.#base.append({ type: 'tx', key: tx, value: subnetTx })
                }
                await this.#sleep(5);
            }
        }
    }

    #createScheduler() {
        return new Scheduler((next) => this.#worker(next), PROCESS_INTERVAL_MS);
    }

    #sleep(ms) {
        return new Promise(resolve => setTimeout(resolve, ms));
    }

    #shouldRun() {
        return !this.#isInterrupted
    }

    async stop(waitForCurrent = true) {
        this.#isInterrupted = true;
        await this.#scheduler.stop(waitForCurrent);
        console.info('TransactionObserver: closing gracefully...');
    }
}

export { TransactionObserver }

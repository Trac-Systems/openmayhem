export default class TransactionPool {
    #config
    #pool
    constructor(config) {
        this.#config = config
        this.#pool = {}
    }

    *[Symbol.iterator]() {
        for (const tx in this.#pool) {
            yield tx
        }
    }

    get(tx) {
        return this.#pool[tx]
    }

    add(tx, content) {
        this.#pool[tx] = { tx, ts: Math.floor(Date.now() / 1000), prepared: content }
    }

    delete(tx) {
        delete this.#pool[tx]
    }

    contains(tx) {
        return !!this.#pool[tx]
    }

    size() {
        return Object.keys(this.#pool).length
    }

    isNotFull() {
        return this.size() <= this.#config.txPoolMaxSize
    }
}

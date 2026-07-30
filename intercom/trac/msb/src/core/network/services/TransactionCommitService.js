import {isHexString} from '../../../utils/helpers.js';
import {TRANSACTION_COMMIT_SERVICE_BUFFER_SIZE} from '../../../utils/constants.js';

const TX_HASH_HEX_STRING_LENGTH = 64;

export class PendingCommitInvalidTxHashError extends Error {
    constructor(txHash) {
        super(`Invalid txHash format: ${txHash}`);
    }
}

export class PendingCommitBufferFullError extends Error {
    constructor(limit) {
        super(`Maximum number of pending commits reached (limit=${limit}).`);
    }
}

export class PendingCommitAlreadyExistsError extends Error {
    constructor(txHash) {
        super(`Pending commit for txHash ${txHash} already exists.`);
    }
}

export class PendingCommitTimeoutError extends Error {
    constructor(txHash, timeoutMs) {
        super(`Pending commit for txHash ${txHash} timed out after ${timeoutMs} ms.`);
    }
}

export class PendingCommitCancelledError extends Error {
    constructor(txHash) {
        super(`Pending commit ${txHash} cancelled (shutdown).`);
    }
}

export class PendingCommitUnexpectedError extends Error {
    constructor(message = 'Unexpected commit error') {
        super(message);
    }
}

class TransactionCommitService {
    #pendingCommits;
    #config;

    constructor(config) {
        this.#pendingCommits = new Map(); // Map<txHash, pendingCommitEntry>
        this.#config = config;
    }

    #assertTxHash(txHash) {
        if (!isHexString(txHash) || txHash.length !== TX_HASH_HEX_STRING_LENGTH) {
            throw new PendingCommitInvalidTxHashError(txHash);
        }
    }

    has(txHash) {
        this.#assertTxHash(txHash);
        return this.#pendingCommits.has(txHash);
    }

    /*
        @returns {Promise}
    */
    registerPendingCommit(txHash) {
        this.#assertTxHash(txHash);

        if (this.#pendingCommits.size >= TRANSACTION_COMMIT_SERVICE_BUFFER_SIZE) {
            throw new PendingCommitBufferFullError(TRANSACTION_COMMIT_SERVICE_BUFFER_SIZE);
        }

        if (this.#pendingCommits.has(txHash)) {
            throw new PendingCommitAlreadyExistsError(txHash);
        }

        const timeoutMs = this.#config.txCommitTimeout;

        const entry = {
            txHash,
            timeoutMs,
            timeoutId: null,
            resolve: null,
            reject: null,
        };

        const promise = new Promise((resolve, reject) => {
            entry.resolve = resolve;
            entry.reject = reject;
        });

        entry.timeoutId = setTimeout(() => {
            this.rejectPendingCommit(
                txHash,
                new PendingCommitTimeoutError(txHash, timeoutMs)
            );
        }, timeoutMs);

        this.#pendingCommits.set(txHash, entry);
        return promise;
    }

    getAndDeletePendingCommit(txHash) {
        this.#assertTxHash(txHash);
        const entry = this.#pendingCommits.get(txHash);
        if (!entry) return null;

        clearTimeout(entry.timeoutId);
        this.#pendingCommits.delete(txHash);
        return entry;
    }

    resolvePendingCommit(txHash, receipt = null) {
        this.#assertTxHash(txHash);
        const entry = this.getAndDeletePendingCommit(txHash);
        if (!entry) return false;
        entry.resolve(receipt);
        return true;
    }

    rejectPendingCommit(txHash, error) {
        this.#assertTxHash(txHash);
        const entry = this.getAndDeletePendingCommit(txHash);
        if (!entry) return false;

        entry.reject(
            error instanceof Error
                ? error
                : new PendingCommitUnexpectedError('Unexpected commit error')
        );

        return true;
    }

    close() {
        for (const [txHash, entry] of this.#pendingCommits) {
            clearTimeout(entry.timeoutId);
            try {
                entry.reject(
                    new PendingCommitCancelledError(txHash)
                );
            } catch (error) {
                console.error(`TransactionCommitService.close: failed to reject pending commit ${txHash}:`, error);
            }
        }
        this.#pendingCommits.clear();
    }
}

export default TransactionCommitService;

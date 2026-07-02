import { sleep } from '../../../utils/helpers.js';
import { operationToPayload } from '../../../utils/applyOperations.js';
/**
 * MessageOrchestrator coordinates message submission, retry, and validator management.
 * It works with ConnectionManager and ledger state to ensure reliable message delivery.
 */
class MessageOrchestrator {
    #config;
    /**
     * Attempts to send a message to validators with retries and state checks.
     * @param {ConnectionManager} connectionManager - The connection manager instance
     * @param {object} state - The state to look for the message outcome
     * @param {object} config - Configuration options:
     */
    constructor(connectionManager, state, config) {
        this.connectionManager = connectionManager;
        this.state = state;
        this.#config = config;
    }

    /**
     * Sends a message to a single randomly selected connected validator.
     * @param {object} message - The message object to be sent
     * @returns {Promise<boolean>} - true if successful, false otherwise
     */
    async send(message) {
        const startTime = Date.now();
        while (Date.now() - startTime < this.#config.messageValidatorResponseTimeout) {
            const validator = this.connectionManager.pickRandomConnectedValidator();
            if (!validator) return false;

            const success = await this.#attemptSendMessage(validator, message);
            if (success) {
                return true;
            }
        }
        return false;
    }

    async #attemptSendMessage(validator, message) {
        let attempts = 0;
        const deductedTxType = operationToPayload(message.type);
        while (attempts <= this.#config.maxRetries) {
            this.connectionManager.sendSingleMessage(message, validator);

            const appeared = await this.waitForUnsignedState(message[deductedTxType].tx, this.#config.messageValidatorRetryDelay);
            if (appeared) {
                this.incrementSentCount(validator);
                if (this.shouldRemove(validator)) {
                    this.connectionManager.remove(validator);
                }
                return true;
            }
            attempts++;
        }

        // If all retries fail, remove validator from pool
        this.connectionManager.remove(validator);
        return false;
    }

    async waitForUnsignedState(txHash, timeout) {
        // Polls state for the transaction hash for up to timeout ms
        const start = Date.now();
        let entry = null;
        while (Date.now() - start < timeout) {
            await sleep(200);
            entry = await this.state.get(txHash)
            if (entry) return true;
        }
        return false;
    }

    incrementSentCount(validatorPubKey) {
        this.connectionManager.incrementSentCount(validatorPubKey);
    }

    shouldRemove(validatorPubKey) {
        return this.connectionManager.getSentCount(validatorPubKey) >= this.#config.messageThreshold;
    }
}

export default MessageOrchestrator;

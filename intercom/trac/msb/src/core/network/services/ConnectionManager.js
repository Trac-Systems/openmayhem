import b4a from 'b4a'
import PeerWallet from "trac-wallet"


// -- Debug Mode --
// TODO: Implement a better debug system in the future. This is just temporary.
const DEBUG = false;
const debugLog = (...args) => {
    if (DEBUG) {
        console.log('DEBUG [ConnectionManager] ==> ', ...args);
    }
};

class ConnectionManager {
    #validators
    #maxValidators
    #config

    // Note: #validators is using publicKey (Buffer) as key
    // As Buffers are objects, we will rely on internal conversions done by JS to compare them.
    // It would be better to handle these conversions manually by using hex strings as keys to avoid issues
    /**
     * @param {object} config
     **/
    constructor(config)  {
        this.#validators = new Map();
        this.#config = config
        this.#maxValidators = config.maxValidators
    }

    /**
     * Sends a message to a single randomly selected connected validator.
     * Returns the public key (buffer) of the validator used, or throws
     * if the specified validator is unavailable.
     * @param {Object} message - The message to send to the validator
     * @returns {String} - The public key of the validator used
     */
    send(message) {
        const connectedValidators = this.connectedValidators();

        if (connectedValidators.length === 0) {
            throw new Error('ConnectionManager: no connected validators available to send message');
        }

        const target = this.pickRandomValidator(connectedValidators);
        const entry = this.#validators.get(target);
        if (!entry || !entry.connection || !entry.connection.protocolSession?.has('legacy')) return null;

        try {
            entry.connection.protocolSession.send(message);
            entry.sent = (entry.sent || 0) + 1;
        } catch (e) {
            // Swallow individual send errors.
        }

        return target;
    }

    /**
     * Sends a message through a specific validator without increasing sent messages count.
     * @param {Object} message - The message to send to the validator
     * @param {String | Buffer} publicKey - A validator public key hex string to be fetched from the pool.
     * @returns {Boolean} True if the message was sent, false otherwise.
     */
    sendSingleMessage(message, publicKey) {
        let publicKeyHex = this.#toHexString(publicKey);
        if (!this.exists(publicKeyHex) || !this.connected(publicKeyHex)) return false; // Fail silently

        const validator = this.#validators.get(publicKeyHex);
        if (!validator || !validator.connection || !validator.connection.protocolSession) return false;
        try {
            validator.connection.protocolSession.send(message);
        } catch (e) {
            // Swallow individual send errors.
        }
        return true; // TODO: Implement better success/failure reporting
    }

    /**
     * Creates a blank entry for a validator in the pool without a connection.
     * @param {String | Buffer} publicKey - The public key hex string of the validator to whitelist
     */
    // TODO: Deprecated/Unused - remove if not needed
    whiteList(publicKey) {
        let publicKeyHex = this.#toHexString(publicKey);
        this.#validators.set(publicKeyHex, { connection: null, sent: 0 });
    }

    /**
     * Adds a validator to the pool if not already present.
     * @param {String | Buffer} publicKey - The public key hex string of the validator to add
     * @param {Object} connection - The connection object associated with the validator
     * @returns {Boolean} - Returns true if the validator was added or updated, false otherwise
     */
    addValidator(publicKey, connection) {
        let publicKeyHex = this.#toHexString(publicKey);
        if (this.maxConnectionsReached()) {
            debugLog(`addValidator: max connections reached.`);
            return false;
        }
        debugLog(`addValidator: adding validator ${this.#toAddress(publicKeyHex)}`);
        if (!this.exists(publicKeyHex)) {
            debugLog(`addValidator: appending validator ${this.#toAddress(publicKeyHex)}`);
            this.#append(publicKeyHex, connection);
            return true;
        } else if (!this.connected(publicKeyHex)) {
            debugLog(`addValidator: updating validator ${this.#toAddress(publicKeyHex)}`);
            this.#update(publicKeyHex, connection);
            return true;
        }
        debugLog(`addValidator: didn't add validator ${this.#toAddress(publicKeyHex)}`);
        return false; // TODO: Implement better success/failure reporting
    }

    /**
     * Removes a validator from the pool.
     * @param {String | Buffer} publicKey - The public key hex string of the validator to remove
     */
    remove(publicKey) {
        debugLog(`remove: removing validator ${this.#toAddress(publicKey)}`);
        const publicKeyHex = this.#toHexString(publicKey);
        if (this.exists(publicKeyHex)) {
            // Close the connection socket
            const entry = this.#validators.get(publicKeyHex);
            if (entry && entry.connection && typeof entry.connection.end === 'function') {
                try {
                    entry.connection.end();
                } catch (e) {
                    // Ignore errors on connection end
                    // TODO: Consider logging these errors here in verbose mode
                }
            }
            debugLog(`remove: removing validator from map: ${this.#toAddress(publicKeyHex)}. Map size before removal: ${this.#validators.size}.`);
            this.#validators.delete(publicKeyHex);
            debugLog(`remove: validator removed successfully. Map size is now ${this.#validators.size}.`);
        }
    }

    /**
     * Checks if the maximum number of connections has been reached.
     * @returns {Boolean} - Returns true if the maximum number of connections has been reached, false otherwise.
     */
    // Note: this function name is a bit misleading. It checks if we have reached max connections and returns boolean
    // The name leads to think it returns the number of max connections
    maxConnectionsReached() {
        return this.connectionCount() >= this.#maxValidators
    }

    /**
     * Gets a list of all currently connected validators' public keys.
     * @returns {Array} - An array of public key hex strings of connected validators
     */
    connectedValidators() {
        return Array.from(this.#validators.keys()).filter(pk => this.connected(pk));
    }

    /**
     * Gets the current number of connected validators.
     * @returns {Number} - The count of connected validators
     */
    connectionCount() {
        return this.connectedValidators().length;
    }

    /**
     * Checks if a validator is currently connected.
     * @param {String | Buffer} publicKey - The public key hex string of the validator to check
     * @returns {Boolean} - Returns true if the validator is connected, false otherwise
     */
    connected(publicKey) {
        const publicKeyHex = this.#toHexString(publicKey);
        return this.exists(publicKeyHex) && this.#validators.get(publicKeyHex).connection !== null;
    }

    /**
     * Checks if a validator exists in the pool.
     * @param {String | Buffer} publicKey - The public key hex string of the validator to check
     * @returns {Boolean} - Returns true if the validator exists, false otherwise
     */
    exists(publicKey) {
        const publicKeyHex = this.#toHexString(publicKey);
        return this.#validators.has(publicKeyHex);
    }

    /**
     * Gets the number of messages sent through a validator.
     * @param {String | Buffer} publicKey - The public key hex string of the validator
     * @returns {Number} - The count of messages sent
     */
    getSentCount(publicKey) {
        const publicKeyHex = this.#toHexString(publicKey);
        const entry = this.#validators.get(publicKeyHex);
        return entry ? (entry.sent || 0) : 0;
    }

    /**
     * Increments the count of messages sent through a validator.
     * @param {String | Buffer} publicKey - The public key hex string of the validator
     */
    incrementSentCount(publicKey) {
        const publicKeyHex = this.#toHexString(publicKey);
        const entry = this.#validators.get(publicKeyHex);
        if (entry) {
            entry.sent = (entry.sent || 0) + 1;
        }
    }

    prettyPrint() {
        console.log('Connection count: ', this.connectionCount())
        console.log('Validator map keys count: ', this.#validators.size)
        console.log('Validator map keys: ', Array.from(this.#validators.keys()).map(val => this.#toAddress(val)).join(', '))
    }

    // Note 1: This method shuffles the whole array (in practice, probably around 50 elements)
    //      just to fetch a small subset of it (most times, 1 element).
    //      There are more efficient ways to pick a small subset of validators. Consider optimizing.
    // Note 2: This method is unused now, but will be kept here for future reference
    // TODO: Deprecated/Unused - remove if not needed
    pickRandomSubset(validators, maxTargets) {
        const copy = validators.slice();
        const count = Math.min(maxTargets, copy.length);

        for (let i = copy.length - 1; i > 0; i--) {
            const j = Math.floor(Math.random() * (i + 1));
            [copy[i], copy[j]] = [copy[j], copy[i]];
        }

        return copy.slice(0, count);
    }

    /**
     * Picks a random validator from the given array of validator public keys.
     * @param {String[]} validatorPubKeys - An array of validator public key hex strings
     * @returns {String|null} - A randomly selected validator public key
     */
    pickRandomValidator(validatorPubKeys) {
        if (validatorPubKeys.length === 0) {
            return null;
        }
        const index = Math.floor(Math.random() * validatorPubKeys.length);
        return validatorPubKeys[index];
    }

    /**
     * Picks a random connected validator.
     * @returns {String|null} - A randomly selected connected validator public key, or null if none are connected
     */
    pickRandomConnectedValidator() {
        const connected = this.connectedValidators();
        if (connected.length === 0) return null;
        return this.pickRandomValidator(connected);
    }

    /**
     * Appends a new validator connection.
     * @param {String|Buffer} publicKey - The public key hex string of the validator
     * @param {Object} connection - The connection object
     */
    #append(publicKey, connection) {
        debugLog(`#append: appending validator ${this.#toAddress(publicKey)}`);
        const publicKeyHex = this.#toHexString(publicKey);
        if (this.#validators.has(publicKeyHex)) {
            // This should never happen, but just in case, we log it
            debugLog(`#append: tried to append existing validator: ${this.#toAddress(publicKey)}`);
            return;
        }
        this.#validators.set(publicKeyHex, { connection, sent: 0 });
        connection.on('close', () => {
            debugLog(`#append: connection closing for validator ${this.#toAddress(publicKey)}`);
            this.remove(publicKeyHex);
            debugLog(`#append: connection closed for validator ${this.#toAddress(publicKey)}`);
        });
    }

    /**
     * Updates an existing validator connection or adds it if not present.
     * @param {String|Buffer} publicKey - The public key hex string of the validator
     * @param {Object} connection - The connection object
     */
    #update(publicKey, connection) {
        // Note: Is there a good reason for the function 'update' to exist separately from 'append'?
        // It seems that both could be merged into a single function that either adds or updates the entry.
        // It would be preferable to keep them separated though, but we would need to review all usages to ensure correctness.
        // Also, we should remove the 'else' branch below if we decide to keep 'update' and 'append' separated.
        const publicKeyHex = this.#toHexString(publicKey);
        debugLog(`#update: updating validator ${this.#toAddress(publicKey)}`);
        if (this.#validators.has(publicKeyHex)) {
            this.#validators.get(publicKeyHex).connection = connection;
        } else {
            this.#validators.set(publicKeyHex, { connection, sent: 0 });
        }
    }

    #toAddress(publicKey) {
        const keyHex = b4a.isBuffer(publicKey) ? publicKey : b4a.from(publicKey, 'hex');
        return PeerWallet.encodeBech32m(
            this.#config.addressPrefix,
            keyHex
        );
    }

    #toHexString(publicKey) {
        return b4a.isBuffer(publicKey) ? publicKey.toString('hex') : publicKey;
    }
}

export default ConnectionManager;

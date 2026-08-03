import b4a from 'b4a'
import {EventType, ResultCode} from '../../../utils/constants.js';
import {publicKeyToAddress} from "../../../utils/helpers.js";
import {Logger} from "../../../utils/logger.js";
/**
 * @typedef {import('hyperswarm').Connection} Connection
 */

export class ConnectionManagerError extends Error {
    constructor(message) {
        super(message);
        this.name = this.constructor.name;
    }
}

class ConnectionManager {
    #validators
    #maxValidators
    #config
    #healthCheckService
    #boundedHealthCheckHandler
    #logger
    // Note: #validators is using publicKey (Buffer) as key
    // As Buffers are objects, we will rely on internal conversions done by JS to compare them.
    // It would be better to handle these conversions manually by using hex strings as keys to avoid issues
    /**
     * @param {Config} config
     **/
    constructor(config) {
        this.#validators = new Map();
        this.#config = config
        this.#maxValidators = config.maxValidators
        this.#boundedHealthCheckHandler = this.#healthCheckHandler.bind(this);
        this.#logger = new Logger(config)
    }

    /**
     * Subscribes to periodic validator health checks.
     * @param {ReadyResource} healthCheckService
     */
    // TODO: We should consider moving this to ValidatorObserver instead.
    // Keep here only if we forsee having health checks for non-validator connections in the future. 
    // For now, it seems that it would be better to keep this logic here.
    subscribeToHealthChecks(healthCheckService) {
        this.#logger.debug('subscribeToHealthChecks: subscribing to health check events');
        if (!healthCheckService || typeof healthCheckService.on !== 'function' || typeof healthCheckService.off !== 'function') {
            throw new Error('ConnectionManager: health check service must implement on/off');
        }

        if (this.#healthCheckService && this.#boundedHealthCheckHandler) {
            this.#logger.debug('subscribeToHealthChecks: removing previous health check handler');
            // Unsubscribe from previous health check service if already subscribed
            // TODO: Maybe we should not allow switching to a new health check service
            this.#healthCheckService.off(EventType.VALIDATOR_HEALTH_CHECK, this.#boundedHealthCheckHandler);
        }

        this.#healthCheckService = healthCheckService; // TODO: Maybe this should be handled in the constructor directly?
        // TODO: declare this method outside this function to avoid redeclaring it every time we subscribe to health checks. We can just bind it to 'this' in the constructor.

        this.#healthCheckService.on(EventType.VALIDATOR_HEALTH_CHECK, this.#boundedHealthCheckHandler);
        this.#logger.debug('subscribeToHealthChecks: subscribed to health check events');
    }

    async #healthCheckHandler(publicKey, requestId) {
        if (typeof publicKey !== 'string' || typeof requestId !== 'string') {
            // We can't throw here because this is an event handler, but we should at least log the error and return early to avoid further issues.
            this.#logger.error(`healthCheck: malformed event payload. Typeof publicKey = ${typeof publicKey}. Typeof requestId = ${typeof requestId}`);
            return;
        }

        const targetAddress = publicKeyToAddress(publicKey, this.#config);

        if (!this.exists(publicKey) || !this.connected(publicKey)) {
            this.#logger.debug(`healthCheck: validator not connected, stopping checks. Address = ${targetAddress}; Request ID = ${requestId}`);
            this.#stopHealthCheck(publicKey);
            return;
        }

        const connection = this.getConnection(publicKey);
        if (!connection || !connection.protocolSession || typeof connection.protocolSession.sendHealthCheck !== 'function') {
            this.#logger.debug(`healthCheck: missing protocol session, removing validator. Address = ${targetAddress}; Request ID = ${requestId}`);
            this.#stopHealthCheck(publicKey);
            this.remove(publicKey);
            return;
        }

        let success = false;
        try {
            this.#logger.debug(`healthCheck: sending liveness request. Address = ${targetAddress}; Request ID = ${requestId}`);

            const resultCode = await connection.protocolSession.sendHealthCheck();
            success = resultCode === ResultCode.OK;
            if (!success) {
                this.#logger.debug(`healthCheck: non-OK result code. Address = ${targetAddress}; Request ID = ${requestId}`);
            }
        } catch {
            success = false;
        }

        if (!success) {
            this.#logger.debug(`healthCheck: liveness request failed, removing validator. Address = ${targetAddress}; Request ID = ${requestId}`);
            this.remove(publicKey);
            this.#stopHealthCheck(publicKey);
        } else {
            this.#logger.debug(`healthCheck: success. Address = ${targetAddress}; Request ID = ${requestId}`);
        }
    };

    #stopHealthCheck(publicKeyHex) {
        const targetAddress = publicKeyToAddress(publicKeyHex, this.#config);

        if (!this.#healthCheckService) {
            this.#logger.debug(`stopHealthCheck: no health check service, cannot stop checks for ${targetAddress}`);
            return;
        }
        try {
            if (this.#healthCheckService.has(publicKeyHex)) {
                this.#logger.debug(`stopHealthCheck: stopping scheduled checks for ${targetAddress}`);
                this.#healthCheckService.stop(publicKeyHex);
            }
        } catch (error) {
            this.#logger.debug(`stopHealthCheck: failed to stop health check for validator ${targetAddress}. Error: ${error.message}`);
        }
    }

    /**
     * Retrieves the Hyperswarm connection object for a given validator public key.
     * @param {String | Buffer} publicKey - The public key (Buffer or hex string) of the validator.
     * @returns {Connection|undefined} - The connection object if found, otherwise undefined.
     */
    getConnection(publicKey) {
        const publicKeyHex = this.#toHexString(publicKey);
        const entry = this.#validators.get(publicKeyHex);
        return entry ? entry.connection : undefined;
    }

    /**
     * Sends a message through a specific validator without increasing sent messages count.
     * @param {Object} message - The message to send to the validator.
     * @param {String | Buffer} publicKey - A validator public key hex string to be fetched from the pool.
     * @returns {Promise<*>} A promise returned by `validator.connection.protocolSession.send(message)`.
     * @throws {ConnectionManagerError} If the validator is not connected.
     * @throws {ConnectionManagerError} If the validator has no valid connection or protocol session.
     */
    async sendSingleMessage(message, publicKey) {
        let publicKeyHex = this.#toHexString(publicKey);
        if (!this.connected(publicKeyHex)) {
            throw new ConnectionManagerError(
                `Cannot send message: validator ${publicKeyToAddress(publicKey, this.#config)} is not connected.`
            );
        }
        const validator = this.#validators.get(publicKeyHex);
        if (!validator || !validator.connection || !validator.connection.protocolSession) {
            throw new ConnectionManagerError(
                `Cannot send message: no valid connection found for validator ${publicKeyToAddress(publicKey, this.#config)}.`
            );
        }
        return validator.connection.protocolSession.send(message)
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
            this.#logger.debug('addValidator: max connections reached.');
            return false;
        }
        this.#logger.debug(`addValidator: adding validator ${publicKeyToAddress(publicKeyHex, this.#config)}`);
        if (!this.exists(publicKeyHex)) {
            this.#logger.debug(`addValidator: appending validator ${publicKeyToAddress(publicKeyHex, this.#config)}`);
            this.#append(publicKeyHex, connection);
            return true;
        } else if (!this.connected(publicKeyHex)) {
            this.#logger.debug(`addValidator: updating validator ${publicKeyToAddress(publicKeyHex, this.#config)}`);
            this.#update(publicKeyHex, connection);
            return true;
        }
        this.#logger.debug(`addValidator: didn't add validator ${publicKeyToAddress(publicKeyHex, this.#config)}`);
        return false; // TODO: Implement better success/failure reporting
    }

    /**
     * Removes a validator from the pool.
     * @param {String | Buffer} publicKey - The public key hex string of the validator to remove
     * @param {object} [options]
     * @param {boolean} [options.endConnection=true] - Whether to close the underlying socket.
     */
    remove(publicKey, { endConnection = true } = {}) {
        this.#logger.debug(`remove: removing validator ${publicKeyToAddress(publicKey, this.#config)}`);
        const publicKeyHex = this.#toHexString(publicKey);
        this.#stopHealthCheck(publicKeyHex);
        if (this.exists(publicKeyHex)) {
            const entry = this.#validators.get(publicKeyHex);
            if (endConnection && entry && entry.connection && typeof entry.connection.end === 'function') {
                try {
                    entry.connection.end();
                } catch (e) {
                    // Ignore errors on connection end
                    this.#logger.debug(`remove: failed to end connection: ${e.message}`);
                    // TODO: Consider logging these errors here in verbose mode
                }
            }
            this.#logger.debug(`remove: removing validator from map: ${publicKeyToAddress(publicKeyHex, this.#config)}. Map size before removal: ${this.#validators.size}.`);
            this.#validators.delete(publicKeyHex);
            this.#logger.debug(`remove: validator removed successfully. Map size is now ${this.#validators.size}.`);
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
        this.#logger.info(`Connection count: ${this.connectionCount()}`);
        this.#logger.info(`Validator map keys count: ${this.#validators.size}`);
        this.#logger.info(`Validator map keys:\n${Array.from(this.#validators.entries()).map(([publicKey, val]) => {
            const protocols = val.connection?.protocolSession?.preferredProtocol || 'none';
            return `${publicKeyToAddress(publicKey, this.#config)}: ${protocols}`;
        }).join('\n')}`);
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
        this.#logger.debug(`#append: appending validator ${publicKeyToAddress(publicKey, this.#config)}`);
        const publicKeyHex = this.#toHexString(publicKey);
        if (this.#validators.has(publicKeyHex)) {
            // This should never happen, but just in case, we log it
            this.#logger.debug(`#append: tried to append existing validator: ${publicKeyToAddress(publicKey, this.#config)}`);
            return;
        }
        this.#validators.set(publicKeyHex, {connection, sent: 0});
        connection.on('close', () => {
            this.#logger.debug(`#append: connection closing for validator ${publicKeyToAddress(publicKey, this.#config)}`);
            this.remove(publicKeyHex);
            this.#logger.debug(`#append: connection closed for validator ${publicKeyToAddress(publicKey, this.#config)}`);
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
        this.#logger.debug(`#update: updating validator ${publicKeyToAddress(publicKey, this.#config)}`);
        if (this.#validators.has(publicKeyHex)) {
            this.#validators.get(publicKeyHex).connection = connection;
        } else {
            this.#validators.set(publicKeyHex, {connection, sent: 0});
        }
    }

    #toHexString(publicKey) {
        return b4a.isBuffer(publicKey) ? publicKey.toString('hex') : publicKey;
    }
}

export default ConnectionManager;

// ProtocolSession is a per-peer (per Hyperswarm connection) bridge that exposes the available
// protocol messengers (legacy JSON, v1 binary) in one place.
//
// Why it exists:
// - `setupProtomuxMessages(connection)` creates Protomux channels/messages for a specific peer.

import { networkMessageFactory } from '../../../messages/network/v1/networkMessageFactory.js';
import { generateUUID } from '../../../utils/helpers.js';
import { NETWORK_CAPABILITIES, ResultCode } from '../../../utils/constants.js';
import { Logger } from '../../../utils/logger.js';

class ProtocolSession {
    #legacyProtocol;
    #v1Protocol;
    #preferredProtocol = null;
    #activeProtocol = null;
    #supportedProtocols = {
        LEGACY: 'legacy',
        V1: 'v1'
    }
    #wallet;
    #config;
    #capabilities;
    #logger;

    constructor(legacyProtocol, v1Protocol, wallet, config) {
        // These are Protomux "message" objects (returned by channel.addMessage).
        // They are connection-scoped and expose .send(...), already wired to the channel's encoding.
        this.#legacyProtocol = legacyProtocol;
        this.#v1Protocol = v1Protocol;

        this.#activeProtocol = this.#v1Protocol;
        this.#wallet = wallet;
        this.#config = config;
        this.#capabilities = NETWORK_CAPABILITIES;
        this.#logger = new Logger(config);
    }

    get preferredProtocol() {
        return this.#preferredProtocol;
    }

    get supportedProtocols() {
        return this.#supportedProtocols;
    }

    isProbed() {
        return this.#preferredProtocol !== null;
    }

    setLegacyAsPreferredProtocol() {
        if (this.isProbed()) {
            // TODO:  SOMETIMES WE ARE PROBING NODE FOR MULTIPLE AMOUNT OF TIME, THIS IS BAD. WE NEED TO INVESTIGATE THIS.
            //this.#logger.warn(`ProtocolSession: preferred protocol is already set and cannot be changed to LEGACY. Current preferred protocol: ${this.#preferredProtocol}`);
            return;
        }
        this.#preferredProtocol = this.#supportedProtocols.LEGACY;
        this.#activeProtocol = this.#legacyProtocol;
        this.#logger.debug('ProtocolSession: set preferred protocol to LEGACY');
    }

    setV1AsPreferredProtocol() {
        if (this.isProbed()) {
            // TODO:  SOMETIMES WE ARE PROBING NODE FOR MULTIPLE AMOUNT OF TIME, THIS IS BAD. WE NEED TO INVESTIGATE THIS.
            //this.#logger.warn(`ProtocolSession: preferred protocol is already set and cannot be changed to V1. Current preferred protocol: ${this.#preferredProtocol}`);
            return;
        }

        this.#preferredProtocol = this.#supportedProtocols.V1;
        this.#activeProtocol = this.#v1Protocol;
        this.#logger.debug('ProtocolSession: set preferred protocol to V1');
    }

    /**
    * Probes the peer to determine which protocol version they support/prefer. 
    * This is needed to know if the connected peer supports the new v1 protocol
    * or if we should fall back to legacy for this connection.
    * 
    * TODO: After legacy protocol is retired, we can remove the concept of "probing" and just use v1 directly.
    * For now, this is needed to determine which protocol to use for health checks.
    * A good future improvement would be to implement a more robust negotiation mechanism that doesn't rely on timeouts
    * (e.g. peer sends a "hello" message indicating supported protocol versions right after connection is established).
    */
    async probe() {
        if (this.isProbed()) {
            this.#logger.warn(`ProtocolSession: preferred protocol is already set. Skipping probe. Current preferred protocol: ${this.#preferredProtocol}`);
            return; // TODO: Consider not returning silently
        }

        try {
            const message = await this.#buildLivenessRequest();
            if (!this.#v1Protocol) {
                throw new Error('ProtocolSession: v1 protocol not available for probing');
            }
            const result = await this.#v1Protocol.send(message);
            if (result !== ResultCode.OK) {
                // TODO: Think about how to handle failure result codes after legacy protocol is retired
                this.#logger.warn(`ProtocolSession: v1 protocol probe failed with non-OK result code: ${result}`);
                this.setLegacyAsPreferredProtocol();
                return;
            }
            this.setV1AsPreferredProtocol();
        } catch (err) {
            this.#logger.debug(`ProtocolSession: v1 protocol probe failed, falling back to legacy. Details: ${err?.message ?? err}`);
            this.setLegacyAsPreferredProtocol();
        }
    }

    /**
     * Sends a single health check message to a peer.
     * This is used by the ValidatorHealthCheckService.
     * @returns {Promise<ResultCode>} Result code indicating success or failure of the health check.
     */
    async sendHealthCheck() {
        switch (this.#preferredProtocol) {
            case this.#supportedProtocols.V1:
                try {
                    const message = await this.#buildLivenessRequest();
                    return await this.#v1Protocol.send(message);
                }
                catch (err) {
                    this.#logger.error(`ProtocolSession: v1 health check failed: ${err?.message ?? err}`);
                    return ResultCode.UNEXPECTED_ERROR; // TODO: Consider just propagating the error instead
                }
            case this.#supportedProtocols.LEGACY:
                this.#logger.warn('ProtocolSession: health check not supported on LEGACY protocol');
                return ResultCode.OK; // TODO: Consider implementing a new result code (e.g. NOT_SUPPORTED) instead of returning OK
            default:
                this.#logger.warn('ProtocolSession: preferred protocol not set. Call probe() first.');
                return ResultCode.UNSPECIFIED; // TODO: Define a more specific result code.
        }
    }

    /**
     * Tells whether the connected peer supports health checks, which is a feature of the v1 protocol.
     * @returns {Boolean} True if health checks are supported in the preferred protocol, false otherwise.
     */
    isHealthCheckSupported() {
        if (this.#preferredProtocol === null) {
            throw new Error('ProtocolSession: preferred protocol not set. Call probe() first.');
        }
        return this.#preferredProtocol === this.#supportedProtocols.V1;
    }

    // TODO: Consider moving this method to be used only in V1 internally, just like 'encode'
    decode(message) {
        return this.#activeProtocol.decode(message);
    }

    async send(message) {
        return this.#activeProtocol.send(message);
    }

    sendAndForget(message) {
        this.#activeProtocol.sendAndForget(message);
    }

    close() {
        if (this.#legacyProtocol) {
            try {
                this.#legacyProtocol.close();
            } catch (e) {
                this.#logger.error(`ProtocolSession: failed to close legacy channel: ${e?.message ?? e}`); // TODO: Think about throwing instead
            }
        }

        if (this.#v1Protocol) {
            try {
                this.#v1Protocol.close();
            } catch (e) {
                this.#logger.error(`ProtocolSession: failed to close v1 channel: ${e?.message ?? e}`); // TODO: Think about throwing instead
            }
        }
    }

    async #buildLivenessRequest() {
        if (!this.#wallet || !this.#config) {
            throw new Error('ProtocolSession: wallet/config not set for liveness request');
        }
        const requestId = generateUUID();
        return await networkMessageFactory(this.#wallet, this.#config)
            .buildLivenessRequest(requestId, this.#capabilities);
    }
}

export default ProtocolSession;

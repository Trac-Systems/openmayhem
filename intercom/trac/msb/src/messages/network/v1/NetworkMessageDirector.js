import {NetworkOperationType} from '../../../utils/constants.js';

/**
 * Director for v1 internal network protocol messages.
 */
class NetworkMessageDirector {
    #builder;

    /**
     * @param {NetworkMessageBuilder} builderInstance
     */
    constructor(builderInstance) {
        this.#builder = builderInstance;
    }

    /**
     * Build a liveness request message.
     * @param {string} id
     * @param {Buffer} data
     * @param {string[]} capabilities
     * @returns {Promise<object>}
     */
    async buildLivenessRequest(id, capabilities) {
        await this.#builder
            .setType(NetworkOperationType.LIVENESS_REQUEST)
            .setId(id)
            .setTimestamp()
            .setCapabilities(capabilities)
            .buildPayload();

        return this.#builder.getResult();
    }

    /**
     * Build a liveness response message.
     * @param {string} id
     * @param {string[]} capabilities
     * @param {number} statusCode
     * @returns {Promise<object>}
     */
    async buildLivenessResponse(id, capabilities, statusCode) {
        await this.#builder
            .setType(NetworkOperationType.LIVENESS_RESPONSE)
            .setId(id)
            .setTimestamp()
            .setCapabilities(capabilities)
            .setResultCode(statusCode)
            .buildPayload();

        return this.#builder.getResult();
    }

    /**
     * Build a broadcast transaction request message.
     * @param {string} id
     * @param {Buffer} data
     * @param {string[]} capabilities
     * @returns {Promise<object>}
     */
    async buildBroadcastTransactionRequest(id, data, capabilities) {
        await this.#builder
            .setType(NetworkOperationType.BROADCAST_TRANSACTION_REQUEST)
            .setId(id)
            .setTimestamp()
            .setData(data)
            .setCapabilities(capabilities)
            .buildPayload();

        return this.#builder.getResult();
    }

    /**
     * Build a broadcast transaction response message.
     *
     * Allowed payload variants:
     * 1) resultCode === OK - proof must be non-empty and timestamp must be > 0.
     * 2) resultCode === TX_ACCEPTED_PROOF_UNAVAILABLE - proof must be empty and timestamp must be > 0.
     * 3) resultCode !== OK and resultCode !== TX_ACCEPTED_PROOF_UNAVAILABLE - proof must be empty and timestamp must be 0.
     *
     * @param {string} id
     * @param {string[]} capabilities
     * @param {number} resultCode
     * @param {Buffer|null|undefined} proof
     * @param {number|Date|null|undefined} timestamp - When transaction has been appended by the validator
     * @returns {Promise<object>}
     */
    async buildBroadcastTransactionResponse(id, capabilities, resultCode, proof = null, timestamp = null) {
        await this.#builder
            .setType(NetworkOperationType.BROADCAST_TRANSACTION_RESPONSE)
            .setId(id)
            .setTimestamp()
            .setCapabilities(capabilities)
            .setProof(proof)
            .setTimestampLedger(timestamp)
            .setResultCode(resultCode)
            .buildPayload();

        return this.#builder.getResult();
    }

}

export default NetworkMessageDirector;

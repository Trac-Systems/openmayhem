import { NetworkOperationType } from '../../../utils/constants.js';

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
     * Build a validator connection request message.
     * @param {string} id
     * @param {string} issuerAddress
     * @param {string[]} capabilities
     * @returns {Promise<object>}
     */
    async buildValidatorConnectionRequest(id, issuerAddress, capabilities) {
        await this.#builder
            .setType(NetworkOperationType.VALIDATOR_CONNECTION_REQUEST)
            .setId(id)
            .setTimestamp()
            .setIssuerAddress(issuerAddress)
            .setCapabilities(capabilities)
            .buildPayload()


        return this.#builder.getResult();
    }

    /**
     * Build a validator connection response message.
     * @param {string} id
     * @param {string} issuerAddress
     * @param {string[]} capabilities
     * @param {number} statusCode
     * @returns {Promise<object>}
     */
    async buildValidatorConnectionResponse(id, issuerAddress, capabilities, statusCode) {
        await this.#builder
        .setType(NetworkOperationType.VALIDATOR_CONNECTION_RESPONSE)
        .setId(id)
        .setTimestamp()
        .setIssuerAddress(issuerAddress)
        .setCapabilities(capabilities)
        .setResultCode(statusCode)
        .buildPayload()

        return this.#builder.getResult();
    }

    /**
     * Build a liveness request message.
     * @param {string} id
     * @param {Buffer} data
     * @param {string[]} capabilities
     * @returns {Promise<object>}
     */
    async buildLivenessRequest(id, data, capabilities) {
        await this.#builder
            .setType(NetworkOperationType.LIVENESS_REQUEST)
            .setId(id)
            .setTimestamp()
            .setData(data)
            .setCapabilities(capabilities)
            .buildPayload();

        return this.#builder.getResult();
    }

    /**
     * Build a liveness response message.
     * @param {string} id
     * @param {Buffer} data
     * @param {string[]} capabilities
     * @param {number} statusCode
     * @returns {Promise<object>}
     */
    async buildLivenessResponse(id, data, capabilities, statusCode) {
        await this.#builder
            .setType(NetworkOperationType.LIVENESS_RESPONSE)
            .setId(id)
            .setTimestamp()
            .setData(data)
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
     * @param {string} id
     * @param {string[]} capabilities
     * @param {number} statusCode
     * @returns {Promise<object>}
     */
    async buildBroadcastTransactionResponse(id, capabilities, statusCode) {
        await this.#builder
            .setType(NetworkOperationType.BROADCAST_TRANSACTION_RESPONSE)
            .setId(id)
            .setTimestamp()
            .setCapabilities(capabilities)
            .setResultCode(statusCode)
            .buildPayload();

        return this.#builder.getResult();
    }

}

export default NetworkMessageDirector;

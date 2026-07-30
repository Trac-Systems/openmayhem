import tracCryptoApi from 'trac-crypto-api';
import b4a from 'b4a';
import {createMessage, safeWriteUInt32BE, idToBuffer, timestampToBuffer} from "../../../utils/buffer.js";
import {NetworkOperationType, ResultCode} from '../../../utils/constants.js';
import {isAddressValid} from "../../../core/state/utils/address.js";
import {encodeCapabilities} from "../../../utils/buffer.js";

/**
 * Builder for v1 internal network protocol messages.
 * @param {IWallet} wallet
 * @param {Config} config
 */
class NetworkMessageBuilder {
    #wallet;
    #type;
    #capabilities;
    #id;
    #timestamp;
    #resultCode;
    #data;
    #proof;
    #timestamp_ledger;
    #header;
    #payloadKey;
    #body;
    #config;

    /**
     * @param {IWallet} wallet
     * @param {Config} config
     */
    constructor(wallet, config) {
        this.#config = config;
        if (!wallet || typeof wallet !== 'object') {
            throw new Error('Wallet must be a valid wallet object');
        }
        if (!isAddressValid(wallet.address, this.#config.addressPrefix)) {
            throw new Error('Wallet should have a valid TRAC address.');
        }

        this.#wallet = wallet;
    }

    setId(id) {
        this.#id = id;
        return this;
    }

    setTimestamp() {
        this.#timestamp = Date.now();
        return this;
    }

    setCapabilities(capabilities = []) {
        if (!Array.isArray(capabilities) || !capabilities.every(capability => typeof capability === 'string')) {
            throw new Error('Capabilities must be a string array.');
        }

        this.#capabilities = capabilities
        return this;
    }

    setType(type) {
        if (!Object.values(NetworkOperationType).includes(type)) {
            throw new Error(`Invalid operation type: ${type}`);
        }
        this.#type = type;
        return this;
    }

    setResultCode(code) {
        if (!Object.values(ResultCode).includes(code)) {
            throw new Error(`Invalid network result code: ${code}`);
        }

        this.#resultCode = code;
        return this;
    }

    setData(data) {
        // case when response have to be empty.
        if (data === undefined || data === null) data = b4a.alloc(0);

        if (!b4a.isBuffer(data)) {
            throw new Error(`Data must be a buffer.`);
        }
        this.#data = data;
        return this;
    }

    setProof(proof) {
        if (proof === undefined || proof === null) proof = b4a.alloc(0);
        if (!b4a.isBuffer(proof)) {
            throw new Error(`Proof must be a buffer.`);
        }
        this.#proof = proof;
        return this;
    }

    setTimestampLedger(timestamp) {
        if (timestamp === undefined || timestamp === null) {
            this.#timestamp_ledger = null;
            return this;
        }

        const value = timestamp instanceof Date ? timestamp.getTime() : timestamp;
        if (!Number.isSafeInteger(value) || value < 0) {
            throw new Error('timestamp must be a non-negative safe integer or Date.');
        }

        this.#timestamp_ledger = value;
        return this;
    }

    #setHeader() {
        if (!this.#type) throw new Error('Header requires type to be set');
        if (!this.#id) throw new Error('Header requires id to be set');
        if (!this.#timestamp) throw new Error('Header requires a timestamp provider');
        if (!Array.isArray(this.#capabilities)) throw new Error('Header requires capabilities array');

        this.#header = {
            type: this.#type,
            id: this.#id,
            timestamp: this.#timestamp,
            capabilities: this.#capabilities,
        };
        return this;
    }

    async #buildLivenessRequestPayload() {
        const nonce = tracCryptoApi.nonce.generate();
        const tsBuf = timestampToBuffer(this.#timestamp);
        const idBuf = idToBuffer(this.#id);
        const message = createMessage(
            this.#type,
            idBuf,
            tsBuf,
            nonce,
            encodeCapabilities(this.#capabilities),
        );
        const hash = await tracCryptoApi.hash.blake3(message);
        const signature = this.#wallet.sign(hash);

        this.#payloadKey = 'liveness_request';
        this.#body = {
            nonce,
            signature
        };
    }

    async #buildLivenessResponsePayload() {
        if (this.#resultCode === null || this.#resultCode === undefined) {
            throw new Error('Result code must be set before building liveness response');
        }

        const nonce = tracCryptoApi.nonce.generate();
        const tsBuf = timestampToBuffer(this.#timestamp);
        const idBuf = idToBuffer(this.#id);
        const message = createMessage(
            this.#type,
            idBuf,
            tsBuf,
            nonce,
            safeWriteUInt32BE(this.#resultCode, 0),
            encodeCapabilities(this.#capabilities),
        );
        const hash = await tracCryptoApi.hash.blake3(message);
        const signature = this.#wallet.sign(hash);

        this.#payloadKey = 'liveness_response';
        this.#body = {
            nonce,
            signature,
            result: this.#resultCode
        };
    }

    async #buildBroadcastRequestPayload() {
        if (!b4a.isBuffer(this.#data)) {
            throw new Error('Data must be set before building broadcast transaction request');
        }
        const nonce = tracCryptoApi.nonce.generate();
        const tsBuf = timestampToBuffer(this.#timestamp);
        const idBuf = idToBuffer(this.#id);
        const message = createMessage(
            this.#type,
            idBuf,
            tsBuf,
            this.#data,
            nonce,
            encodeCapabilities(this.#capabilities),
        );
        const hash = await tracCryptoApi.hash.blake3(message);
        const signature = this.#wallet.sign(hash);

        this.#payloadKey = 'broadcast_transaction_request';
        this.#body = {
            data: this.#data,
            nonce,
            signature
        };
    }

    async #buildBroadcastTransactionResponse() {
        if (this.#resultCode === null || this.#resultCode === undefined) {
            throw new Error('Result code must be set before building broadcast transaction response');
        }
        const nonce = tracCryptoApi.nonce.generate();
        const tsBuf = timestampToBuffer(this.#timestamp);
        const idBuf = idToBuffer(this.#id);
        const proof = b4a.isBuffer(this.#proof) ? this.#proof : b4a.alloc(0);
        const hasProof = proof.length > 0;
        const timestamp = Number.isSafeInteger(this.#timestamp_ledger) ? this.#timestamp_ledger : 0;
        const hasTimestamp = timestamp > 0;

        if (this.#resultCode === ResultCode.OK) {
            if (!hasProof || !hasTimestamp) {
                throw new Error('Result code OK requires non-empty proof and timestamp > 0.');
            }
        } else if (this.#resultCode === ResultCode.TX_ACCEPTED_PROOF_UNAVAILABLE) {
            if (hasProof) {
                throw new Error('Result code TX_ACCEPTED_PROOF_UNAVAILABLE requires empty proof.');
            }
            if (!hasTimestamp) {
                throw new Error('Result code TX_ACCEPTED_PROOF_UNAVAILABLE requires timestamp > 0.');
            }
        } else {
            if (hasProof) {
                throw new Error('Non-OK result code requires empty proof.');
            }
            if (timestamp !== 0) {
                throw new Error('Non-OK result code requires timestamp to be 0, except TX_ACCEPTED_PROOF_UNAVAILABLE.');
            }
        }

        const message = createMessage(
            this.#type,
            idBuf,
            tsBuf,
            nonce,
            proof,
            timestampToBuffer(timestamp),
            safeWriteUInt32BE(this.#resultCode, 0),
            encodeCapabilities(this.#capabilities),
        );

        const hash = await tracCryptoApi.hash.blake3(message);
        const signature = this.#wallet.sign(hash);
        this.#payloadKey = 'broadcast_transaction_response';
        this.#body = {
            nonce,
            signature,
            proof,
            timestamp: timestamp,
            result: this.#resultCode
        };
    }

    async buildPayload() {
        this.#setHeader();

        switch (this.#type) {
            case NetworkOperationType.LIVENESS_REQUEST: {
                await this.#buildLivenessRequestPayload();
                break;
            }
            case NetworkOperationType.LIVENESS_RESPONSE: {
                await this.#buildLivenessResponsePayload();
                break;
            }
            case NetworkOperationType.BROADCAST_TRANSACTION_REQUEST: {
                await this.#buildBroadcastRequestPayload();
                break;
            }
            case NetworkOperationType.BROADCAST_TRANSACTION_RESPONSE: {
                await this.#buildBroadcastTransactionResponse();
                break;
            }
            default:
                throw new Error(`Unsupported network type ${this.#type}`);
        }
    }

    getResult() {
        if (!this.#header || !this.#payloadKey || !this.#body) {
            throw new Error('Header or payload not set before getResult');
        }

        return {
            ...this.#header,
            [this.#payloadKey]: this.#body
        };
    }
}

export default NetworkMessageBuilder;

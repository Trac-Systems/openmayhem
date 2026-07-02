import PeerWallet from 'trac-wallet';
import b4a from 'b4a';
import {createMessage, safeWriteUInt32BE, idToBuffer, timestampToBuffer} from "../../../utils/buffer.js";
import {NetworkOperationType, ResultCode} from '../../../utils/constants.js';
import {addressToBuffer, isAddressValid} from "../../../core/state/utils/address.js";
import {encodeCapabilities} from "../../../utils/buffer.js";

/**
 * Builder for v1 internal network protocol messages.
 * @param {PeerWallet} wallet
 * @param {object} config
 */
class NetworkMessageBuilder {
    #wallet;
    #type;
    #capabilities;
    #id;
    #timestamp;
    #issuerAddress;
    #resultCode;
    #data;
    #header;
    #payloadKey;
    #body;
    #config;

    /**
     * @param {PeerWallet} wallet
     * @param {object} config
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

    setIssuerAddress(issuerAddress) {
        if (!isAddressValid(issuerAddress, this.#config.addressPrefix)) {
            throw new Error('Issuer TRAC address must be valid.');
        }
        this.#issuerAddress = issuerAddress;
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
        if (!b4a.isBuffer(data)) {
            throw new Error(`Data must be a buffer.`);
        }
        this.#data = data;
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

    async #buildValidatorConnectionRequestPayload() {
        const issuer = this.#issuerAddress
        if (!isAddressValid(issuer, this.#config.addressPrefix)) {
            throw new Error('Issuer address must be a valid TRAC address');
        }

        if (this.#issuerAddress !== this.#wallet.address) {
            throw new Error('Issuer address must be the signer address');
        }

        const nonce = PeerWallet.generateNonce();
        const tsBuf = timestampToBuffer(this.#timestamp);
        const idBuf = idToBuffer(this.#id);
        const message = createMessage(
            this.#type,
            idBuf,
            tsBuf,
            addressToBuffer(issuer, this.#config.addressPrefix),
            nonce,
            encodeCapabilities(this.#capabilities),
        );
        const hash = await PeerWallet.blake3(message);
        const signature = this.#wallet.sign(hash);

        this.#payloadKey = 'validator_connection_request';
        this.#body = {
            issuer_address: issuer,
            nonce,
            signature
        };
    }

    async #buildValidatorConnectionResponsePayload() {
        const issuer = this.#issuerAddress
        if (!isAddressValid(issuer, this.#config.addressPrefix)) {
            throw new Error('Issuer address must be a valid TRAC address');
        }

        if (this.#issuerAddress === this.#wallet.address) {
            throw new Error('Issuer address must be the different than the signer address');
        }

        if (this.#resultCode === null || this.#resultCode === undefined) {
            throw new Error('Result code must be set before building validator connection response');
        }

        const nonce = PeerWallet.generateNonce();
        const tsBuf = timestampToBuffer(this.#timestamp);
        const idBuf = idToBuffer(this.#id);
        const message = createMessage(
            this.#type,
            idBuf,
            tsBuf,
            addressToBuffer(issuer, this.#config.addressPrefix),
            nonce,
            safeWriteUInt32BE(this.#resultCode, 0),
            encodeCapabilities(this.#capabilities),
        );
        const hash = await PeerWallet.blake3(message);
        const signature = this.#wallet.sign(hash);

        this.#payloadKey = 'validator_connection_response';
        this.#body = {
            issuer_address: issuer,
            nonce,
            signature,
            result: this.#resultCode
        };
    }

    async #buildLivenessRequestPayload() {
        const nonce = PeerWallet.generateNonce();
        const tsBuf = timestampToBuffer(this.#timestamp);
        const idBuf = idToBuffer(this.#id);
        const message = createMessage(
            this.#type,
            idBuf,
            tsBuf,
            nonce,
            encodeCapabilities(this.#capabilities),
        );
        const hash = await PeerWallet.blake3(message);
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

        const nonce = PeerWallet.generateNonce();
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
        const hash = await PeerWallet.blake3(message);
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
        const nonce = PeerWallet.generateNonce();
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
        const hash = await PeerWallet.blake3(message);
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
        const nonce = PeerWallet.generateNonce();
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
        const hash = await PeerWallet.blake3(message);
        const signature = this.#wallet.sign(hash);

        this.#payloadKey = 'broadcast_transaction_response';
        this.#body = {
            nonce,
            signature,
            result: this.#resultCode
        };
    }

    async buildPayload() {
        this.#setHeader();

        switch (this.#type) {
            case NetworkOperationType.VALIDATOR_CONNECTION_REQUEST: {
                await this.#buildValidatorConnectionRequestPayload();
                break;
            }
            case NetworkOperationType.VALIDATOR_CONNECTION_RESPONSE: {
                await this.#buildValidatorConnectionResponsePayload();
                break;
            }
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

import V1ValidationSchema from "./V1ValidationSchema.js";
import {NetworkOperationType, ResultCode} from "../../../../../utils/constants.js";
import tracCryptoApi from "trac-crypto-api";
import b4a from "b4a";
import {
    createMessage,
    encodeCapabilities,
    idToBuffer,
    safeWriteUInt32BE,
    timestampToBuffer
} from "../../../../../utils/buffer.js";
import {
    V1ProtocolError,
} from "../V1ProtocolError.js";
import _ from 'lodash';

class V1BaseOperation {
    #v1ValidationSchema

    constructor(_config) {
        this.#v1ValidationSchema = new V1ValidationSchema();
    }

    async validate(_payload, _connection, _pendingRequestServiceEntry) {
        throw new Error("Method 'validate()' must be implemented.");
    }

    isPayloadSchemaValid(payload) {
        if (_.isNil(payload?.type)) {
            throw new V1ProtocolError(ResultCode.INVALID_PAYLOAD,'Payload or payload type is missing.');
        }

        const selectedValidator = this.#selectCheckSchemaValidator(payload.type);
        const isPayloadValid = selectedValidator(payload);
        if (!isPayloadValid) {
            throw new V1ProtocolError(ResultCode.INVALID_PAYLOAD, 'Payload is invalid.');
        }
    }

    async validateSignature(payload, remotePublicKey) {
        let signature;
        let message;
        try {
            const result = this.#buildSignatureMessage(payload);
            signature = result.signature;
            message = result.message;
        } catch (error) {
            if (error instanceof V1ProtocolError) {
                throw error;
            }
            throw new V1ProtocolError(ResultCode.INVALID_PAYLOAD, `Failed to build signature message: ${error.message}`);
        }

        let hash;
        try {
            hash = await tracCryptoApi.hash.blake3(message);
        } catch {
            throw new V1ProtocolError(ResultCode.INVALID_PAYLOAD, 'Failed to hash signature message.');
        }

        let verified = false;
        try {
            verified = tracCryptoApi.signature.verify(signature, hash, remotePublicKey);
        } catch {
            verified = false;
        }
        if (!verified) {
            throw new V1ProtocolError(ResultCode.SIGNATURE_INVALID, 'signature verification failed.');
        }
    }

    #buildSignatureMessage(payload) {
        if (!Number.isInteger(payload.type)) {
            throw new V1ProtocolError(ResultCode.INVALID_PAYLOAD, 'Operation type must be an integer.');
        }
        if (payload.type === 0) {
            throw new V1ProtocolError(ResultCode.INVALID_PAYLOAD, 'Operation type is unspecified.');
        }

        const idBuf = idToBuffer(payload.id);
        const tsBuf = timestampToBuffer(payload.timestamp);
        const capsBuf = encodeCapabilities(payload.capabilities ?? []);

        switch (payload.type) {
            case NetworkOperationType.LIVENESS_REQUEST: {
                const nonce = payload.liveness_request.nonce;
                const signature = payload.liveness_request.signature;
                const message = createMessage(payload.type, idBuf, tsBuf, nonce, capsBuf);
                return {signature, message};
            }
            case NetworkOperationType.LIVENESS_RESPONSE: {
                const nonce = payload.liveness_response.nonce;
                const signature = payload.liveness_response.signature;
                const result = payload.liveness_response.result;
                const message = createMessage(payload.type, idBuf, tsBuf, nonce, safeWriteUInt32BE(result, 0), capsBuf);
                return {signature, message};
            }
            case NetworkOperationType.BROADCAST_TRANSACTION_REQUEST: {
                const data = payload.broadcast_transaction_request.data;
                const nonce = payload.broadcast_transaction_request.nonce;
                const signature = payload.broadcast_transaction_request.signature;
                const message = createMessage(payload.type, idBuf, tsBuf, data, nonce, capsBuf);
                return {signature, message};
            }
            case NetworkOperationType.BROADCAST_TRANSACTION_RESPONSE: {
                const nonce = payload.broadcast_transaction_response.nonce;
                const signature = payload.broadcast_transaction_response.signature;
                const result = payload.broadcast_transaction_response.result;
                const proofRaw = payload.broadcast_transaction_response.proof;
                const proof = b4a.isBuffer(proofRaw) ? proofRaw : b4a.alloc(0);
                const hasProof = proof.length > 0;
                const timestampRaw = payload.broadcast_transaction_response.timestamp;
                const timestamp = Number.isSafeInteger(timestampRaw) ? timestampRaw : 0;
                const hasTimestamp = timestamp > 0;

                if (result === ResultCode.OK) {
                    if (!hasProof || !hasTimestamp) {
                        throw new V1ProtocolError(ResultCode.INVALID_PAYLOAD, 'Result code OK requires non-empty proof and timestamp > 0.');
                    }
                } else if (result === ResultCode.TX_ACCEPTED_PROOF_UNAVAILABLE) {
                    if (hasProof) {
                        throw new V1ProtocolError(ResultCode.INVALID_PAYLOAD, 'Result code TX_ACCEPTED_PROOF_UNAVAILABLE requires empty proof.');
                    }
                    if (!hasTimestamp) {
                        throw new V1ProtocolError(ResultCode.INVALID_PAYLOAD, 'Result code TX_ACCEPTED_PROOF_UNAVAILABLE requires timestamp > 0.');
                    }
                } else {
                    if (hasProof) {
                        throw new V1ProtocolError(ResultCode.INVALID_PAYLOAD, 'Non-OK result code requires empty proof.');
                    }
                    if (timestamp !== 0) {
                        throw new V1ProtocolError(ResultCode.INVALID_PAYLOAD, 'Non-OK result code requires timestamp to be 0, except TX_ACCEPTED_PROOF_UNAVAILABLE.');
                    }
                }

                const message = createMessage(
                    payload.type,
                    idBuf,
                    tsBuf,
                    nonce,
                    proof,
                    timestampToBuffer(timestamp),
                    safeWriteUInt32BE(result, 0),
                    capsBuf
                );

                return {signature, message};
            }
            default:
                throw new V1ProtocolError(ResultCode.UNEXPECTED_ERROR, `Unknown operation type: ${payload.type}`);
        }
    }

    #selectCheckSchemaValidator(type) {
        if (!Number.isInteger(type)) {
            throw new V1ProtocolError(ResultCode.INVALID_PAYLOAD, 'Operation type must be an integer.');
        }
        if (type === 0) {
            throw new V1ProtocolError(ResultCode.INVALID_PAYLOAD, 'Operation type is unspecified.');
        }

        switch (type) {
            case NetworkOperationType.LIVENESS_REQUEST:
                return this.#v1ValidationSchema.validateV1LivenessRequest.bind(this.#v1ValidationSchema);
            case NetworkOperationType.LIVENESS_RESPONSE:
                return this.#v1ValidationSchema.validateV1LivenessResponse.bind(this.#v1ValidationSchema);
            case NetworkOperationType.BROADCAST_TRANSACTION_REQUEST:
                return this.#v1ValidationSchema.validateV1BroadcastTransactionRequest.bind(this.#v1ValidationSchema);
            case NetworkOperationType.BROADCAST_TRANSACTION_RESPONSE:
                return this.#v1ValidationSchema.validateV1BroadcastTransactionResponse.bind(this.#v1ValidationSchema);
            default:
                throw new V1ProtocolError(ResultCode.UNEXPECTED_ERROR, `Unknown operation type: ${type}`);
        }
    }

    validatePeerCorrectness(remotePublicKey, pendingRequestServiceEntry) {
        const senderPublicKeyHex = b4a.toString(remotePublicKey, 'hex');
        if (senderPublicKeyHex !== pendingRequestServiceEntry.requestedTo) {
            throw new V1ProtocolError(
                ResultCode.INVALID_PAYLOAD,
                `Response sender mismatch. Expected ${pendingRequestServiceEntry.requestedTo}, got ${senderPublicKeyHex}.`
            );
        }
    }

    validateResponseType(payload, pendingRequestServiceEntry) {
        let expectedResponseType;
        switch (pendingRequestServiceEntry.requestType) {
            case NetworkOperationType.LIVENESS_REQUEST:
                expectedResponseType = NetworkOperationType.LIVENESS_RESPONSE;
                break;
            case NetworkOperationType.BROADCAST_TRANSACTION_REQUEST:
                expectedResponseType = NetworkOperationType.BROADCAST_TRANSACTION_RESPONSE;
                break;
            default:
                throw new V1ProtocolError(
                    ResultCode.UNEXPECTED_ERROR,
                    `Unsupported pending request type: ${pendingRequestServiceEntry.requestType}.`
                );
        }

        if (payload.type !== expectedResponseType) {
            throw new V1ProtocolError(
                ResultCode.INVALID_PAYLOAD,
                `Response type mismatch for id ${pendingRequestServiceEntry.id}. Expected ${expectedResponseType}, got ${payload.type}.`
            );
        }
    }

}

export default V1BaseOperation;

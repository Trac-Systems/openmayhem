import Autobase from "autobase";
import b4a from "b4a";
import Hypercore from 'hypercore';
import V1BaseOperation from "./V1BaseOperation.js";
import {unsafeDecodeApplyOperation} from "../../../../../utils/protobuf/operationHelpers.js";
import {isDeepEqualApplyPayload} from "../../../../../utils/deepEqualApplyPayload.js";
import {addressToBuffer, bufferToAddress} from "../../../../state/utils/address.js";
import {publicKeyToAddress} from "../../../../../utils/helpers.js";
import Check from "../../../../../utils/check.js";
import {OperationType, ResultCode} from "../../../../../utils/constants.js";
import {V1ProtocolError} from "../V1ProtocolError.js";

const VALIDATOR_METADATA_FIELDS = new Set(["va", "vn", "vs"]);

const stripValidatorMetadata = (value) => {

    if (value === null || value === undefined) return value;
    if (b4a.isBuffer(value)) return value;
    if (Array.isArray(value)) return value.map(stripValidatorMetadata);
    if (typeof value !== "object") return value;

    const result = {};
    for (const [key, nestedValue] of Object.entries(value)) {
        if (VALIDATOR_METADATA_FIELDS.has(key)) {
            result[key] = null;
            continue;
        }
        result[key] = stripValidatorMetadata(nestedValue);
    }
    return result;
};

class V1BroadcastTransactionResponse extends V1BaseOperation {
    #config;
    #check;
    #state;

    constructor(state, config) {
        super(config);
        this.#state = state;
        this.#config = config;
        this.#check = new Check(config);
    }

    async validate(payload, connection, pendingRequestServiceEntry) {
        this.isPayloadSchemaValid(payload);
        this.validateResponseType(payload, pendingRequestServiceEntry);
        this.validatePeerCorrectness(connection.remotePublicKey, pendingRequestServiceEntry);
        await this.validateSignature(payload, connection.remotePublicKey);
        const resultCode = payload.broadcast_transaction_response.result;
        // if result code is not OK, we can skip the rest of the validations.
        this.validateIfResultCodeIsValidatorInternalError(resultCode);
        if (resultCode === ResultCode.OK) {
            const proofResult = await this.verifyProofOfPublication(payload);
            const {
                validatorDecodedTx,
                manifest
            } = await this.assertProofPayloadMatchesRequestPayload(proofResult, pendingRequestServiceEntry);
            this.validateDecodedCompletePayloadSchema(validatorDecodedTx);
            const {
                writerKeyFromManifest,
                validatorAddressCorrelatedWithManifest
            } = await this.validateWritingKey(validatorDecodedTx, manifest);
            await this.validateValidatorCorrectness(
                validatorDecodedTx,
                connection.remotePublicKey,
                writerKeyFromManifest,
                validatorAddressCorrelatedWithManifest,
            );
        }

        return true;
    }

    validateDecodedCompletePayloadSchema(validatorDecodedTx) {
        const type = validatorDecodedTx?.type;
        if (!Number.isInteger(type)) {
            throw new V1ProtocolError(
                ResultCode.VALIDATOR_RESPONSE_TX_TYPE_INVALID,
                'Decoded validator transaction type is missing or invalid.'
            );
        }

        if (!Object.values(OperationType).includes(type)) {
            throw new V1ProtocolError(
                ResultCode.VALIDATOR_RESPONSE_TX_TYPE_UNKNOWN,
                `Decoded validator transaction type ${type} is not defined in OperationType constants.`
            );
        }

        let isValid = false;
        switch (type) {
            case OperationType.ADD_WRITER:
            case OperationType.REMOVE_WRITER:
            case OperationType.ADMIN_RECOVERY:
                isValid =  this.#check.validateRoleAccessOperation(validatorDecodedTx);
                break;
            case OperationType.BOOTSTRAP_DEPLOYMENT:
                isValid =  this.#check.validateBootstrapDeploymentOperation(validatorDecodedTx);
                break;
            case OperationType.TX:
                isValid = this.#check.validateTransactionOperation(validatorDecodedTx);
                break;
            case OperationType.TRANSFER:
                isValid = this.#check.validateTransferOperation(validatorDecodedTx);
                break;
            default:
                throw new V1ProtocolError(
                    ResultCode.VALIDATOR_RESPONSE_TX_TYPE_UNSUPPORTED,
                    `Unsupported decoded validator transaction type: ${type}`
                );
        }

        if (!isValid) {
            throw new V1ProtocolError(
                ResultCode.VALIDATOR_RESPONSE_SCHEMA_INVALID,
                `Decoded validator transaction schema validation failed for type ${type}.`
            );
        }
    }

    async verifyProofOfPublication(payload) {
        const proof = payload.broadcast_transaction_response.proof;
        return this.#state.verifyProofOfPublication(proof);
    }

    async assertProofPayloadMatchesRequestPayload(proofResult, pendingRequestServiceEntry) {
        const stateTxEncodedFromRequest = pendingRequestServiceEntry.requestTxData;
        if (!b4a.isBuffer(stateTxEncodedFromRequest) || stateTxEncodedFromRequest.length === 0) {
            throw new V1ProtocolError(
                ResultCode.PENDING_REQUEST_MISSING_TX_DATA,
                'Missing transaction data in pending request entry.'
            );
        }
        const provenBlock = proofResult.proof.block.value;
        const manifest = proofResult.proof.manifest;
        const stateTxEncodedFromResponse = await Autobase.decodeValue(provenBlock);

        const stateTxDecodedFromRequest = unsafeDecodeApplyOperation(stateTxEncodedFromRequest);
        const stateTxDecodedFromResponse = unsafeDecodeApplyOperation(stateTxEncodedFromResponse);
        const strippedValidatorPayload = stripValidatorMetadata(stateTxDecodedFromResponse);

        if (!isDeepEqualApplyPayload(stateTxDecodedFromRequest, strippedValidatorPayload)) {
            throw new V1ProtocolError(
                ResultCode.PROOF_PAYLOAD_MISMATCH,
                'Decoded transaction payload mismatch after removing validator metadata fields.'
            );
        }
        return {validatorDecodedTx: stateTxDecodedFromResponse, manifest};
    }

    async validateWritingKey(validatorDecodedTx, manifest) {
        const writerKeyFromManifest = Hypercore.key(manifest);
        const writerKeyFromManifestHex = b4a.toString(writerKeyFromManifest, "hex");

        const validatorAddressBuffer = await this.#state.getRegisteredWriterKey(writerKeyFromManifestHex);
        if (!b4a.isBuffer(validatorAddressBuffer) || validatorAddressBuffer.length === 0) {
            throw new V1ProtocolError(
                ResultCode.VALIDATOR_WRITER_KEY_NOT_REGISTERED,
                `Validator with writer key ${writerKeyFromManifestHex} is not registered.`
            );
        }

        return {
            writerKeyFromManifest,
            validatorAddressCorrelatedWithManifest: validatorAddressBuffer
        };
    }

    async validateValidatorCorrectness(validatorDecodedTx, connectionRemotePublicKey, writerKeyFromManifest, validatorAddressCorrelatedWithManifest) {
        const validatorAddressFromTx = extractRequiredVaFromDecodedTx(validatorDecodedTx);
        const validatorAddressFromConnectionPublicKey = addressToBuffer(
            publicKeyToAddress(connectionRemotePublicKey, this.#config),
            this.#config.addressPrefix
        );

        if (!b4a.equals(validatorAddressFromTx, validatorAddressFromConnectionPublicKey)) {
            throw new V1ProtocolError(
                ResultCode.VALIDATOR_ADDRESS_MISMATCH,
                `Validator address from transaction (${bufferToAddress(validatorAddressFromTx, this.#config.addressPrefix)}) does not match address derived from connection public key (${bufferToAddress(validatorAddressFromConnectionPublicKey, this.#config.addressPrefix)}).`
            );
        }

        if (!b4a.equals(validatorAddressFromTx, validatorAddressCorrelatedWithManifest)) {
            throw new V1ProtocolError(
                ResultCode.VALIDATOR_ADDRESS_MISMATCH,
                `Validator address from transaction (${bufferToAddress(validatorAddressFromTx, this.#config.addressPrefix)}) does not match address correlated with manifest writer key (${bufferToAddress(validatorAddressCorrelatedWithManifest, this.#config.addressPrefix)}).`
            );
        }

        if (!b4a.equals(validatorAddressCorrelatedWithManifest, validatorAddressFromConnectionPublicKey)) {
            throw new V1ProtocolError(
                ResultCode.VALIDATOR_ADDRESS_MISMATCH,
                `Validator address correlated with manifest writer key (${bufferToAddress(validatorAddressCorrelatedWithManifest, this.#config.addressPrefix)}) does not match address derived from connection public key (${bufferToAddress(validatorAddressFromConnectionPublicKey, this.#config.addressPrefix)}).`
            );
        }

        const validatorAddressFromConnection = bufferToAddress(validatorAddressFromConnectionPublicKey, this.#config.addressPrefix);
        const account = await this.#state.getNodeEntry(validatorAddressFromConnection);

        if (!account) {
            throw new V1ProtocolError(
                ResultCode.VALIDATOR_NODE_ENTRY_NOT_FOUND,
                `No node entry found in state for validator address derived from connection public key (${validatorAddressFromConnection}).`
            );
        }

        if (!account.isWriter) {
            throw new V1ProtocolError(
                ResultCode.VALIDATOR_NODE_NOT_WRITER,
                `Node entry found for validator address derived from connection public key (${validatorAddressFromConnection}), but it is not registered as a writer.`
            );
        }

        if (!b4a.isBuffer(account.wk) || !b4a.equals(account.wk, writerKeyFromManifest)) {
            throw new V1ProtocolError(
                ResultCode.VALIDATOR_WRITER_KEY_MISMATCH,
                `Writer key from manifest (${b4a.toString(writerKeyFromManifest, "hex")}) does not match writer key in state for validator address derived from connection public key (${validatorAddressFromConnection}).`
            );
        }
    }

    validateIfResultCodeIsValidatorInternalError(resultCode) {
        if (resultCode === ResultCode.TX_COMMITTED_RECEIPT_MISSING) {
            throw new V1ProtocolError(
                resultCode,
                `Validator response indicates an error with result code ${resultCode}, which is an internal error code.`
            )
        }
    }
}

export function extractRequiredVaFromDecodedTx(validatorDecodedTx) {
    if (!validatorDecodedTx || typeof validatorDecodedTx !== 'object') {
        throw new V1ProtocolError(
            ResultCode.VALIDATOR_TX_OBJECT_INVALID,
            'Invalid decoded transaction: expected object'
        );
    }

    const operationPayload = Object.values(validatorDecodedTx).find((value) =>
        value &&
        typeof value === 'object' &&
        !Array.isArray(value) &&
        !b4a.isBuffer(value) &&
        Object.prototype.hasOwnProperty.call(value, 'va')
    );

    const va = operationPayload?.va;

    if (!b4a.isBuffer(va)) {
        throw new V1ProtocolError(
            ResultCode.VALIDATOR_VA_MISSING,
            'Missing validator address (va) in decoded transaction'
        );
    }
    return va;
}

export default V1BroadcastTransactionResponse;

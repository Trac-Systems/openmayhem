import Validator from 'fastest-validator';
import b4a from 'b4a';
import {
    NetworkOperationType,
    NONCE_BYTE_LENGTH,
    SIGNATURE_BYTE_LENGTH,
    MAX_PARTIAL_TX_PAYLOAD_BYTE_SIZE,
    ResultCode
} from '../../../../../utils/constants.js';

const ALLOWED_RESULT_CODES = Object.values(ResultCode);

class V1ValidationSchema {
    #validator;
    #validateV1LivenessRequest;
    #validateV1LivenessResponse;
    #validateV1BroadcastTransactionRequest;
    #validateV1BroadcastTransactionResponse;

    constructor() {
        this.#validator = new Validator({
            useNewCustomCheckerFunction: true,
            messages: {
                buffer: "The '{field}' field must be a Buffer! Actual: {actual}",
                bufferLength: "The '{field}' field must be a Buffer with length {expected}! Actual: {actual}",
                bufferMinLength: "The '{field}' field must be a Buffer with min length {expected}! Actual: {actual}",
                bufferMaxLength: "The '{field}' field must be a Buffer with max length {expected}! Actual: {actual}",
                nonZeroBuffer: "The '{field}' field must not be an empty or zero-filled Buffer!",
                emptyBuffer: "The '{field}' field must not be an empty Buffer! Actual: {actual}",
            },
        });
        const isBuffer = b4a.isBuffer;
        this.#validator.add("buffer", function ({schema, messages}, _path, _context) {
            const allowZero = schema.allowZero === true;
            const allowEmpty = schema.allowEmpty === true;
            const exactLength = Number.isInteger(schema.length) ? schema.length : null;
            const minLength = Number.isInteger(schema.min) ? schema.min : null;
            const maxLength = Number.isInteger(schema.max) ? schema.max : null;
            return {
                source:
                    `
                        if (!${isBuffer}(value)) {
                            ${this.makeError({type: "buffer", actual: "value", messages})}
                            return value;
                        }
                        const len = value.length;
                        ${exactLength === null ? '' : `
                        if (len !== ${exactLength}) {
                            ${this.makeError({type: "bufferLength", expected: exactLength, actual: "len", messages})}
                        }`}
                        ${minLength === null ? '' : `
                        if (len < ${minLength}) {
                            ${this.makeError({type: "bufferMinLength", expected: minLength, actual: "len", messages})}
                        }`}
                        ${maxLength === null ? '' : `
                        if (len > ${maxLength}) {
                            ${this.makeError({type: "bufferMaxLength", expected: maxLength, actual: "len", messages})}
                        }`}
                        if (len === 0 && !${allowEmpty}) {
                            ${this.makeError({type: "emptyBuffer", actual: "len", messages})}
                            return value;
                        }
                        if (!${allowZero}) {
                            let isZeroFilled = true;
                            for (let i = 0; i < len; i++) {
                                if (value[i] !== 0) {
                                    isZeroFilled = false;
                                    break;
                                }
                            }
                            if (isZeroFilled) {
                                ${this.makeError({type: "nonZeroBuffer", actual: "value", messages})}
                            }
                        }
                            return value;
                    `
            };
        });

        this.#validateV1LivenessRequest = this.#compileV1LivenessRequestSchema();
        this.#validateV1LivenessResponse = this.#compileV1LivenessResponseSchema();
        this.#validateV1BroadcastTransactionRequest = this.#compileV1BroadcastTransactionRequestSchema();
        this.#validateV1BroadcastTransactionResponse = this.#compileV1BroadcastTransactionResponseSchema();
    }

    #compileV1LivenessRequestSchema() {
        const schema = {
            $$strict: true,
            type: {type: 'number', integer: true, equal: NetworkOperationType.LIVENESS_REQUEST, required: true},
            id: {type: 'string', min: 1, max: 64, required: true},
            timestamp: {type: 'number', integer: true, min: 1, max: Number.MAX_SAFE_INTEGER, required: true},
            liveness_request: {
                strict: true,
                type: 'object',
                props: {
                    nonce: {type: 'buffer', length: NONCE_BYTE_LENGTH, required: true},
                    signature: {type: 'buffer', length: SIGNATURE_BYTE_LENGTH, required: true},
                }
            },
            capabilities: {type: 'array', items: 'string', required: true},

        };
        return this.#validator.compile(schema);
    }

    validateV1LivenessRequest(operation) {
        return this.#validateV1LivenessRequest(operation) === true;
    }

    #compileV1LivenessResponseSchema() {
        const schema = {
            $$strict: true,
            type: {type: 'number', integer: true, equal: NetworkOperationType.LIVENESS_RESPONSE, required: true},
            id: {type: 'string', min: 1, max: 64, required: true},
            timestamp: {type: 'number', integer: true, min: 1, max: Number.MAX_SAFE_INTEGER, required: true},
            liveness_response: {
                strict: true,
                type: 'object',
                props: {
                    nonce: {type: 'buffer', length: NONCE_BYTE_LENGTH, required: true},
                    signature: {type: 'buffer', length: SIGNATURE_BYTE_LENGTH, required: true},
                    result: {type: 'enum', values: ALLOWED_RESULT_CODES, required: true},
                }
            },
            capabilities: {type: 'array', items: 'string', required: true},

        };
        return this.#validator.compile(schema);
    }

    validateV1LivenessResponse(operation) {
        return this.#validateV1LivenessResponse(operation) === true;
    }

    #compileV1BroadcastTransactionRequestSchema() {
        const schema = {
            $$strict: true,
            type: {
                type: 'number',
                integer: true,
                equal: NetworkOperationType.BROADCAST_TRANSACTION_REQUEST,
                required: true
            },
            id: {type: 'string', min: 1, max: 64, required: true},
            timestamp: {type: 'number', integer: true, min: 1, max: Number.MAX_SAFE_INTEGER, required: true},
            broadcast_transaction_request: {
                strict: true,
                type: 'object',
                props: {
                    data: {
                        type: 'buffer',
                        min: 1,
                        max: MAX_PARTIAL_TX_PAYLOAD_BYTE_SIZE,
                        allowZero: true,
                        required: true
                    },
                    nonce: {type: 'buffer', length: NONCE_BYTE_LENGTH, required: true},
                    signature: {type: 'buffer', length: SIGNATURE_BYTE_LENGTH, required: true},
                }
            },
            capabilities: {type: 'array', items: 'string', required: true},

        };
        return this.#validator.compile(schema);
    }

    validateV1BroadcastTransactionRequest(operation) {
        return this.#validateV1BroadcastTransactionRequest(operation) === true;
    }

    #compileV1BroadcastTransactionResponseSchema() {
        const schema = {
            $$strict: true,
            type: {
                type: 'number',
                integer: true,
                equal: NetworkOperationType.BROADCAST_TRANSACTION_RESPONSE,
                required: true
            },
            id: {type: 'string', min: 1, max: 64, required: true},
            timestamp: {type: 'number', integer: true, min: 1, max: Number.MAX_SAFE_INTEGER, required: true},
            broadcast_transaction_response: {
                strict: true,
                type: 'object',
                props: {
                    nonce: {type: 'buffer', length: NONCE_BYTE_LENGTH, required: true},
                    signature: {type: 'buffer', length: SIGNATURE_BYTE_LENGTH, required: true},
                    proof: {type: 'buffer', allowEmpty: true, allowZero: true, required: true},
                    timestamp: {
                        type: 'number',
                        integer: true,
                        min: 0,
                        max: Number.MAX_SAFE_INTEGER,
                        optional: true
                    },
                    result: {type: 'enum', values: ALLOWED_RESULT_CODES, required: true},
                }
            },
            capabilities: {type: 'array', items: 'string', required: true},

        };
        return this.#validator.compile(schema);
    }

    validateV1BroadcastTransactionResponse(operation) {
        return this.#validateV1BroadcastTransactionResponse(operation) === true;
    }
}

export default V1ValidationSchema;

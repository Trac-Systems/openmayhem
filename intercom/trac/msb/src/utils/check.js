import Validator from 'fastest-validator';
import b4a from 'b4a';

import {
    OperationType,
    WRITER_BYTE_LENGTH,
    NONCE_BYTE_LENGTH,
    SIGNATURE_BYTE_LENGTH,
    HASH_BYTE_LENGTH,
    BOOTSTRAP_BYTE_LENGTH,
    CHANNEL_BYTE_LENGTH,
    AMOUNT_BYTE_LENGTH,
} from './constants.js';
//TODO: ATTENTION - CURRENT IMPLEMENTATION UTILIZES `custom` FOR MULTIPLE TIMES IN MANY SCHEMAS. IT SHOULD BE CLEANED UP
// TO UTILIZE ONLY ONE FUNCTION COMMON FOR ALL THE SCHEMAS. CREATE A TICKED P2/P3.

class Check {
    #validator;
    #validateCoreAdminOperationSchema
    #validateAdminControlOperationSchema
    #validateRoleAccessOperationSchema
    #validateBootstrapDeploymentSchema;
    #validateTransactionOperationSchema
    #validateTransferOperationSchema
    #validateBalanceInitializationSchema
    #config

    /**
     * @param {object} config
     **/
    constructor(config) {
        this.#config = config

        this.#validator = new Validator({
            useNewCustomCheckerFunction: true,
            messages: {
                buffer: "The '{field}' field must be a Buffer! Actual: {actual}",
                bufferLength: "The '{field}' field must be a Buffer with length {expected}! Actual: {actual}",
                nonZeroBuffer: "The '{field}' field must not be an empty or zero-filled Buffer!",
                emptyBuffer: "The '{field}' field must not be an empty Buffer!",
            },
        });
        const isBuffer = b4a.isBuffer;
        this.#validator.add("buffer", function ({ schema, messages }, path, context) {
            return {
                source:
                    `
                        if (!${isBuffer}(value)) {
                            ${this.makeError({ type: "buffer", actual: "value", messages })}
                        }
                        if (value.length !== ${schema.length}) {
                            ${this.makeError({
                        type: "bufferLength",
                        expected: schema.length,
                        actual: "value.length",
                        messages
                    })}
                        }
                        let isEmpty = true;
                            for (let i = 0; i < value.length; i++) {
                                if (value[i] !== 0) {
                                    isEmpty = false;
                                    break;
                                }
                            }
                            if (isEmpty) {
                                ${this.makeError({ type: "emptyBuffer", actual: "value", messages })}
                            }
                            return value;
                    `
            };
        });

        this.#validator.add("buffer_amount", function ({ schema, messages }, path, context) {
            return {
                source:
                    `
                        if (!${isBuffer}(value)) {
                            ${this.makeError({ type: "buffer", actual: "value", messages })}
                        }
                        if (value.length !== ${schema.length}) {
                            ${this.makeError({
                        type: "bufferLength",
                        expected: schema.length,
                        actual: "value.length",
                        messages
                    })}
                        }
                        return value;
                    `
            };
        });


        this.#validateCoreAdminOperationSchema = this.#compileCoreAdminOperationSchema();
        this.#validateAdminControlOperationSchema = this.#compileAdminControlOperationSchema();
        this.#validateRoleAccessOperationSchema = this.#compileRoleAccessOperationSchema();
        this.#validateBootstrapDeploymentSchema = this.#compileBootstrapDeploymentSchema();
        this.#validateTransactionOperationSchema = this.#compileTransactionOperationSchema();
        this.#validateTransferOperationSchema = this.#compileTransferOperationSchema();
        this.#validateBalanceInitializationSchema = this.#compileBalanceInitializationSchema();

    }

    // Complete by default - no writer needed
    #compileCoreAdminOperationSchema() {
        const addressLength = this.#config.addressLength
        const schema = {
            $$strict: true,
            type: {
                type: 'number',
                required: true,
                custom: (value, errors) => {
                    const allowedTypes = [
                        OperationType.ADD_ADMIN,
                        OperationType.DISABLE_INITIALIZATION,
                    ];

                    if (!allowedTypes.includes(value)) {
                        errors.push({
                            type: 'valueNotAllowed',
                            actual: value,
                            expected: allowedTypes,
                            field: 'type',
                            message: `Operation type must be one of: ${allowedTypes.join(', ')}`
                        });
                    }
                    return value;
                }
            },
            address: {type: 'buffer', length: addressLength, required: true}, // invoker adddress (admin)
            cao: {
                strict: true,
                type: 'object',
                props: {
                    tx: { type: 'buffer', length: HASH_BYTE_LENGTH, required: true }, // tx hash
                    txv: { type: 'buffer', length: HASH_BYTE_LENGTH, required: true }, // tx validity
                    iw: { type: 'buffer', length: WRITER_BYTE_LENGTH, required: true }, // writer key of the admin
                    in: { type: 'buffer', length: NONCE_BYTE_LENGTH, required: true }, // nonce
                    is: { type: 'buffer', length: SIGNATURE_BYTE_LENGTH, required: true }, // signature
                }
            }
        };
        return this.#validator.compile(schema);
    }

    validateCoreAdminOperation(operation) {
        return this.#validateCoreAdminOperationSchema(operation) === true;
    }

    #compileBalanceInitializationSchema() {
        const schema = {
            $$strict: true,
            type: {
                type: 'number',
                required: true,
                custom: (value, errors) => {
                    if (value !== OperationType.BALANCE_INITIALIZATION) {
                        errors.push({
                            type: 'valueNotAllowed',
                            actual: value,
                            expected: OperationType.BALANCE_INITIALIZATION,
                            field: 'type',
                            message: `Operation type must be ${OperationType.BALANCE_INITIALIZATION} (BALANCE_INITIALIZATION)`
                        });
                    }
                    return value;
                }
            },
            address: { type: 'buffer', length: this.#config.addressLength, required: true },
            bio: {
                strict: true,
                type: 'object',
                props: {
                    tx: { type: 'buffer', length: HASH_BYTE_LENGTH, required: true }, // tx hash
                    txv: { type: 'buffer', length: HASH_BYTE_LENGTH, required: true }, // tx validity
                    ia: { type: 'buffer', length: this.#config.addressLength, required: true }, // selected address to specific operation.
                    am: { type: 'buffer', length: AMOUNT_BYTE_LENGTH, required: true }, // amount to transfer
                    in: { type: 'buffer', length: NONCE_BYTE_LENGTH, required: true }, // nonce of the invoker
                    is: { type: 'buffer', length: SIGNATURE_BYTE_LENGTH, required: true }, // signature of the invoker
                }
            }
        };
        return this.#validator.compile(schema);
    }

    validateBalanceInitialization(operation) {
        return this.#validateBalanceInitializationSchema(operation) === true;
    }

    // Complete by default - no writer needed
    #compileAdminControlOperationSchema() {
        const schema = {
            $$strict: true,
            type: {
                type: 'number',
                required: true,
                custom: (value, errors) => {
                    const allowedTypes = [
                        OperationType.APPEND_WHITELIST,
                        OperationType.ADD_INDEXER,
                        OperationType.REMOVE_INDEXER,
                        OperationType.BAN_VALIDATOR
                    ];

                    if (!allowedTypes.includes(value)) {
                        errors.push({
                            type: 'valueNotAllowed',
                            actual: value,
                            expected: allowedTypes,
                            field: 'type',
                            message: `Operation type must be one of: ${allowedTypes.join(', ')}`
                        });
                    }
                    return value;
                }
            },
            address: {type: 'buffer', length: this.#config.addressLength, required: true}, // invoker adddress (admin)
            aco: {
                strict: true,
                type: 'object',
                props: {
                    tx: { type: 'buffer', length: HASH_BYTE_LENGTH, required: true }, // tx hash
                    txv: { type: 'buffer', length: HASH_BYTE_LENGTH, required: true }, // tx validity
                    ia: { type: 'buffer', length: this.#config.addressLength, required: true }, // incoming address - selected address for specific operation
                    in: { type: 'buffer', length: NONCE_BYTE_LENGTH, required: true }, // nonce
                    is: { type: 'buffer', length: SIGNATURE_BYTE_LENGTH, required: true }, // signature
                }
            }
        };
        return this.#validator.compile(schema);
    }

    validateAdminControlOperation(operation) {
        return this.#validateAdminControlOperationSchema(operation) === true;
    }

    #compileRoleAccessOperationSchema() {
        const schema = {
            $$strict: true,
            type: {
                type: 'number',
                required: true,
                custom: (value, errors) => {
                    const allowedTypes = [
                        OperationType.ADD_WRITER,
                        OperationType.REMOVE_WRITER,
                        OperationType.ADMIN_RECOVERY
                    ];

                    if (!allowedTypes.includes(value)) {
                        errors.push({
                            type: 'valueNotAllowed',
                            actual: value,
                            expected: allowedTypes,
                            field: 'type',
                            message: `Operation type must be one of: ${allowedTypes.join(', ')}`
                        });
                    }
                    return value;
                }
            },
            address: {type: 'buffer', length: this.#config.addressLength, required: true},
            rao: {
                strict: true,
                type: 'object',
                props: {
                    tx: { type: 'buffer', length: HASH_BYTE_LENGTH, required: true }, // tx hash
                    txv: { type: 'buffer', length: HASH_BYTE_LENGTH, required: true }, // tx validity
                    iw: { type: 'buffer', length: WRITER_BYTE_LENGTH, required: true }, // writing key of the invoker
                    in: { type: 'buffer', length: NONCE_BYTE_LENGTH, required: true }, // nonce of the invoker
                    is: { type: 'buffer', length: SIGNATURE_BYTE_LENGTH, required: true }, // signature
                    va: { type: 'buffer', length: this.#config.addressLength, optional: true },
                    vn: { type: 'buffer', length: NONCE_BYTE_LENGTH, optional: true },
                    vs: { type: 'buffer', length: SIGNATURE_BYTE_LENGTH, optional: true }

                },
                custom: (value, errors) => {
                    if (!value || typeof value !== 'object') return value;
                    const { vn, vs, va } = value;
                    const vnPresent = vn !== undefined
                    const vsPresent = vs !== undefined
                    const vaPresent = va !== undefined

                    const fieldsPresent = [vnPresent, vsPresent, vaPresent].filter(Boolean).length;

                    if (fieldsPresent > 0 && fieldsPresent < 3) {
                        errors.push({
                            type: 'conditionalDependency',
                            field: 'bdo',
                            message: 'Fields "vn", "vs", and "va" must all be present if any one is provided'
                        });
                    }
                    if (vn === null || vs === null || va === null) {
                        errors.push({
                            type: 'buffer',
                            field: 'bdo',
                            message: 'Validator fields cannot be null, must be a Buffer or undefined'
                        });
                    }

                    return value;
                }
            }
        };
        return this.#validator.compile(schema);
    }

    validateRoleAccessOperation(operation) {
        return this.#validateRoleAccessOperationSchema(operation) === true;
    }

    #compileTransactionOperationSchema() {
        const schema = {
            $$strict: true,
            type: {
                type: 'number',
                required: true,
                custom: (value, errors) => { // more secure than enum
                    if (value !== OperationType.TX) {
                        errors.push({
                            type: 'valueNotAllowed',
                            actual: value,
                            expected: OperationType.TX,
                            field: 'type',
                            message: `Operation type must be ${OperationType.TX} (TX)`
                        });
                    }
                    return value;
                }
            },
            address: {type: 'buffer', length: this.#config.addressLength, required: true}, // invoker address
            txo: {
                strict: true,
                type: 'object',
                props: {
                    tx: { type: 'buffer', length: HASH_BYTE_LENGTH, required: true }, // tx hash
                    txv: { type: 'buffer', length: HASH_BYTE_LENGTH, required: true }, // tx validity
                    iw: { type: 'buffer', length: WRITER_BYTE_LENGTH, required: true }, // Writing key of the requesting node (external subnetwork)
                    ch: { type: 'buffer', length: NONCE_BYTE_LENGTH, required: true }, // Content hash (hash of the transaction's data)
                    bs: { type: 'buffer', length: BOOTSTRAP_BYTE_LENGTH, required: true }, // External bootstrap contract
                    mbs: { type: 'buffer', length: BOOTSTRAP_BYTE_LENGTH, required: true }, // MSB bootstrap key
                    in: { type: 'buffer', length: NONCE_BYTE_LENGTH, required: true }, // Nonce of the requesting node
                    is: { type: 'buffer', length: SIGNATURE_BYTE_LENGTH, required: true }, // Requester's signature
                    va: { type: 'buffer', length: this.#config.addressLength, optional: true }, //validator address
                    vn: { type: 'buffer', length: NONCE_BYTE_LENGTH, optional: true }, //validator nonce
                    vs: { type: 'buffer', length: SIGNATURE_BYTE_LENGTH, optional: true }, //validator signature
                },
                custom: (value, errors) => {
                    if (!value || typeof value !== 'object') return value;
                    const { vn, vs, va } = value;
                    const vnPresent = vn !== undefined;
                    const vsPresent = vs !== undefined;
                    const vaPresent = va !== undefined;

                    const fieldsPresent = [vnPresent, vsPresent, vaPresent].filter(Boolean).length;

                    if (fieldsPresent > 0 && fieldsPresent < 3) {
                        errors.push({
                            type: 'conditionalDependency',
                            field: 'bdo',
                            message: 'Fields "vn", "vs", and "va" must all be present if any one is provided'
                        });
                    }
                    if (vn === null || vs === null || va === null) {
                        errors.push({
                            type: 'buffer',
                            field: 'bdo',
                            message: 'Validator fields cannot be null, must be a Buffer or undefined'
                        });
                    }

                    return value;
                }
            }
        };
        return this.#validator.compile(schema);
    }

    validateTransactionOperation(op) {
        return this.#validateTransactionOperationSchema(op) === true;
    }

    #compileBootstrapDeploymentSchema() {
        const schema = {
            $$strict: true,
            type: {
                type: 'number',
                required: true,
                custom: (value, errors) => {
                    if (value !== OperationType.BOOTSTRAP_DEPLOYMENT) {
                        errors.push({
                            type: 'valueNotAllowed',
                            actual: value,
                            expected: OperationType.BOOTSTRAP_DEPLOYMENT,
                            field: 'type',
                            message: `Operation type must be ${OperationType.BOOTSTRAP_DEPLOYMENT} (BOOTSTRAP_DEPLOYMENT)`
                        });
                    }
                    return value;
                }
            },
            address: {type: 'buffer', length: this.#config.addressLength, required: true},
            bdo: {

                strict: true,
                type: "object",
                props: {
                    tx: { type: 'buffer', length: HASH_BYTE_LENGTH, required: true },
                    txv: { type: 'buffer', length: HASH_BYTE_LENGTH, required: true },
                    bs: { type: 'buffer', length: BOOTSTRAP_BYTE_LENGTH, required: true },
                    ic: { type: 'buffer', length: CHANNEL_BYTE_LENGTH, required: true },
                    in: { type: 'buffer', length: NONCE_BYTE_LENGTH, required: true },
                    is: { type: 'buffer', length: SIGNATURE_BYTE_LENGTH, required: true },
                    va: { type: 'buffer', length: this.#config.addressLength, optional: true },
                    vn: { type: 'buffer', length: NONCE_BYTE_LENGTH, optional: true },
                    vs: { type: 'buffer', length: SIGNATURE_BYTE_LENGTH, optional: true },
                },
                custom: (value, errors) => {
                    if (!value || typeof value !== 'object') return value;
                    const { vn, vs, va } = value;
                    const vnPresent = vn !== undefined
                    const vsPresent = vs !== undefined
                    const vaPresent = va !== undefined

                    const fieldsPresent = [vnPresent, vsPresent, vaPresent].filter(Boolean).length;

                    if (fieldsPresent > 0 && fieldsPresent < 3) {
                        errors.push({
                            type: 'conditionalDependency',
                            field: 'bdo',
                            message: 'Fields "vn", "vs", and "va" must all be present if any one is provided'
                        });
                    }
                    if (vn === null || vs === null || va === null) {
                        errors.push({
                            type: 'buffer',
                            field: 'bdo',
                            message: 'Validator fields cannot be null, must be a Buffer or undefined'
                        });
                    }

                    return value;
                }
            }
        };
        return this.#validator.compile(schema);
    }

    validateBootstrapDeploymentOperation(op) {
        return this.#validateBootstrapDeploymentSchema(op) === true;
    }

    #compileTransferOperationSchema() {
        const schema = {
            $$strict: true,
            type: {
                type: 'number',
                required: true,
                custom: (value, errors) => {
                    if (value !== OperationType.TRANSFER) {
                        errors.push({
                            type: 'valueNotAllowed',
                            actual: value,
                            expected: OperationType.TRANSFER,
                            field: 'type',
                            message: `Operation type must be ${OperationType.TRANSFER} (TRANSFER)`
                        });
                    }
                    return value;
                }
            },
            address: {type: 'buffer', length: this.#config.addressLength, required: true},
            tro: {
                strict: true,
                type: 'object',
                props: {
                    tx: { type: 'buffer', length: HASH_BYTE_LENGTH, required: true }, // tx hash
                    txv: { type: 'buffer', length: HASH_BYTE_LENGTH, required: true }, // tx validity
                    to: { type: 'buffer', length: this.#config.addressLength, required: true }, // recipient address
                    am: { type: 'buffer_amount', length: AMOUNT_BYTE_LENGTH, required: true }, // amount to transfer
                    in: { type: 'buffer', length: NONCE_BYTE_LENGTH, required: true }, // nonce of the invoker
                    is: { type: 'buffer', length: SIGNATURE_BYTE_LENGTH, required: true }, // signature of the invoker
                    va: { type: 'buffer', length: this.#config.addressLength, optional: true },  // validator address
                    vn: { type: 'buffer', length: NONCE_BYTE_LENGTH, optional: true },  // validator nonce
                    vs: { type: 'buffer', length: SIGNATURE_BYTE_LENGTH, optional: true } // validator signature

                },
                custom: (value, errors) => {
                    if (!value || typeof value !== 'object') return value;
                    const { vn, vs, va } = value;
                    const vnPresent = vn !== undefined
                    const vsPresent = vs !== undefined
                    const vaPresent = va !== undefined

                    const fieldsPresent = [vnPresent, vsPresent, vaPresent].filter(Boolean).length;

                    if (fieldsPresent > 0 && fieldsPresent < 3) {
                        errors.push({
                            type: 'conditionalDependency',
                            field: 'tro',
                            message: 'Fields "vn", "vs", and "va" must all be present if any one is provided'
                        });
                    }
                    if (vn === null || vs === null || va === null) {
                        errors.push({
                            type: 'buffer',
                            field: 'tro',
                            message: 'Validator fields cannot be null, must be a Buffer or undefined'
                        });
                    }

                    return value;
                }
            }
        };
        return this.#validator.compile(schema);
    }

    validateTransferOperation(op) {
        return this.#validateTransferOperationSchema(op) === true;
    }
}

export default Check;


import applyOperations from './applyOperations.cjs';
import networkV1Generated from './networkV1.generated.cjs';
import b4a from 'b4a';

const networkV1Operations = networkV1Generated.network.v1;
const NETWORK_TO_OBJECT_OPTIONS = Object.freeze({
    enums: Number,
    longs: Number,
    bytes: Buffer,
    defaults: true,
    arrays: true,
    oneofs: false
});

/**
 * Safely encodes an operation using `applyOperations.Operation.encode`.
 * If the encoding fails (e.g., due to an invalid payload), returns an empty Buffer.
 *
 * @param {*} payload - Any input that should conform to the `applyOperation` schema.
 * @returns {Buffer} - Encoded Buffer if successful, otherwise an empty Buffer (`b4a.alloc(0)`).
 */
export const safeEncodeApplyOperation = (payload) => {
    try {
        const result = applyOperations.Operation.encode(payload);
        if (b4a.isBuffer(result)) return result
    } catch (error) {
        console.log("safeEncodeApplyOperation error:", error.message);
    }
    return b4a.alloc(0);
}

/**
 * Safely decodes a Buffer into an `Operation` object using `applyOperations.Operation.decode`.
 * Returns `null` if decoding fails or the input is invalid.
 *
 * @param {Buffer} payload - A buffer containing encoded data.
 * @returns {Object|null} - Decoded `applyOperation` object on success, or `null` on failure.
 */
export const safeDecodeApplyOperation = (payload) => {
    try {
        if (!b4a.isBuffer(payload)) return null;
        return applyOperations.Operation.decode(payload);
    } catch (error) {
        console.log(error);
    }
    return null;
}

export const unsafeDecodeApplyOperation= (payload) => {
    return applyOperations.Operation.decode(payload);
}

export const unsafeEncodeApplyOperation = (payload) => {
    return applyOperations.Operation.encode(payload);
}

export const normalizeIncomingMessage = (message) => {
    if (!message) return null;
    if (b4a.isBuffer(message)) {
        return applyOperations.Operation.decode(message);
    }

    if (message.type === 'Buffer' && Array.isArray(message.data)) {
        const buffer = b4a.from(message.data);
        return applyOperations.Operation.decode(buffer);
    }

    return null;
};

export const encodeV1networkOperation = (payload) => {
    return b4a.from(networkV1Operations.MessageHeader.encode(payload).finish());
}


export const decodeV1networkOperation = (payload) => {
    return networkV1Operations.MessageHeader.toObject(
        networkV1Operations.MessageHeader.decode(payload),
        NETWORK_TO_OBJECT_OPTIONS
    );
}

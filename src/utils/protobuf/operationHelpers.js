import applyOperations from './applyOperations.cjs';
import networkV1Operations from './network.cjs';
import b4a from 'b4a';

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
    return networkV1Operations.MessageHeader.encode(payload);
}


export const decodeV1networkOperation = (payload) => {
    return networkV1Operations.MessageHeader.decode(payload);
}

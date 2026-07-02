import b4a from 'b4a';
import { bigIntTo16ByteBuffer } from './amountSerialization.js';

export const ZERO_WK = b4a.alloc(32, 0); // 32 bytes of zeroes, used as a placeholder for writing keys
export const NULL_BUFFER = b4a.alloc(0) // null buffer (single byte of 0)

const isUInt32 = (n) => { return Number.isInteger(n) && n >= 1 && n <= 0xFFFFFFFF; }

export function isBufferValid(key, size) {
    return b4a.isBuffer(key) && key.length === size;
}

export const safeWriteUInt32BE = (value, offset) => {
    try {
        const buf = b4a.alloc(4);
        buf.writeUInt32BE(value, offset);
        return buf;
    } catch (error) {
        return b4a.alloc(4);
    }
}

export const createMessage = (...args) => {

    if (args.length === 0) return b4a.alloc(0);

    const buffers = args.map(arg => {
        if (b4a.isBuffer(arg)) {
            return arg;
        } else if (typeof arg === 'number' && isUInt32(arg)) {
            return safeWriteUInt32BE(arg, 0);
        }
    }).filter(buf => b4a.isBuffer(buf));

    if (buffers.length === 0) return b4a.alloc(0);
    return b4a.concat(buffers);
}

export function normalizeBuffer(message) {
    if (b4a.isBuffer(message)) {
        return message;
    }

    if (message.type === 'Buffer' && Array.isArray(message.data)) {
        return b4a.from(message.data);
    }

    if (typeof message === 'object' && Object.keys(message).every(key => !isNaN(key))) {
        return b4a.from(Object.values(message));
    }

    return null;
}

export const bigIntToBuffer = bigIntTo16ByteBuffer

export function deepCopyBuffer(buffer) {
    if (!buffer) return null;
    const copy = b4a.alloc(buffer.length);
    buffer.copy(copy);
    return copy;
}

function uint64ToBuffer(value, fieldName) {
    if (typeof value === 'number') {
        if (!Number.isSafeInteger(value) || value < 0) {
            throw new Error(`${fieldName} must be a non-negative safe integer`);
        }
        value = BigInt(value);
    } else if (typeof value !== 'bigint') {
        throw new Error(`${fieldName} must be a number or bigint`);
    }
    if (value < 0n) {
        throw new Error(`${fieldName} must be a non-negative integer`);
    }

    const buf = b4a.alloc(8);
    buf.writeBigUInt64BE(value);
    return buf;
}

export function timestampToBuffer(timestamp) {
    return uint64ToBuffer(timestamp, 'timestamp');
}

export function idToBuffer(id) {
    if (typeof id !== 'string') {
        throw new Error('id must be a string');
    }
    return b4a.from(id, 'utf8');
}


export function encodeCapabilities(capabilities) {
    if (!Array.isArray(capabilities)) {
        throw new Error('Capabilities must be an array');
    }
    const validCapabilities = capabilities.map((capability) => {
        if (typeof capability !== 'string') {
            throw new Error('Capabilities array must contain only strings');
        }
        return capability;
    });

    const parts = [];
    for (const capability of validCapabilities.slice().sort()) {
        const capabilityBuffer = b4a.from(capability, 'utf8');
        const bufferLen = b4a.allocUnsafe(2);
        bufferLen.writeUInt16BE(capabilityBuffer.length, 0);
        parts.push(bufferLen, capabilityBuffer);
    }

    return parts.length ? b4a.concat(parts) : b4a.alloc(0);
}

import b4a from "b4a";
import { LICENSE_BYTE_LENGTH } from "./constants.js";

// FUNCTUIONS TO SERIALIZE AND DESERIALIZE AMOUNTS ONLY ON CLI LEVEL. ATTENTION DO NOT USE IT ON PROTOCOL LEVEL
export function decimalStringToBigInt(inputString, decimals = 18) {
    if (typeof inputString !== 'string') {
        throw new TypeError('Input must be a string');
    }

    const trimmedInput = inputString.trim();

    // Check for negative numbers first
    if (trimmedInput.startsWith('-')) {
        throw new Error('Negative amounts are not allowed');
    }

    if (!/^\d+(\.\d+)?$/.test(trimmedInput)) {
        throw new Error('Invalid decimal format. Use format: 123.456');
    }

    const parts = trimmedInput.split('.');
    const integerPart = parts[0];
    let fractionalPart = parts[1] || '';
    if (fractionalPart.length > decimals) {
        throw new Error(`Too many decimal places. Maximum allowed: ${decimals}`);
    }
    
    fractionalPart = fractionalPart.padEnd(decimals, '0');
    const fullNumberString = integerPart + fractionalPart;

    try {
        const value = BigInt(fullNumberString);
        // Check if the value exceeds maximum allowed (2^128 - 1)
        if (value > (2n ** 128n - 1n)) {
            throw new Error('Amount exceeds maximum allowed value (2^128 - 1)');
        }
        return value;
    } catch (error) {
        if (error.message.includes('exceeds maximum')) {
            throw error;
        }
        throw new Error(`Failed to convert to BigInt: ${error.message}`);
    }
}

export function bigIntTo16ByteBuffer(bigIntValue) {
    if (typeof bigIntValue !== 'bigint') {
        throw new TypeError('Input must be a BigInt');
    }

    const value = BigInt.asUintN(128, bigIntValue);
    const buff = b4a.alloc(16);

    let tmp = value;
    for (let i = 15; i >= 0; i--) {
        buff[i] = Number(tmp & 0xffn);
        tmp = tmp >> 8n;
    }
    return buff;
}

export function bufferToBigInt(buff) {
    if (!b4a.isBuffer(buff) || buff.length !== 16) {
        throw new TypeError('Input must be a 16-byte Buffer');
    }

    let res = 0n;
    for (let i = 0; i < 16; i++) {
        res = (res << 8n) | BigInt(buff[i]);
    }

    return res;
}

export function licenseBufferToBigInt(buff) {
    if (!b4a.isBuffer(buff) || buff.length !== LICENSE_BYTE_LENGTH) {
        throw new TypeError('Input must be a 4-byte Buffer');
    }

    let result = 0n;
    for (let i = 0; i < buff.length; i++) {
        const shift = 8n * BigInt(buff.length - 1 - i); 
        result += BigInt(buff[i]) << shift;
    }

    return result;
}

export function bigIntToDecimalString(bigIntValue, decimals = 18) {
    if (typeof bigIntValue !== 'bigint') {
        throw new TypeError('Input must be a BigInt');
    }
    if (!Number.isInteger(decimals) || decimals < 0) {
        throw new TypeError('Decimals must be a non-negative integer');
    }
    
    if (bigIntValue < 0n) {
        throw new Error('Negative amounts are not allowed');
    }
    if (bigIntValue > (2n ** 128n - 1n)) {
        throw new Error('Amount exceeds maximum allowed value (2^128 - 1)');
    }
    const str = bigIntValue.toString().padStart(decimals + 1, '0');
    if (str.length <= decimals) {
        throw new Error(`Value too small for given decimals (${decimals})`);
    }
    const integerPart = str.slice(0, -decimals) || '0';
    const fractionalPart = str.slice(-decimals).replace(/0+$/, '');
    return fractionalPart.length > 0 ? `${integerPart}.${fractionalPart}` : integerPart;
}
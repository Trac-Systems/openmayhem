import b4a from 'b4a';

import { bufferToAddress, isAddressValid } from './address.js';
import { WRITER_BYTE_LENGTH } from '../../../utils/constants.js';
import { isBufferValid } from '../../../utils/buffer.js';
import { address as addressApi } from 'trac-crypto-api';

/**
 * Encodes an admin entry as a buffer containing the TRAC address and writing key.
 * 
 * The buffer format is: [TRAC_ADDRESS][WRITING_KEY(32)]
 * Where TRAC_ADDRESS is a bech32m-encoded address without the HRP and separator.
 *
 * @param {Buffer} address - The admin address.
 * @param {Buffer} wk - The admin's writing key buffer (must be 32 bytes).
 * @param {string} addressHrp - The HRP of the bech32m address.
 * @returns {Buffer} The encoded admin entry buffer, or an empty buffer if input is invalid.
 */
export function encode(address, wk, addressHrp) {
    try {
        if (!isAddressValid(address, addressHrp) ||
            !isBufferValid(wk, WRITER_BYTE_LENGTH)) {
            throw new Error('Invalid address or writing key buffer');
        }
        const adminEntry = b4a.alloc(address.length + WRITER_BYTE_LENGTH);
        b4a.copy(address, adminEntry, 0);
        b4a.copy(wk, adminEntry, address.length);
        return adminEntry;
    } catch (error) {
        console.error('Error when encoding admin entry:', error);
        return b4a.alloc(0);
    }
}

/**
 * Decodes an admin entry buffer into its TRAC address and writing key components.
 *
 * The buffer format is: [TRAC_ADDRESS][WRITING_KEY(32)]
 * Where TRAC_ADDRESS is a bech32m-encoded address without the HRP and separator.
 *
 * @param {Buffer} adminEntry - The encoded admin entry buffer.
 * @param {string} addressHrp - The HRP of the bech32m address.
 * @returns {Object | null} An object with:
 *   - address: String containing the TRAC address.
 *   - wk: Buffer containing the writing key.
 */
export function decode(adminEntry, addressHrp) {
    try {
        const addressLength = addressApi.size(addressHrp)
        if (!isBufferValid(adminEntry, addressLength + WRITER_BYTE_LENGTH)) {
            return null;
        }

        const addressPart = adminEntry.subarray(0, addressLength);
        const address = bufferToAddress(addressPart, addressHrp);
        const wk = adminEntry.subarray(addressLength);
        return { address, wk };
    }
    catch (error) {
        console.error('Error decoding admin entry:', error);
        return null;
    }
}

export default {
    encode,
    decode
};
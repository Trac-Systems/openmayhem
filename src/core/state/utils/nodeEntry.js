import b4a from 'b4a';

import { WRITER_MASK, INDEXER_MASK, WHITELISTED_MASK, calculateNodeRole, isNodeRoleValid } from './roles.js';
import { WRITER_BYTE_LENGTH, BALANCE_BYTE_LENGTH, LICENSE_BYTE_LENGTH } from '../../../utils/constants.js';
import { isBufferValid } from '../../../utils/buffer.js';

export const NODE_ENTRY_SIZE = LICENSE_BYTE_LENGTH + WRITER_BYTE_LENGTH + 2 * BALANCE_BYTE_LENGTH + 1;
export const ZERO_BALANCE = b4a.alloc(BALANCE_BYTE_LENGTH);
export const ZERO_LICENSE = b4a.alloc(LICENSE_BYTE_LENGTH);


/**
 * Initializes a new node entry with given writing key and role and the balance is set to zero.
 * Creates a buffer in format: [NODE_ROLE_MASK(1)][WRITING_KEY(32)][BALANCE(16)][LICENSE(4)][STAKED_BALANCE(16)]
 *
 * @param {Buffer} writingKey - The writing key for the node (must be 32 bytes)
 * @param {number} role - Initial role from NodeRole enum 
 * @param {Buffer} balance - Initial balance from node (must be 16 bytes)
 * @param {Buffer} license - Initial license from node (must be 4 bytes)
 * @param {Buffer} stakedBalance - Initial staked balance from node (must be 16 bytes)
 * @returns {Buffer} The initialized node entry buffer, or empty buffer if invalid input
 */
export function init(writingKey, role, balance = ZERO_BALANCE, license = ZERO_LICENSE, stakedBalance = ZERO_BALANCE) {
    if (!isBufferValid(writingKey, WRITER_BYTE_LENGTH) ||
        !isBufferValid(balance, BALANCE_BYTE_LENGTH) ||
        !isNodeRoleValid(role) ||
        !isBufferValid(license, LICENSE_BYTE_LENGTH) ||
        !isBufferValid(stakedBalance, BALANCE_BYTE_LENGTH)) {
        console.error('Invalid input for node initialization');
        return b4a.alloc(0);
    }

    try {
        const nodeEntry = b4a.alloc(NODE_ENTRY_SIZE);
        nodeEntry[0] = role;
        let offset = 1;

        b4a.copy(writingKey, nodeEntry, offset);
        offset += WRITER_BYTE_LENGTH;

        b4a.copy(balance, nodeEntry, offset);
        offset += BALANCE_BYTE_LENGTH;

        b4a.copy(license, nodeEntry, offset);
        offset += LICENSE_BYTE_LENGTH;

        b4a.copy(stakedBalance, nodeEntry, offset);

        return nodeEntry;
    } catch (error) {
        console.error('Error initializing node entry:', error);
        return b4a.alloc(0);
    }
}

/**
 * Encodes a node entry as a buffer containing the node's role mask and writing key.
 *
 * The node entry buffer format is:
 *   [NODE_ROLE_MASK(1)][WRITING_KEY(32)][BALANCE(16)][LICENSE(4)][STAKED_BALANCE(16)]
 *   - The first byte is a bitmask representing the node's roles (whitelisted, writer, indexer).
 *   - 32 bytes are the node's writing key.
 *   - 16 bytes are the node's balance.
 *   - 4 bytes are the node's license.
 *   - 16 bytes are the node's staked balance.
 *
 * @param {Object} node - An object representing the node, with properties:
 *   - wk: Buffer containing the node's writing key (must be 32 bytes).
 *   - isWhitelisted: Boolean indicating if the node is whitelisted.
 *   - isWriter: Boolean indicating if the node is a writer.
 *   - isIndexer: Boolean indicating if the node is an indexer.
 *   - balance: Buffer indicating the node balance.
 *   - license: Buffer indicating the node license.
 *   - stakedBalance: Buffer indicating the validator staked balance.
 * @returns {Buffer} The encoded node entry buffer, or an empty buffer if input is invalid.
 */
export function encode(node) {
    const nodeRole = calculateNodeRole(node);
    if (!isBufferValid(node.wk, WRITER_BYTE_LENGTH) ||
        !isBufferValid(node.balance, BALANCE_BYTE_LENGTH) ||
        !isNodeRoleValid(nodeRole) ||
        !isBufferValid(node.license, LICENSE_BYTE_LENGTH) ||
        !isBufferValid(node.stakedBalance, BALANCE_BYTE_LENGTH)
    ) {
        return b4a.alloc(0); // Return an empty buffer if one of the inputs is invalid
    }

    try {
        let entry = b4a.alloc(NODE_ENTRY_SIZE);
        entry[0] = nodeRole;
        let offset = 1;

        b4a.copy(node.wk, entry, offset);
        offset += WRITER_BYTE_LENGTH;

        b4a.copy(node.balance, entry, offset);
        offset += BALANCE_BYTE_LENGTH;

        b4a.copy(node.license, entry, offset);
        offset += LICENSE_BYTE_LENGTH;

        b4a.copy(node.stakedBalance, entry, offset);
        return entry;
    }
    catch (error) {
        console.error('Error encoding node entry:', error);
        return b4a.alloc(0); // Return an empty buffer on error
    }
}

/**
 * Decodes a node entry buffer into an object with its writing key and role flags.
 *
 * The node entry buffer format is:
 *   [NODE_ROLE_MASK(1)][WRITING_KEY(32)][BALANCE(16)][LICENSE(4)][STAKED_BALANCE(16)]
 *   - The first byte is a bitmask indicating the node's roles (whitelisted, writer, indexer).
 *   - The remaining bytes are the node's writing key.
 *
 * @param {Buffer} nodeEntry - The encoded node entry buffer.
 * @returns {Object|null} An object with:
 *   - wk: Buffer containing the writing key.
 *   - isWhitelisted: Boolean indicating if the node is whitelisted.
 *   - isWriter: Boolean indicating if the node is a writer.
 *   - isIndexer: Boolean indicating if the node is an indexer.
 *   - balance: Buffer representing the balance in its atomic unit.
 *   - license: Buffer representing the license ID
 *   - stakedBalance: Buffer representing the staked balance in its atomic unit.
 *   Returns null if the buffer is invalid or an error occurs.
 */
export function decode(nodeEntry) {
    if (!isBufferValid(nodeEntry, NODE_ENTRY_SIZE)) {
        return null;
    }

    try {
        const role = nodeEntry[0];
        let offset = 1;

        const isWhitelisted = !!(role & WHITELISTED_MASK);
        const isWriter = !!(role & WRITER_MASK);
        const isIndexer = !!(role & INDEXER_MASK);

        const wk = nodeEntry.subarray(offset, offset + WRITER_BYTE_LENGTH);
        offset += WRITER_BYTE_LENGTH;

        const balance = nodeEntry.subarray(offset, offset + BALANCE_BYTE_LENGTH);
        offset += BALANCE_BYTE_LENGTH;

        const license = nodeEntry.subarray(offset, offset + LICENSE_BYTE_LENGTH);
        offset += LICENSE_BYTE_LENGTH;

        const stakedBalance = nodeEntry.subarray(offset, NODE_ENTRY_SIZE);

        return { wk, isWhitelisted, isWriter, isIndexer, balance, license, stakedBalance };
    }
    catch (error) {
        console.error('Error decoding node entry:', error);
        return null; // Return null on error
    }
}

/**
 * Checks if a node entry is whitelisted.
 * @param {Buffer} nodeEntry - The node entry buffer.
 * @returns {boolean} True if whitelisted, false otherwise.
 */
export function isWhitelisted(nodeEntry) {
    if (!isBufferValid(nodeEntry, NODE_ENTRY_SIZE)) {
        return false;
    }
    return !!(nodeEntry[0] & WHITELISTED_MASK);
}

/**
 * Checks if a node entry is a writer.
 * @param {Buffer} nodeEntry - The node entry buffer.
 * @returns {boolean} True if writer, false otherwise.
 */
export function isWriter(nodeEntry) {
    if (!isWhitelisted(nodeEntry)) {
        return false;
    }
    return !!(nodeEntry[0] & WRITER_MASK);
}

/**
 * Checks if a node entry is an indexer.
 * @param {Buffer} nodeEntry - The node entry buffer.
 * @returns {boolean} True if indexer, false otherwise.
 */
export function isIndexer(nodeEntry) {
    if (!isWriter(nodeEntry)) {
        return false;
    }
    return !!(nodeEntry[0] & INDEXER_MASK);
}

/**
 * Updates the role flags (writer/indexer) of an existing node entry buffer in-place.
 * Does not decode or reallocate memory; only updates the first byte (role mask).
 * If the buffer is not the expected size, the function does nothing.
 *
 * @param {Buffer} nodeEntry - The encoded node entry buffer to update.
 * @param {boolean} isWhitelisted - Whether the node should be marked as whitelisted.
 * @param {boolean} isWriter - Whether the node should be marked as a writer.
 * @param {boolean} isIndexer - Whether the node should be marked as an indexer.
 * @returns {Buffer | null} The updated node entry buffer or null if the input is invalid.
 */
export function setRole(nodeEntry, nodeRole) {
    if (!isNodeRoleValid(nodeRole)) {
        console.error('Invalid node role provided');
        return null;
    }
    if (!isBufferValid(nodeEntry, NODE_ENTRY_SIZE)) {
        console.error('Invalid node entry buffer size');
        return null;
    }
    const newNodeEntry = b4a.alloc(NODE_ENTRY_SIZE);
    b4a.copy(nodeEntry, newNodeEntry);
    newNodeEntry[0] = nodeRole;
    return newNodeEntry;
}

/**
 * Sets the writing key in a node entry buffer.
 * @param {Buffer} nodeEntry - The node entry buffer.
 * @param {Buffer} writingKey - The writing key buffer to set.
 * @returns {Buffer|null} The updated node entry buffer, or null if invalid.
 */
export function setWritingKey(nodeEntry, writingKey) {
    try {
        if (!isBufferValid(nodeEntry, NODE_ENTRY_SIZE) || !isBufferValid(writingKey, WRITER_BYTE_LENGTH)) {
            console.error('Invalid input for setting writing key');
            return null;
        }
        b4a.copy(writingKey, nodeEntry, 1);
        return nodeEntry;
    } catch (error) {
        console.error('Error setting writing key in node entry:', error);
        return null;
    }
}

/**
 * Sets the balance.
 * @param {Buffer} nodeEntry - The node entry buffer.
 * @param {Buffer} balance - The buffer representation of the balance
 * @returns {Buffer|null} The updated node entry buffer, or null if invalid.
 */
export function setBalance(nodeEntry, balance) {
    try {
        if (!isBufferValid(nodeEntry, NODE_ENTRY_SIZE) || !isBufferValid(balance, BALANCE_BYTE_LENGTH)) {
            console.error('Invalid input for setting balance');
            return null;
        }

        b4a.copy(balance, nodeEntry, WRITER_BYTE_LENGTH + 1);
        return nodeEntry;
    } catch (error) {
        console.error('Error setting balance in node entry:', error);
        return null;
    }
}

/** Sets the license ID.
 * @param {Buffer} nodeEntry - The node entry buffer.
 * @param {Buffer} license - The buffer representation of the license ID
 * @returns {Buffer|null} The updated node entry buffer, or null if invalid.
 */
export function setLicense(nodeEntry, license) {
    try {
        if (!isBufferValid(nodeEntry, NODE_ENTRY_SIZE) || !isBufferValid(license, LICENSE_BYTE_LENGTH)) {
            console.error('Invalid input for setting license');
            return null;
        }
        b4a.copy(license, nodeEntry, WRITER_BYTE_LENGTH + BALANCE_BYTE_LENGTH + 1);
        return nodeEntry;
    } catch (error) {
        console.error('Error setting license in node entry:', error);
        return null;
    }
}
/**
 * Sets both the staked balance in a node entry buffer.
 * @param {Buffer} nodeEntry - The node entry buffer.
 * @param {Buffer} stakedBalance - The buffer representation of the 16 byte staked balance
 * @returns {Buffer|null} The updated node entry buffer, or null if invalid.
 */
export function setStakedBalance(nodeEntry, stakedBalance) {
    try {
        if (!isBufferValid(nodeEntry, NODE_ENTRY_SIZE) || !isBufferValid(stakedBalance, BALANCE_BYTE_LENGTH)) {
            console.error('Invalid input for setting staked balance');
            return null;
        }
        const newNodeEntry = b4a.alloc(NODE_ENTRY_SIZE);
        b4a.copy(nodeEntry, newNodeEntry);
        b4a.copy(stakedBalance, newNodeEntry, WRITER_BYTE_LENGTH + BALANCE_BYTE_LENGTH + LICENSE_BYTE_LENGTH + 1);
        return newNodeEntry;
    } catch (error) {
        console.error('Error setting staked balance in node entry:', error);
        return null;
    }
}

/**
 * Sets both the role and writing key in a node entry buffer.
 * @param {Buffer} nodeEntry - The node entry buffer.
 * @param {number} nodeRole - The new node role byte.
 * @param {Buffer} writingKey - The writing key buffer to set.
 * @returns {Buffer|null} The updated node entry buffer, or null if invalid.
 */
export function setRoleAndWriterKey(nodeEntry, nodeRole, writingKey) {
    try {
        if (!isNodeRoleValid(nodeRole) || !isBufferValid(writingKey, WRITER_BYTE_LENGTH)) {
            console.error('Invalid input for setting node entry');
            return null;
        }
        if (!isBufferValid(nodeEntry, NODE_ENTRY_SIZE)) {
            console.error('Invalid node entry buffer size');
            return null;
        }
        nodeEntry[0] = nodeRole;
        b4a.copy(writingKey, nodeEntry, 1);
        return nodeEntry;
    } catch (error) {
        console.error('Error setting node entry role and writing key:', error);
        return null;
    }
}

export default {
    NODE_ENTRY_SIZE,
    ZERO_LICENSE,
    init,
    encode,
    decode,
    setBalance,
    setRole,
    setWritingKey,
    setRoleAndWriterKey,
    setLicense,
    setStakedBalance,
    isWhitelisted,
    isWriter,
    isIndexer
};
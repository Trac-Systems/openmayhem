/**
 * Bitmasks for node roles
 * @type {number}
 */
export const WHITELISTED_MASK = 0x1;
export const WRITER_MASK = 0x2;
export const INDEXER_MASK = 0x4;

/**
 * Node roles
 * @readonly
 * @enum {number}
 */
export const NodeRole = {
    READER: 0x0, // Participant in the network able to read transactions and states
    WHITELISTED: 0x1, // A Reader that is also able to apply to become a writer.
    WRITER: 0x3, // A Whitelisted node that is also able to write transactions.
    INDEXER: 0x7, // Special writer that only participates in consensus.
};

 /**
 * Calculates the node role bitmask from a node object.
 * **Note**: This function only sets the bits which represent each role. Some results might be invalid,
 * such as a node that is an indexer without being a writer or a writer which is not whitelisted.
 * We recommend using the `isNodeRoleValid` function to validate the result.
 * @param {{isWhitelisted?: boolean, isWriter?: boolean, isIndexer?: boolean}} nodeObj - The node object.
 * @returns {number} The calculated node role bitmask.
 */
export function calculateNodeRole(nodeObj) {
    let role = NodeRole.READER;
    if (nodeObj.isWhitelisted) role |= WHITELISTED_MASK;
    if (nodeObj.isWriter) role |= WRITER_MASK;
    if (nodeObj.isIndexer) role |= INDEXER_MASK;
    return role;
}

/**
 * Checks if a given role is a valid NodeRole value.
 * @param {number} role - The role to validate.
 * @returns {boolean} True if valid, false otherwise.
 */
export function isNodeRoleValid(role) {
    return Object.values(NodeRole).includes(role);
}

export default {
    WHITELISTED_MASK,
    WRITER_MASK,
    INDEXER_MASK,
    NodeRole,
    calculateNodeRole,
    isNodeRoleValid
};
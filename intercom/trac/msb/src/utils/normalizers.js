import { OperationType } from './constants.js';
import { normalizeHex } from './helpers.js';
import { addressToBuffer, bufferToAddress } from '../core/state/utils/address.js';
import b4a from 'b4a';
import { bufferToBigInt } from './amountSerialization.js'
import {isBootstrapDeployment, isRoleAccess, isTransaction, isTransfer} from './applyOperations.js';

/**
 * Normalizes the payload for a transfer operation.
 * It validates the payload structure and converts specific fields
 * to the correct buffer or hexadecimal string format for processing.
 *
 * @param {Object} payload The raw payload for the transfer operation.
 * @param {object} config The environment configuration object.
 * @returns {Object} A new object with addresses converted to buffers and hex values normalized.
 * @throws {Error} If the payload is invalid or missing required fields.
 */
export function normalizeTransferOperation(payload, config) {
    if (!payload || typeof payload !== 'object' || !payload.tro) {
        throw new Error('Invalid payload for transfer operation normalization.');
    }
    const { type, address, tro } = payload;
    if (
        type !== OperationType.TRANSFER ||
        !address ||
        !tro.tx || !tro.txv || !tro.in ||
        !tro.to || !tro.am || !tro.is
    ) {
        throw new Error('Missing required fields in transfer operation payload.');
    }

    const normalizedTro = {
        tx: normalizeHex(tro.tx),     // Transaction hash
        txv: normalizeHex(tro.txv),   // Transaction validity
        in: normalizeHex(tro.in),     // Nonce
        to: addressToBuffer(tro.to, config.addressPrefix),   // Recipient address
        am: normalizeHex(tro.am),     // Amount
        is: normalizeHex(tro.is)      // Signature
    };

    return {
        type,
        address: addressToBuffer(address, config.addressPrefix),
        tro: normalizedTro
    };
}

/**
 * Normalizes the payload for a transaction operation.
 * This is useful for validating and assembling a transaction operation.
 *
 * @param {Object} payload The raw payload for the transaction operation.
 * @param {object} config The environment configuration object.
 * @returns {Object} A normalized payload with addresses converted to buffers and hex values normalized.
 */
export function normalizeTransactionOperation(payload, config) {
    if (!payload || typeof payload !== 'object' || !payload.txo) {
        throw new Error('Invalid payload for transaction operation normalization.');
    }
    const { type, address, txo } = payload;
    if (
        type !== OperationType.TX ||
        !address ||
        !txo.tx || !txo.txv || !txo.iw ||
        !txo.ch || !txo.bs || !txo.mbs ||
        !txo.in || !txo.is
    ) {
        throw new Error('Missing required fields in transaction operation payload.');
    }
    const normalizedTxo = {
        tx: normalizeHex(txo.tx),     // Transaction hash
        txv: normalizeHex(txo.txv),   // Transaction validity
        iw: normalizeHex(txo.iw),     // Writing key
        ch: normalizeHex(txo.ch),     // Content hash
        bs: normalizeHex(txo.bs),     // Bootstrap
        mbs: normalizeHex(txo.mbs),   // MSB Bootstrap
        in: normalizeHex(txo.in),     // Nonce
        is: normalizeHex(txo.is)      // Signature
    };

    return {
        type,
        address: addressToBuffer(address, config.addressPrefix),
        txo: normalizedTxo
    };
}

/**
 * Normalizes an operation payload by converting any Buffer values to hex strings.
 * This is useful for preparing a payload to be returned as a JSON response.
 *
 * @param {Object} payload The raw payload for the role access operation.
 * @param {object} config The environment configuration object.
 * @returns {Object} A normalized payload with addresses converted to buffers and hex values normalized.
 */
export function normalizeDecodedPayloadForJson(payload, config) {
    if (!payload || typeof payload !== "object") {
        return payload;
    }

    const newPayload = {};
    const addressKeys = ["address", "to", "va", "ia"];
    for (const key in payload) {
        if (payload.hasOwnProperty(key)) {
            const value = payload[key];

            if (b4a.isBuffer(value)) {
                // 👇 intercept address buffers by key name (e.g. `address`)
                if (addressKeys.some(k => key.toLowerCase().includes(k))) {
                    const addr = bufferToAddress(value, config.addressPrefix);
                    newPayload[key] = addr ?? b4a.toString(value, "hex");
                } else if (key.toLowerCase().includes("am")) {
                    const amount = bufferToBigInt(value)
                    newPayload[key] = amount.toString();
                } else {
                    newPayload[key] = b4a.toString(value, "hex");
                }

            } else if (typeof value === "object" && value !== null) {
                // recursively handle nested objects
                newPayload[key] = normalizeDecodedPayloadForJson(value, config);

            } else {
                newPayload[key] = value;
            }
        }
    }
    return newPayload;
}

/**
 * Normalizes the payload for a role access operation.
 * This is useful for validating and assembling a role access operation.
 *
 * @param {Object} payload The raw payload for the role access operation.
 * @param {object} config The environment configuration object.
 * @returns {Object} A normalized payload with addresses converted to buffers and hex values normalized.
 */
export function normalizeRoleAccessOperation(payload, config) {
    if (!payload || typeof payload !== 'object' || !payload.rao) {
        throw new Error('Invalid payload for role access normalization.');
    }
    const { type, address, rao } = payload;
    if (
        !type ||
        !address ||
        !rao.tx || !rao.txv || !rao.iw || !rao.in || !rao.is
    ) {
        throw new Error('Missing required fields in role access payload.');
    }

    const normalizedRao = {
        tx: normalizeHex(rao.tx),
        txv: normalizeHex(rao.txv),
        iw: normalizeHex(rao.iw),
        in: normalizeHex(rao.in),
        is: normalizeHex(rao.is)
    };

    return {
        type,
        address: addressToBuffer(address, config.addressPrefix),
        rao: normalizedRao
    };
}

/**
 * Normalizes the payload for a bootstrap deployment operation.
 * This is useful for validating and assembling a bootstrap deployment operation.
 *
 * @param {Object} payload The raw payload for the bootstrap deployment operation.
 * @param {object} config The environment configuration object.
 * @returns {Object} A normalized payload with addresses converted to buffers and hex values normalized.
 */
export function normalizeBootstrapDeploymentOperation(payload, config) {
    if (!payload || typeof payload !== 'object' || !payload.bdo) {
        throw new Error('Invalid payload for bootstrap deployment normalization.');
    }
    const { type, address, bdo } = payload;
    if (
        type !== OperationType.BOOTSTRAP_DEPLOYMENT ||
        !address ||
        !bdo.tx || !bdo.bs || !bdo.ic || !bdo.in || !bdo.is || !bdo.txv
    ) {
        throw new Error('Missing required fields in bootstrap deployment payload.');
    }

    const normalizedBdo = {
        tx: normalizeHex(bdo.tx),   // Transaction hash
        txv: normalizeHex(bdo.txv), // Transaction validity
        bs: normalizeHex(bdo.bs),   // External bootstrap
        ic: normalizeHex(bdo.ic),   // Channel
        in: normalizeHex(bdo.in),   // Nonce
        is: normalizeHex(bdo.is)    // Signature
    };

    return {
        type,
        address: addressToBuffer(address, config.addressPrefix),
        bdo: normalizedBdo
    };
}

/**
 * Normalizes an incoming partial operation message based on its operation type.
 *
 * @param {Object} message The raw incoming message.
 * @param {object} config The environment configuration object.
 * @returns {Object} Normalized payload.
 * @throws {Error} If message is invalid or operation type is unsupported.
 */
export function normalizeMessageByOperationType(message, config) {
    if (!message || typeof message !== 'object') {
        throw new Error('Invalid message for normalization.');
    }

    const { type } = message;
    if (!Number.isInteger(type) || type <= 0) {
        throw new Error('Message type is missing or invalid.');
    }

    if (isRoleAccess(type)) {
        return normalizeRoleAccessOperation(message, config);
    }

    if (isTransaction(type)) {
        return normalizeTransactionOperation(message, config);
    }

    if (isBootstrapDeployment(type)) {
        return normalizeBootstrapDeploymentOperation(message, config);
    }

    if (isTransfer(type)) {
        return normalizeTransferOperation(message, config);
    }

    throw new Error(`Unsupported operation type for normalization: ${type}`);
}

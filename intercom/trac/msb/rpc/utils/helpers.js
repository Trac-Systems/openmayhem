import b4a from "b4a"
import { operationToPayload } from "../../src/utils/applyOperations.js"
import { isHexString } from "../../src/utils/helpers.js"; 
import { ValidationError } from "../../src/utils/errors.js";

export function decodeBase64Payload(base64) {
    let decodedPayloadString
    try {
        decodedPayloadString = b4a.from(base64, "base64").toString("utf-8")
    } catch {
        throw new ValidationError("Failed to decode base64 payload.")
    }

    try {
        return JSON.parse(decodedPayloadString)
    } catch {
        throw new ValidationError("Decoded payload is not valid JSON.")
    }
}

export function isBase64(str) {
    const base64Regex =
    /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/
    return base64Regex.test(str)
}

export function validatePayloadStructure(payload) {
    if (
        typeof payload !== "object" ||
    payload === null ||
    typeof payload.type !== "number" ||
    typeof payload.address !== "string" ||
    !["txo", "tro"].some((key) => key in payload && typeof payload[key] === "object")
    ) {
        throw new ValidationError("Invalid payload structure.")
    }
}

export function sanitizeTransferPayload(payload) {
    const operationKey = operationToPayload(payload.type);

    if (operationKey !== 'tro' && operationKey !== 'txo') {
        throw new ValidationError('Payload is not a transfer/transaction operation.');
    }

    const operation = payload[operationKey];
    if (payload.address && typeof payload.address === "string") {
        payload.address = payload.address.trim()
    }

    if (operation && typeof operation === "object") {
        for (const [key, value] of Object.entries(operation)) {
            if (typeof value === "string") {
                let sanitized = value.trim()

                // normalize hex-like strings
                if (/^[0-9A-F]+$/i.test(sanitized)) {
                    sanitized = sanitized.toLowerCase()
                }

                payload[operationKey][key] = sanitized
            }
        }
    }

    return payload
}

export function sanitizeBulkPayloadsRequestBody(body) {
    const cleanBody = body
        .replace(/^\uFEFF/, '')
        .replace(/\r/g, '')
        .replace(/\0/g, '')
        .trim();

    return JSON.parse(cleanBody);
}

export function isValidTxHash(hash) {
    return isHexString(hash) && hash?.length === 64;
}

export function hasSpacesInUrl(url) {
    return url.includes('%20') || url.includes(' ');
}

import b4a from "b4a"
import { operationToPayload } from "../../src/utils/applyOperations.js"
export function decodeBase64Payload(base64) {
	let decodedPayloadString
	try {
		decodedPayloadString = b4a.from(base64, "base64").toString("utf-8")
	} catch (err) {
		throw new Error("Failed to decode base64 payload.")
	}

	try {
		return JSON.parse(decodedPayloadString)
	} catch (err) {
		throw new Error("Decoded payload is not valid JSON.")
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
		throw new Error("Invalid payload structure.")
	}
}

export function sanitizeTransferPayload(payload) {
	const operationKey = operationToPayload(payload.type);

	if (operationKey !== 'tro' && operationKey !== 'txo') {
		throw new Error('Payload is not a transfer/transaction operation.');
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

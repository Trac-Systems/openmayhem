import b4a from 'b4a';
import InvalidAddressValidationScenario from './invalidAddressValidationScenario.js';
import {
	safeDecodeApplyOperation,
	safeEncodeApplyOperation
} from '../../../../../../src/utils/protobuf/operationHelpers.js';

export default function createAddressWithInvalidPublicKeyScenario(config, pathOverride) {
	const { addressPath, ...rest } = config ?? {};
	const normalizedPath = normalizePath(pathOverride ?? addressPath);

	return new InvalidAddressValidationScenario({
		...rest,
		mutatePayload: (t, payload) => mutateAddressBuffer(t, payload, normalizedPath)
	});
}

function normalizePath(path) {
	if (!path) return ['address'];
	return Array.isArray(path) ? path : [path];
}

function mutateAddressBuffer(t, validPayload, path) {
	const decodedPayload = safeDecodeApplyOperation(validPayload);
	t.ok(decodedPayload, 'fixtures decode');

	const parent = locateParent(decodedPayload, path);
	if (!parent) return validPayload;

	const field = path[path.length - 1];
	const buffer = parent[field];
	if (!b4a.isBuffer(buffer)) return validPayload;

	const mutatedBuffer = b4a.from(buffer);
	const lastIndex = mutatedBuffer.length - 1;
	const currentChar = mutatedBuffer[lastIndex];
	const asciiP = 'p'.charCodeAt(0);
	const asciiQ = 'q'.charCodeAt(0);
	mutatedBuffer[lastIndex] = currentChar === asciiP ? asciiQ : asciiP;

	parent[field] = mutatedBuffer;
	return safeEncodeApplyOperation(decodedPayload);
}

function locateParent(payload, path) {
	let current = payload;
	for (let i = 0; i < path.length - 1; i++) {
		if (!current) return null;
		current = current[path[i]];
	}
	return current ?? null;
}

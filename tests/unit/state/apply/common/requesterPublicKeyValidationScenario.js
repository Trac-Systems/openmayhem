import b4a from 'b4a';
import RequesterAddressValidationScenario from './requesterAddressValidationScenario.js';
import { safeDecodeApplyOperation, safeEncodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';

function mutateRequesterPublicKey(t, validPayload) {
	const decodedPayload = safeDecodeApplyOperation(validPayload);
	t.ok(decodedPayload, 'fixtures decode');

	const mutatedAddress = b4a.from(decodedPayload.address);
	const lastIndex = mutatedAddress.length - 1;
	const currentChar = mutatedAddress[lastIndex];
	const asciiP = 'p'.charCodeAt(0);
	const asciiQ = 'q'.charCodeAt(0);
	mutatedAddress[lastIndex] = currentChar === asciiP ? asciiQ : asciiP;

	decodedPayload.address = mutatedAddress;
	return safeEncodeApplyOperation(decodedPayload);
}

export default function createRequesterPublicKeyValidationScenario(config) {
	return new RequesterAddressValidationScenario({
		...config,
		mutatePayload: mutateRequesterPublicKey
	});
}

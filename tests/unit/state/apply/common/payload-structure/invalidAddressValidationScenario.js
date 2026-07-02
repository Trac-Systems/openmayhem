import b4a from 'b4a';
import { eventFlush } from '../../../../../helpers/autobaseTestHelpers.js';
import {
	safeDecodeApplyOperation,
	safeEncodeApplyOperation
} from '../../../../../../src/utils/protobuf/operationHelpers.js';
import OperationValidationScenarioBase from '../base/OperationValidationScenarioBase.js';

export default class InvalidAddressValidationScenario extends OperationValidationScenarioBase {
	constructor({
		title,
		setupScenario,
		buildValidPayload,
		assertStateUnchanged,
		addressPath,
		mutatePayload,
		applyInvalidPayload = defaultApplyInvalidPayload,
		expectedLogs
	}) {
		const mutation = mutatePayload ?? createDefaultMutation(addressPath ?? ['address']);

		super({
			title,
			setupScenario,
			buildValidPayload,
			mutatePayload: mutation,
			applyInvalidPayload,
			assertStateUnchanged,
			expectedLogs
		});
	}
}

function createDefaultMutation(addressPath) {
	const pathArray = Array.isArray(addressPath) ? addressPath : [addressPath];
	if (!pathArray.length) {
		throw new Error('Invalid address mutation requires a non-empty addressPath.');
	}

	return (t, validPayload) => {
		const decodedPayload = safeDecodeApplyOperation(validPayload);
		t.ok(decodedPayload, 'fixtures decode');

		const parent = locateParent(decodedPayload, pathArray);
		if (!parent) return validPayload;

		const field = pathArray[pathArray.length - 1];
		const buffer = parent[field];
		if (!b4a.isBuffer(buffer)) return validPayload;

		const mutated = b4a.from(buffer);
		mutated[0] = mutated[0] === 120 ? 121 : 120;
		parent[field] = mutated;

		return safeEncodeApplyOperation(decodedPayload);
	};
}

function locateParent(payload, pathArray) {
	let current = payload;
	for (let i = 0; i < pathArray.length - 1; i++) {
		if (!current) return null;
		current = current[pathArray[i]];
	}
	return current ?? null;
}

async function defaultApplyInvalidPayload(context, invalidPayload) {
	const { bootstrap } = context;
	await bootstrap.base.append(invalidPayload);
	await bootstrap.base.update();
	await eventFlush();
}

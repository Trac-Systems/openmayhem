import b4a from 'b4a';
import OperationValidationScenarioBase from '../base/OperationValidationScenarioBase.js';
import {
	safeDecodeApplyOperation,
	safeEncodeApplyOperation
} from '../../../../../../src/utils/protobuf/operationHelpers.js';
import { eventFlush } from '../../../../../helpers/autobaseTestHelpers.js';

export const PartialOperationMutationStrategy = {
	MISSING_COMPONENT: 'missing-component',
	NONCE_MATCH: 'nonce-match',
	ADDRESS_MATCH: 'address-match',
	SIGNATURE_MATCH: 'signature-match'
};

export default class PartialOperationValidationScenario extends OperationValidationScenarioBase {
	constructor({
		title,
		setupScenario,
		buildValidPayload,
		assertStateUnchanged,
		strategy,
		parentKey = 'rao',
		applyInvalidPayload = defaultApplyInvalidPayload,
		expectedLogs
	}) {
		const mutatePayload = createMutationStrategy(strategy, parentKey);
		super({
			title,
			setupScenario,
			buildValidPayload,
			mutatePayload,
			applyInvalidPayload,
			assertStateUnchanged,
			expectedLogs
		});
	}
}

function createMutationStrategy(strategy, parentKey) {
	switch (strategy) {
		case PartialOperationMutationStrategy.MISSING_COMPONENT:
			return (t, payload) => removeComponent(t, payload, parentKey, 'vs');
		case PartialOperationMutationStrategy.NONCE_MATCH:
			return (t, payload) => duplicateField(t, payload, parentKey, 'vn', 'in');
		case PartialOperationMutationStrategy.ADDRESS_MATCH:
			return (t, payload) => alignAddress(t, payload, parentKey);
		case PartialOperationMutationStrategy.SIGNATURE_MATCH:
			return (t, payload) => duplicateField(t, payload, parentKey, 'vs', 'is');
		default:
			throw new Error(`Unsupported partial operation strategy: ${strategy}`);
	}
}

function removeComponent(t, validPayload, parentKey, component) {
	const decoded = safeDecodeApplyOperation(validPayload);
	t.ok(decoded, 'fixtures decode');
	const parent = decoded[parentKey];
	if (!parent) return validPayload;
	delete parent[component];
	return safeEncodeApplyOperation(decoded);
}

function duplicateField(t, validPayload, parentKey, targetKey, sourceKey) {
	const decoded = safeDecodeApplyOperation(validPayload);
	t.ok(decoded, 'fixtures decode');
	const parent = decoded[parentKey];
	if (!parent || !parent[sourceKey]) return validPayload;
	parent[targetKey] = b4a.from(parent[sourceKey]);
	return safeEncodeApplyOperation(decoded);
}

function alignAddress(t, validPayload, parentKey) {
	const decoded = safeDecodeApplyOperation(validPayload);
	t.ok(decoded, 'fixtures decode');
	const parent = decoded[parentKey];
	if (!parent || !decoded.address) return validPayload;
	parent.va = b4a.from(decoded.address);
	return safeEncodeApplyOperation(decoded);
}

async function defaultApplyInvalidPayload(context, invalidPayload) {
	const { bootstrap } = context;
	await bootstrap.base.append(invalidPayload);
	await bootstrap.base.update();
	await eventFlush();
}

import OperationValidationScenarioBase from './base/OperationValidationScenarioBase.js';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import b4a from 'b4a';
import { safeDecodeApplyOperation, safeEncodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';

export const MessageComponentStrategy = {
	TX_VALIDITY: 'TX_VALIDITY',
	NONCE: 'NONCE'
};

export default class InvalidMessageComponentValidationScenario extends OperationValidationScenarioBase {
	constructor({
		title,
		setupScenario,
		buildValidPayload,
		assertStateUnchanged,
		strategy = MessageComponentStrategy.TX_VALIDITY,
		applyInvalidPayload = defaultApplyInvalidPayload,
		expectedLogs
	}) {
		const mutatePayload = createMutationStrategy(strategy);
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

function createMutationStrategy(strategy) {
	switch (strategy) {
		case MessageComponentStrategy.NONCE:
			return mutateNonce;
		case MessageComponentStrategy.TX_VALIDITY:
		default:
			return mutateTxValidity;
	}
}

function mutateTxValidity(t, validPayload) {
	return mutateComponent(t, validPayload, 'txv');
}

function mutateNonce(t, validPayload) {
	return mutateComponent(t, validPayload, 'in');
}

function mutateComponent(t, validPayload, componentKey) {
	const decodedPayload = safeDecodeApplyOperation(validPayload);
	t.ok(decodedPayload, 'fixtures decode');

	const parent = findParentWithKey(decodedPayload, componentKey);
	if (!parent || !parent[componentKey]) return validPayload;

	parent[componentKey] = flipBuffer(parent[componentKey]);
	return safeEncodeApplyOperation(decodedPayload);
}

function findParentWithKey(payload, key) {
	const parents = ['cao', 'bio', 'aco', 'tro', 'rao', 'bdo', 'txo'];
	for (const parentKey of parents) {
		const parent = payload[parentKey];
		if (parent?.[key]) return parent;
	}
	return null;
}

function flipBuffer(buf) {
	const clone = b4a.from(buf);
	if (clone.length === 0) return clone;
	clone[clone.length - 1] ^= 0xff;
	return clone;
}

async function defaultApplyInvalidPayload(context, invalidPayload) {
	const { bootstrap } = context;
	await bootstrap.base.append(invalidPayload);
	await bootstrap.base.update();
	await eventFlush();
}

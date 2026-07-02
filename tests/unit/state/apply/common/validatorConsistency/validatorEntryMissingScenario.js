import OperationValidationScenarioBase from '../base/OperationValidationScenarioBase.js';
import { eventFlush } from '../../../../../helpers/autobaseTestHelpers.js';

export default class ValidatorEntryMissingScenario extends OperationValidationScenarioBase {
	constructor({
		title,
		setupScenario,
		buildValidPayload,
		assertStateUnchanged,
		selectNode = defaultSelectNode,
		expectedLogs
	}) {
		super({
			title,
			setupScenario,
			buildValidPayload,
			mutatePayload: passThroughPayload,
			applyInvalidPayload: createApplyInvalidPayload(selectNode),
			assertStateUnchanged,
			expectedLogs
		});
	}
}

function passThroughPayload(_t, payload) {
	return payload;
}

function defaultSelectNode(context) {
	return context.adminBootstrap ?? context.bootstrap ?? context.peers?.[0] ?? null;
}

function createApplyInvalidPayload(selectNode) {
	return async (context, payload) => {
		const node = selectNode(context);
		if (!node.base) {
			throw new Error('Validator entry missing scenario requires a writable node.');
		}

		await node.base.append(payload);
		await node.base.update();
		await eventFlush();
	};
}

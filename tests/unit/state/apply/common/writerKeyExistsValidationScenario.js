import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import OperationValidationScenarioBase from './base/OperationValidationScenarioBase.js';

export default class WriterKeyExistsValidationScenario extends OperationValidationScenarioBase {
	constructor({
		title,
		setupScenario,
		buildValidPayload,
		assertStateUnchanged,
		mutatePayload = passThroughPayload,
		applyInvalidPayload,
		expectedLogs
	}) {
		super({
			title,
			setupScenario,
			buildValidPayload,
			mutatePayload,
			applyInvalidPayload: applyInvalidPayload ?? defaultApplyInvalidPayload,
			assertStateUnchanged,
			expectedLogs
		});
	}
}

function passThroughPayload(_t, payload) {
	return payload;
}

async function defaultApplyInvalidPayload(context, invalidPayload, _t, validPayload) {
	const adminNode = context.adminBootstrap ?? context.bootstrap;
	if (!adminNode) {
		throw new Error('Writer key validation scenario requires an admin bootstrap node.');
	}

	await adminNode.base.append(validPayload);
	await adminNode.base.update();
	await eventFlush();

	await adminNode.base.append(invalidPayload);
	await adminNode.base.update();
	await eventFlush();
}

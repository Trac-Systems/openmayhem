import OperationValidationScenarioBase from '../base/OperationValidationScenarioBase.js';

const passThroughPayload = (_t, payload) => payload;

export default class IndexerRoleUpdateFailureScenario extends OperationValidationScenarioBase {
	constructor({
		title,
		setupScenario,
		buildValidPayload,
		assertStateUnchanged,
		expectedLogs,
		applyRoleMutationFailure,
		mutatePayload = passThroughPayload
	}) {
		if (typeof applyRoleMutationFailure !== 'function') {
			throw new Error('Indexer role update failure scenario requires an applyRoleMutationFailure function.');
		}

		super({
			title,
			setupScenario,
			buildValidPayload,
			mutatePayload,
			applyInvalidPayload: applyRoleMutationFailure,
			assertStateUnchanged,
			expectedLogs
		});
	}
}

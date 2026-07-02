import OperationValidationScenarioBase from '../base/OperationValidationScenarioBase.js';
import { applyWithRequesterEntryRemoval } from '../../addWriter/addWriterScenarioHelpers.js';

const passThroughPayload = (_t, payload) => payload;

export default class RequesterNodeEntryMissingScenario extends OperationValidationScenarioBase {
	constructor({
		title,
		setupScenario,
		buildValidPayload,
		assertStateUnchanged,
		expectedLogs,
		selectPeer,
		mutatePayload = passThroughPayload
	}) {
		if (typeof selectPeer !== 'function') {
			throw new Error('Requester node entry scenario requires a selectPeer function.');
		}

		super({
			title,
			setupScenario,
			buildValidPayload,
			mutatePayload,
			applyInvalidPayload: async (context, invalidPayload) => {
				const peer = selectPeer(context);
				if (!peer) {
					throw new Error('Requester node entry scenario requires a peer instance.');
				}
				await applyWithRequesterEntryRemoval(context, invalidPayload, { peer });
			},
			assertStateUnchanged,
			expectedLogs
		});
	}
}

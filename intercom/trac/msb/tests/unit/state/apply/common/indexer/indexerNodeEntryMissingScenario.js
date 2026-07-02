import OperationValidationScenarioBase from '../base/OperationValidationScenarioBase.js';
import { applyWithRequesterEntryRemoval } from '../../addWriter/addWriterScenarioHelpers.js';

const passThroughPayload = (_t, payload) => payload;

export default class IndexerNodeEntryMissingScenario extends OperationValidationScenarioBase {
	constructor({
		title,
		setupScenario,
		buildValidPayload,
		assertStateUnchanged,
		expectedLogs,
		selectPeer,
		mutatePayload = passThroughPayload
	}) {
		const peerSelector = typeof selectPeer === 'function' ? selectPeer : null;

		super({
			title,
			setupScenario,
			buildValidPayload,
			mutatePayload,
			applyInvalidPayload: (context, invalidPayload, t, validPayload) => {
				const peer =
					peerSelector?.(context, {
						invalidPayload,
						validPayload,
						t
					}) ?? null;
				return applyWithRequesterEntryRemoval(context, invalidPayload, { peer });
			},
			assertStateUnchanged,
			expectedLogs
		});
	}
}

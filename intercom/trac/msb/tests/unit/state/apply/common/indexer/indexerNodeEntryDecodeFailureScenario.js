import OperationValidationScenarioBase from '../base/OperationValidationScenarioBase.js';
import { applyWithRequesterEntryCorruption } from '../../addWriter/addWriterScenarioHelpers.js';

const passThroughPayload = (_t, payload) => payload;

export default class IndexerNodeEntryDecodeFailureScenario extends OperationValidationScenarioBase {
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
				return applyWithRequesterEntryCorruption(context, invalidPayload, { peer });
			},
			assertStateUnchanged,
			expectedLogs
		});
	}
}

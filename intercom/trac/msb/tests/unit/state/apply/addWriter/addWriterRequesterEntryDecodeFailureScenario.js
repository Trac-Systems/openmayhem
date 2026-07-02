import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import {
	setupAddWriterScenario,
	buildAddWriterPayload,
	assertAddWriterFailureState,
	applyWithRequesterEntryCorruption
} from './addWriterScenarioHelpers.js';

export default function addWriterRequesterEntryDecodeFailureScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply addWriter rejects payloads when requester entry cannot be decoded',
		setupScenario: setupAddWriterScenario,
		buildValidPayload: buildAddWriterPayload,
		mutatePayload: (_t, payload) => payload,
		applyInvalidPayload: applyWithRequesterEntryCorruption,
		assertStateUnchanged: (t, context) => assertAddWriterFailureState(t, context, { skipSync: true }),
		expectedLogs: ['Failed to decode node entry.']
	}).performScenario();
}

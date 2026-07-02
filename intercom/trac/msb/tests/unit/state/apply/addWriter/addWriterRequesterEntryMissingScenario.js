import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import {
	setupAddWriterScenario,
	buildAddWriterPayload,
	assertAddWriterFailureState,
	applyWithRequesterEntryRemoval
} from './addWriterScenarioHelpers.js';

export default function addWriterRequesterEntryMissingScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply addWriter rejects payloads when requester entry is missing',
		setupScenario: setupAddWriterScenario,
	buildValidPayload: buildAddWriterPayload,
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: applyWithRequesterEntryRemoval,
	assertStateUnchanged: (t, context) => assertAddWriterFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Failed to verify requester node address.', 'Failed to add writer.']
}).performScenario();
}

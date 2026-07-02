import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import {
	setupRemoveWriterScenario,
	buildRemoveWriterPayload,
	assertRemoveWriterFailureState
} from './removeWriterScenarioHelpers.js';
import { applyWithRequesterEntryRemoval } from '../addWriter/addWriterScenarioHelpers.js';

export default function removeWriterRequesterEntryMissingScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply removeWriter rejects payloads when requester entry is missing',
		setupScenario: setupRemoveWriterScenario,
		buildValidPayload: context => buildRemoveWriterPayload(context),
		mutatePayload: (_t, payload) => payload,
		applyInvalidPayload: applyWithRequesterEntryRemoval,
		assertStateUnchanged: (t, context) => assertRemoveWriterFailureState(t, context, { skipSync: true }),
		expectedLogs: ['Failed to verify requester node entry.']
	}).performScenario();
}

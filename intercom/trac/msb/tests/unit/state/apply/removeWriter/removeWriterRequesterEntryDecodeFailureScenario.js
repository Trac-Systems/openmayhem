import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import {
	setupRemoveWriterScenario,
	buildRemoveWriterPayload,
	assertRemoveWriterFailureState
} from './removeWriterScenarioHelpers.js';
import { applyWithRequesterEntryCorruption } from '../addWriter/addWriterScenarioHelpers.js';

export default function removeWriterRequesterEntryDecodeFailureScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply removeWriter rejects payloads when requester entry cannot be decoded',
		setupScenario: setupRemoveWriterScenario,
		buildValidPayload: context => buildRemoveWriterPayload(context),
		mutatePayload: (_t, payload) => payload,
		applyInvalidPayload: applyWithRequesterEntryCorruption,
		assertStateUnchanged: (t, context) => assertRemoveWriterFailureState(t, context, { skipSync: true }),
		expectedLogs: ['Failed to decode requester node entry.']
	}).performScenario();
}

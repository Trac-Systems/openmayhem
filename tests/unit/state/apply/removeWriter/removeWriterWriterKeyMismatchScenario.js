import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import {
	setupRemoveWriterScenario,
	buildRemoveWriterPayload,
	assertRemoveWriterFailureState
} from './removeWriterScenarioHelpers.js';
import { applyWithRequesterWriterKeyMismatch } from '../addWriter/addWriterScenarioHelpers.js';

export default function removeWriterWriterKeyMismatchScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply removeWriter rejects payloads when requester writer key mismatches entry',
		setupScenario: setupRemoveWriterScenario,
		buildValidPayload: context => buildRemoveWriterPayload(context),
		mutatePayload: (_t, payload) => payload,
		applyInvalidPayload: applyWithRequesterWriterKeyMismatch,
		assertStateUnchanged: (t, context) => assertRemoveWriterFailureState(t, context, { skipSync: true }),
		expectedLogs: [
			"Writer key must be registered, match node's current key, and belong to the requester."
		]
	}).performScenario();
}

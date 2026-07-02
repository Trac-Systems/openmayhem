import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import {
	setupRemoveWriterScenario,
	buildRemoveWriterPayload,
	assertRemoveWriterFailureState,
	applyWithWriterRegistryRemoval
} from './removeWriterScenarioHelpers.js';

export default function removeWriterWriterKeyRegistryMissingScenario() {
	new OperationValidationScenarioBase({
		title:
			'State.apply removeWriter rejects payloads when writer key is not registered to the requester',
		setupScenario: setupRemoveWriterScenario,
		buildValidPayload: context => buildRemoveWriterPayload(context),
		mutatePayload: (_t, payload) => payload,
		applyInvalidPayload: applyWithWriterRegistryRemoval,
		assertStateUnchanged: (t, context) => assertRemoveWriterFailureState(t, context, { skipSync: true }),
		expectedLogs: [
			"Writer key must be registered, match node's current key, and belong to the requester."
		]
	}).performScenario();
}

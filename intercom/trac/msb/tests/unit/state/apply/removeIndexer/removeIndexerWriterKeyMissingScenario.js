import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import {
	setupRemoveIndexerScenario,
	buildRemoveIndexerPayload,
	assertRemoveIndexerGuardFailureState,
	applyWithoutIndexerMembership
} from './removeIndexerScenarioHelpers.js';

export default function removeIndexerWriterKeyMissingScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply removeIndexer rejects payloads when writer key is missing from indexer list',
		setupScenario: setupRemoveIndexerScenario,
		buildValidPayload: context => buildRemoveIndexerPayload(context),
		mutatePayload: (_t, payload) => payload,
		applyInvalidPayload: (context, invalidPayload) =>
			applyWithoutIndexerMembership(context, invalidPayload),
		assertStateUnchanged: (t, context) => assertRemoveIndexerGuardFailureState(t, context, { skipSync: true }),
		expectedLogs: ['Writer key does not exist in indexer list.']
	}).performScenario();
}

import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import {
	setupAddWriterScenario,
	buildAddWriterPayload,
	assertAddWriterFailureState,
	applyWithRequesterRoleOverride
} from './addWriterScenarioHelpers.js';
import nodeRoleUtils from '../../../../../src/core/state/utils/roles.js';

export default function addWriterRequesterAlreadyWriterScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply addWriter rejects payloads when requester is already a writer',
		setupScenario: setupAddWriterScenario,
		buildValidPayload: buildAddWriterPayload,
		mutatePayload: (_t, payload) => payload,
		applyInvalidPayload: (context, invalidPayload) =>
			applyWithRequesterRoleOverride(context, invalidPayload, nodeRoleUtils.NodeRole.WRITER),
		assertStateUnchanged: (t, context) => assertAddWriterFailureState(t, context, { skipSync: true }),
		expectedLogs: ['Node must be whitelisted, and cannot be a writer or an indexer.']
	}).performScenario();
}

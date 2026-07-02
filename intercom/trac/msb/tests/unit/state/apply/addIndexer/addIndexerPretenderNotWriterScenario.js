import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import nodeRoleUtils from '../../../../../src/core/state/utils/roles.js';
import {
	setupAddIndexerScenario,
	buildAddIndexerPayload,
	assertAddIndexerFailureState,
	applyWithPretenderRoleMutation
} from './addIndexerScenarioHelpers.js';

export default function addIndexerPretenderNotWriterScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply addIndexer rejects payloads when target is not a writer',
		setupScenario: setupAddIndexerScenario,
		buildValidPayload: context => buildAddIndexerPayload(context),
		mutatePayload: (_t, payload) => payload,
		applyInvalidPayload: (context, invalidPayload) =>
			applyWithPretenderRoleMutation(context, invalidPayload, nodeRoleUtils.NodeRole.READER),
		assertStateUnchanged: (t, context) => assertAddIndexerFailureState(t, context, { skipSync: true }),
		expectedLogs: ['Node must be a writer, and cannot already be an indexer.']
	}).performScenario();
}

import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import {
	setupRemoveWriterScenario,
	buildRemoveWriterPayload,
	assertRemoveWriterFailureState
} from './removeWriterScenarioHelpers.js';
import nodeRoleUtils from '../../../../../src/core/state/utils/roles.js';
import { applyWithRequesterRoleOverride } from '../addWriter/addWriterScenarioHelpers.js';

export default function removeWriterRequesterIndexerScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply removeWriter rejects payloads when requester is an indexer',
		setupScenario: setupRemoveWriterScenario,
		buildValidPayload: context => buildRemoveWriterPayload(context),
		mutatePayload: (_t, payload) => payload,
		applyInvalidPayload: (context, invalidPayload) =>
			applyWithRequesterRoleOverride(context, invalidPayload, nodeRoleUtils.NodeRole.INDEXER),
		assertStateUnchanged: (t, context) => assertRemoveWriterFailureState(t, context, { skipSync: true }),
		expectedLogs: ['Node has to be a writer, and cannot be an indexer.']
	}).performScenario();
}

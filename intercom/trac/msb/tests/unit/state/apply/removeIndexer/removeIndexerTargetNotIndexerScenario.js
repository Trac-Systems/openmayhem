import { test } from 'brittle';
import nodeRoleUtils from '../../../../../src/core/state/utils/roles.js';
import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import { applyWithPretenderRoleMutation } from '../addIndexer/addIndexerScenarioHelpers.js';
import {
	setupRemoveIndexerScenario,
	buildRemoveIndexerPayload,
	assertRemoveIndexerFailureState
} from './removeIndexerScenarioHelpers.js';

export default function removeIndexerTargetNotIndexerScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply removeIndexer rejects payloads when target is not an indexer',
		setupScenario: setupRemoveIndexerScenario,
		buildValidPayload: context => buildRemoveIndexerPayload(context),
		mutatePayload: (_t, payload) => payload,
		applyInvalidPayload: (context, invalidPayload) =>
			applyWithPretenderRoleMutation(context, invalidPayload, nodeRoleUtils.NodeRole.WRITER),
		assertStateUnchanged: (t, context) => assertRemoveIndexerFailureState(t, context, { skipSync: true }),
		expectedLogs: ['Node must be an indexer.']
	}).performScenario(test);
}

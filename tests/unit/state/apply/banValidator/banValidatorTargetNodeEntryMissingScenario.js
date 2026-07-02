import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import {
	setupBanValidatorScenario,
	buildBanValidatorPayload,
	assertBanValidatorFailureState,
	applyWithTargetNodeEntryRemoval
} from './banValidatorScenarioHelpers.js';

export default function banValidatorTargetNodeEntryMissingScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply banValidator rejects payloads when target node entry is missing',
		setupScenario: setupBanValidatorScenario,
		buildValidPayload: context => buildBanValidatorPayload(context),
		mutatePayload: (_t, payload) => payload,
		applyInvalidPayload: applyWithTargetNodeEntryRemoval,
		assertStateUnchanged: (t, context) => assertBanValidatorFailureState(t, context, { skipSync: true }),
		expectedLogs: ['Failed to verify target node entry.']
	}).performScenario();
}

import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import {
	setupBanValidatorScenario,
	buildBanValidatorPayload,
	assertBanValidatorFailureState,
	applyWithBanValidatorRoleUpdateFailure
} from './banValidatorScenarioHelpers.js';

export default function banValidatorTargetRoleUpdateFailureScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply banValidator rejects payloads when target node role update fails',
		setupScenario: setupBanValidatorScenario,
		buildValidPayload: context => buildBanValidatorPayload(context),
		mutatePayload: (_t, payload) => payload,
		applyInvalidPayload: applyWithBanValidatorRoleUpdateFailure,
		assertStateUnchanged: (t, context) => assertBanValidatorFailureState(t, context, { skipSync: true }),
		expectedLogs: ['Failed to update target node role.']
	}).performScenario();
}

import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import {
	setupBanValidatorScenario,
	buildBanValidatorPayload,
	assertBanValidatorFailureState,
	applyWithBanValidatorRoleDecodeFailure
} from './banValidatorScenarioHelpers.js';

export default function banValidatorTargetDecodeFailureScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply banValidator rejects payloads when target node entry cannot be decoded after role update',
		setupScenario: setupBanValidatorScenario,
		buildValidPayload: context => buildBanValidatorPayload(context),
		mutatePayload: (_t, payload) => payload,
		applyInvalidPayload: applyWithBanValidatorRoleDecodeFailure,
		assertStateUnchanged: (t, context) => assertBanValidatorFailureState(t, context, { skipSync: true }),
		expectedLogs: ['Failed to decode target node entry.']
	}).performScenario();
}

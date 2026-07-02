import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import {
	setupBanValidatorScenario,
	buildBanValidatorPayload,
	assertBanValidatorFailureState,
	applyWithBanValidatorWithdrawFailure
} from './banValidatorScenarioHelpers.js';

export default function banValidatorWithdrawFailureScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply banValidator rejects payloads when staked balance cannot be withdrawn',
		setupScenario: setupBanValidatorScenario,
		buildValidPayload: context => buildBanValidatorPayload(context),
		mutatePayload: (_t, payload) => payload,
		applyInvalidPayload: applyWithBanValidatorWithdrawFailure,
		assertStateUnchanged: (t, context) => assertBanValidatorFailureState(t, context, { skipSync: true }),
		expectedLogs: ['Failed to withdraw staked balance.']
	}).performScenario();
}

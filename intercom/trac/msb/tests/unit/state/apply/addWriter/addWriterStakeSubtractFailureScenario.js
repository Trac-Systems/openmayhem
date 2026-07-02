import { BALANCE_ZERO, BALANCE_TO_STAKE } from '../../../../../src/core/state/utils/balance.js';
import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import {
	setupAddWriterScenario,
	buildAddWriterPayload,
	assertAddWriterFailureState,
	applyWithStakeEntryMutation
} from './addWriterScenarioHelpers.js';

export default function addWriterStakeSubtractFailureScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply addWriter rejects payloads when stake balance subtraction fails',
		setupScenario: setupAddWriterScenario,
		buildValidPayload: buildAddWriterPayload,
		mutatePayload: (_t, payload) => payload,
		applyInvalidPayload: async (context, invalidPayload) => {
			const balancePrototype = Object.getPrototypeOf(BALANCE_ZERO);
			const originalSub = balancePrototype.sub;
			let shouldFailSubtraction = true;

			balancePrototype.sub = function patchedSub(balance) {
				if (shouldFailSubtraction && balance === BALANCE_TO_STAKE) {
					shouldFailSubtraction = false;
					return null;
				}
				return originalSub.call(this, balance);
			};

			try {
				await applyWithStakeEntryMutation(
					context,
					invalidPayload,
					nodeEntryBuffer => nodeEntryBuffer
				);
			} finally {
				balancePrototype.sub = originalSub;
			}
		},
		assertStateUnchanged: (t, context) => assertAddWriterFailureState(t, context, { skipSync: true }),
		expectedLogs: ['Failed to subtract stake balance']
	}).performScenario();
}

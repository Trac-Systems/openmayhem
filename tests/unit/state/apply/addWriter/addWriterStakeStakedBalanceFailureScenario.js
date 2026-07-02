import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import nodeEntryUtils from '../../../../../src/core/state/utils/nodeEntry.js';
import {
	setupAddWriterScenario,
	buildAddWriterPayload,
	assertAddWriterFailureState,
	applyWithStakeEntryMutation
} from './addWriterScenarioHelpers.js';

export default function addWriterStakeStakedBalanceFailureScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply addWriter rejects payloads when setting staked balance fails',
		setupScenario: setupAddWriterScenario,
		buildValidPayload: buildAddWriterPayload,
		mutatePayload: (_t, payload) => payload,
		applyInvalidPayload: async (context, invalidPayload) => {
			const originalSetStakedBalance = nodeEntryUtils.setStakedBalance;
			let shouldFail = true;

			nodeEntryUtils.setStakedBalance = function patchedSetStakedBalance(nodeEntryBuffer, stakedBalance) {
				if (shouldFail) {
					shouldFail = false;
					return null;
				}
				return originalSetStakedBalance(nodeEntryBuffer, stakedBalance);
			};

			try {
				await applyWithStakeEntryMutation(context, invalidPayload, nodeEntryBuffer => nodeEntryBuffer);
			} finally {
				nodeEntryUtils.setStakedBalance = originalSetStakedBalance;
			}
		},
		assertStateUnchanged: (t, context) => assertAddWriterFailureState(t, context, { skipSync: true }),
		expectedLogs: ['Failed to set staked balance in node entry']
	}).performScenario();
}

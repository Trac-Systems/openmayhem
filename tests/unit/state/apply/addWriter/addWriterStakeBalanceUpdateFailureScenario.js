import { BALANCE_ZERO } from '../../../../../src/core/state/utils/balance.js';
import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import {
	setupAddWriterScenario,
	buildAddWriterPayload,
	assertAddWriterFailureState,
	applyWithStakeEntryMutation
} from './addWriterScenarioHelpers.js';

const STAKE_BALANCE_MARK = Symbol('stake-balance-update-target');

export default function addWriterStakeBalanceUpdateFailureScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply addWriter rejects payloads when stake balance update fails',
		setupScenario: setupAddWriterScenario,
		buildValidPayload: buildAddWriterPayload,
		mutatePayload: (_t, payload) => payload,
		applyInvalidPayload: async (context, invalidPayload) => {
			const balancePrototype = Object.getPrototypeOf(BALANCE_ZERO);
			const originalUpdate = balancePrototype.update;
			let shouldFail = true;

			balancePrototype.update = function patchedUpdate(nodeEntryBuffer) {
				if (shouldFail && nodeEntryBuffer?.[STAKE_BALANCE_MARK]) {
					shouldFail = false;
					delete nodeEntryBuffer[STAKE_BALANCE_MARK];
					return null;
				}
				return originalUpdate.call(this, nodeEntryBuffer);
			};

			try {
				await applyWithStakeEntryMutation(
					context,
					invalidPayload,
					nodeEntryBuffer => {
						if (nodeEntryBuffer && typeof nodeEntryBuffer === 'object') {
							nodeEntryBuffer[STAKE_BALANCE_MARK] = true;
						}
						return nodeEntryBuffer;
					}
				);
			} finally {
				balancePrototype.update = originalUpdate;
			}
		},
		assertStateUnchanged: (t, context) => assertAddWriterFailureState(t, context, { skipSync: true }),
		expectedLogs: ['Failed to update node entry with new balance']
	}).performScenario();
}

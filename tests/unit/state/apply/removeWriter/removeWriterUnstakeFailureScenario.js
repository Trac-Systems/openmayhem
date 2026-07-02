import b4a from 'b4a';
import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import {
	setupRemoveWriterScenario,
	buildRemoveWriterPayload,
	assertRemoveWriterFailureState
} from './removeWriterScenarioHelpers.js';
import { applyWithStakeEntryMutation } from '../addWriter/addWriterScenarioHelpers.js';
import nodeEntryUtils from '../../../../../src/core/state/utils/nodeEntry.js';
import { BALANCE_ZERO } from '../../../../../src/core/state/utils/balance.js';

export default function removeWriterUnstakeFailureScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply removeWriter rejects payloads when unstaking requester balance fails',
		setupScenario: setupRemoveWriterScenario,
		buildValidPayload: context => buildRemoveWriterPayload(context),
		mutatePayload: (_t, payload) => payload,
		applyInvalidPayload: (context, invalidPayload) =>
			applyWithStakeEntryMutation(context, invalidPayload, nodeEntryBuffer => {
				if (!nodeEntryBuffer) return nodeEntryBuffer;
				const mutatedBuffer = nodeEntryUtils.setStakedBalance(
					b4a.from(nodeEntryBuffer),
					BALANCE_ZERO.value
				);
				if (!mutatedBuffer) {
					throw new Error('Failed to mutate staked balance for unstake failure scenario.');
				}
				return mutatedBuffer;
			}),
		assertStateUnchanged: (t, context) => assertRemoveWriterFailureState(t, context, { skipSync: true }),
		expectedLogs: ['Failed to unstake balance for writer.']
	}).performScenario();
}

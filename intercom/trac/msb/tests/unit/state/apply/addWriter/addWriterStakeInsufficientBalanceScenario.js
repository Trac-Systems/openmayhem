import { BALANCE_ZERO } from '../../../../../src/core/state/utils/balance.js';
import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import {
	setupAddWriterScenario,
	buildAddWriterPayload,
	assertAddWriterFailureState,
	applyWithStakeEntryMutation
} from './addWriterScenarioHelpers.js';

export default function addWriterStakeInsufficientBalanceScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply addWriter rejects payloads when stake balance is insufficient',
		setupScenario: setupAddWriterScenario,
		buildValidPayload: buildAddWriterPayload,
		mutatePayload: (_t, payload) => payload,
		applyInvalidPayload: (context, invalidPayload) =>
			applyWithStakeEntryMutation(
				context,
				invalidPayload,
				nodeEntryBuffer => nodeEntryBuffer,
				decodedEntry => {
					if (!decodedEntry) return decodedEntry;
					return { ...decodedEntry, balance: BALANCE_ZERO.value };
				}
			),
		assertStateUnchanged: (t, context) => assertAddWriterFailureState(t, context, { skipSync: true }),
		expectedLogs: ['Insufficient balance to stake']
	}).performScenario();
}

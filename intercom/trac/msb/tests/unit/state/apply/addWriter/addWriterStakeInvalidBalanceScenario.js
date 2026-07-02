import b4a from 'b4a';
import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import {
	setupAddWriterScenario,
	buildAddWriterPayload,
	assertAddWriterFailureState,
	applyWithStakeEntryMutation
} from './addWriterScenarioHelpers.js';

export default function addWriterStakeInvalidBalanceScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply addWriter rejects payloads when stake balance is invalid',
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
					return { ...decodedEntry, balance: b4a.alloc(1) };
				}
			),
		assertStateUnchanged: (t, context) => assertAddWriterFailureState(t, context, { skipSync: true }),
		expectedLogs: ['Invalid node balance']
	}).performScenario();
}

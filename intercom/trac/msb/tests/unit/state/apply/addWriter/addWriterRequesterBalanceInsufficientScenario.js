import b4a from 'b4a';
import RequesterBalanceScenarioBase from '../common/balances/base/requesterBalanceScenarioBase.js';
import {
	setupAddWriterScenario,
	buildAddWriterPayload,
	assertAddWriterFailureState,
	selectWriterPeer
} from './addWriterScenarioHelpers.js';

export default function addWriterRequesterBalanceInsufficientScenario() {
	new RequesterBalanceScenarioBase({
		title: 'State.apply addWriter rejects payloads when requester balance is insufficient',
		setupScenario: setupAddWriterScenario,
		buildValidPayload: buildAddWriterPayload,
		assertStateUnchanged: (t, context) => assertAddWriterFailureState(t, context, { skipSync: true }),
		selectPeer: selectWriterPeer,
		mutateDecodedEntry: decoded =>
			decoded ? { ...decoded, balance: b4a.alloc(decoded.balance.length) } : decoded,
		expectedLogs: ['Insufficient requester balance.', 'Add writer operation ignored.']
	}).performScenario();
}

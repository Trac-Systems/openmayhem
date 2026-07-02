import b4a from 'b4a';
import RequesterBalanceScenarioBase from '../common/balances/base/requesterBalanceScenarioBase.js';
import {
	setupRemoveWriterScenario,
	buildRemoveWriterPayload,
	assertRemoveWriterFailureState,
	selectWriterPeer
} from './removeWriterScenarioHelpers.js';

export default function removeWriterRequesterBalanceInsufficientScenario() {
	new RequesterBalanceScenarioBase({
		title: 'State.apply removeWriter rejects payloads when requester balance is insufficient',
		setupScenario: setupRemoveWriterScenario,
		buildValidPayload: context => buildRemoveWriterPayload(context),
		assertStateUnchanged: (t, context) => assertRemoveWriterFailureState(t, context, { skipSync: true }),
		selectPeer: selectWriterPeer,
		mutateDecodedEntry: decoded =>
			decoded ? { ...decoded, balance: b4a.alloc(decoded.balance.length) } : decoded,
		expectedLogs: ['Insufficient requester balance.', 'Remove writer operation ignored.']
	}).performScenario();
}

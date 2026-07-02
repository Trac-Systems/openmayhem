import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import {
	setupRemoveWriterScenario,
	buildRemoveWriterPayload,
	assertRemoveWriterFailureState,
	applyWithWriterRegistryForeignAddress
} from './removeWriterScenarioHelpers.js';
import { selectValidatorPeerWithoutEntry } from '../addWriter/addWriterScenarioHelpers.js';

export default function removeWriterWriterKeyOwnershipScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply removeWriter rejects payloads when writer key belongs to another node',
		setupScenario: t => setupRemoveWriterScenario(t, { nodes: 3 }),
		buildValidPayload: context => buildRemoveWriterPayload(context),
		mutatePayload: (_t, payload) => payload,
		applyInvalidPayload: (context, invalidPayload) => {
			const foreignPeer =
				selectValidatorPeerWithoutEntry(context) ?? context.peers?.find(peer => peer !== context.adminBootstrap);
			return applyWithWriterRegistryForeignAddress(context, invalidPayload, foreignPeer ?? context.peers[1]);
		},
		assertStateUnchanged: (t, context) => assertRemoveWriterFailureState(t, context, { skipSync: true }),
		expectedLogs: [
			"Writer key must be registered, match node's current key, and belong to the requester."
		]
	}).performScenario();
}

import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import {
	setupAddIndexerScenario,
	buildAddIndexerPayload,
	assertAddIndexerFailureState,
	applyWithIndexerWriterKeyAlreadyRegistered,
	ensureIndexerRegistration,
	selectIndexerCandidatePeer
} from './addIndexerScenarioHelpers.js';

export default function addIndexerWriterKeyAlreadyRegisteredScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply addIndexer rejects payloads when writer key already exists in indexer list',
		setupScenario: async t => {
			const context = await setupAddIndexerScenario(t);
			const writerPeer =
				context.addIndexerScenario?.writerPeer ?? selectIndexerCandidatePeer(context);
			const cleanup = ensureIndexerRegistration(context.adminBootstrap.base, writerPeer.base.local.key);
			context.addIndexerScenario = {
				...(context.addIndexerScenario ?? {}),
				writerKeyMembershipCleanup: cleanup,
				writerPeer
			};
			return context;
		},
		buildValidPayload: context => buildAddIndexerPayload(context),
		mutatePayload: (_t, payload) => payload,
		applyInvalidPayload: applyWithIndexerWriterKeyAlreadyRegistered,
		assertStateUnchanged: (t, context) => assertAddIndexerFailureState(t, context, { skipSync: true }),
		expectedLogs: ['Writer key already exists in indexer list.']
	}).performScenario();
}

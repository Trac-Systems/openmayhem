import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import {
	setupBanValidatorScenario,
	buildBanValidatorPayload,
	assertBanValidatorFailureState,
	promoteValidatorToIndexer
} from './banValidatorScenarioHelpers.js';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';

export default function banValidatorTargetIndexerScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply banValidator rejects payloads when target node is an indexer',
		setupScenario: async t => {
			const context = await setupBanValidatorScenario(t);
			await promoteValidatorToIndexer(context);
			return context;
		},
		buildValidPayload: context => buildBanValidatorPayload(context),
		mutatePayload: (_t, payload) => payload,
		applyInvalidPayload: async (context, payload) => {
			await context.adminBootstrap.base.append(payload);
			await context.adminBootstrap.base.update();
			await eventFlush();
		},
		assertStateUnchanged: (t, context) =>
			assertBanValidatorFailureState(t, context, {
				expectedRoles: { isWhitelisted: true, isWriter: true, isIndexer: true },
				skipSync: true
			}),
		expectedLogs: ['Only writer/whitelisted node can be banned.']
	}).performScenario();
}

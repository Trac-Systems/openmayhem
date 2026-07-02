import InvalidPayloadValidationScenario from '../common/payload-structure/invalidPayloadValidationScenario.js';
import {
	buildTransferPayload,
	mutateTransferPayloadForInvalidSchema,
	assertTransferFailureState,
	setupTransferScenario,
	applyInvalidTransferPayloadWithNoMutations
} from './transferScenarioHelpers.js';

export default function transferContractSchemaValidationScenario() {
	new InvalidPayloadValidationScenario({
		title: 'State.apply transfer rejects payloads when contract schema validation fails',
		setupScenario: setupTransferScenario,
		buildValidPayload: context => buildTransferPayload(context),
		mutatePayload: mutateTransferPayloadForInvalidSchema,
		applyInvalidPayload: (context, invalidPayload, t) =>
			applyInvalidTransferPayloadWithNoMutations(t, context, invalidPayload),
		assertStateUnchanged: (t, context, _validPayload, invalidPayload) =>
			assertTransferFailureState(t, context, { payload: invalidPayload }),
		expectedLogs: ['Contract schema validation failed.']
	}).performScenario();
}

import InvalidPayloadValidationScenario from '../common/payload-structure/invalidPayloadValidationScenario.js';
import AdminEntryDecodeFailureScenario from '../common/access-control/adminEntryDecodeFailureScenario.js';
import AdminPublicKeyDecodeFailureScenario from '../common/access-control/adminPublicKeyDecodeFailureScenario.js';
import AdminOnlyGuardScenario from '../common/access-control/adminOnlyGuardScenario.js';
import AdminConsistencyMismatchScenario from '../common/access-control/adminConsistencyMismatchScenario.js';
import InvalidHashValidationScenario from '../common/payload-structure/invalidHashValidationScenario.js';
import InvalidSignatureValidationScenario, { SignatureMutationStrategy } from '../common/payload-structure/invalidSignatureValidationScenario.js';
import TransactionValidityMismatchScenario from '../common/transactionValidityMismatchScenario.js';
import OperationAlreadyAppliedScenario from '../common/operationAlreadyAppliedScenario.js';
import RequesterAddressValidationScenario from '../common/requesterAddressValidationScenario.js';
import createRequesterPublicKeyValidationScenario from '../common/requesterPublicKeyValidationScenario.js';
import IndexerSequenceStateInvalidScenario from '../common/indexer/indexerSequenceStateInvalidScenario.js';
import disableInitializationHappyPathScenario from './disableInitializationHappyPathScenario.js';
import disableInitializationAlreadyDisabledScenario from './disableInitializationAlreadyDisabledScenario.js';
import {
	setupDisableInitializationScenario,
	buildDisableInitializationPayload,
	buildDisableInitializationPayloadWithTxValidity,
	mutateDisableInitializationPayloadForInvalidSchema,
	assertDisableInitializationFailureState,
	assertInitializationDisabledState,
	bypassDisableInitializationAlreadyDisabledGuardOnce
} from './disableInitializationScenarioHelpers.js';

disableInitializationHappyPathScenario();

new InvalidPayloadValidationScenario({
	title: 'State.apply disableInitialization rejects payloads that fail schema validation',
	setupScenario: setupDisableInitializationScenario,
	buildValidPayload: buildDisableInitializationPayload,
	mutatePayload: mutateDisableInitializationPayloadForInvalidSchema,
	assertStateUnchanged: assertDisableInitializationFailureState,
	expectedLogs: ['Schema validation failed.']
}).performScenario();

disableInitializationAlreadyDisabledScenario();

new RequesterAddressValidationScenario({
	title: 'State.apply disableInitialization requester address is invalid',
	setupScenario: setupDisableInitializationScenario,
	buildValidPayload: buildDisableInitializationPayload,
	assertStateUnchanged: assertDisableInitializationFailureState,
	expectedLogs: ['Failed to validate requester address.']
}).performScenario();

createRequesterPublicKeyValidationScenario({
	title: 'State.apply disableInitialization requester public key is invalid',
	setupScenario: setupDisableInitializationScenario,
	buildValidPayload: buildDisableInitializationPayload,
	assertStateUnchanged: assertDisableInitializationFailureState,
	expectedLogs: ['Failed to decode requester public key.']
}).performScenario();

new AdminEntryDecodeFailureScenario({
	title: 'State.apply disableInitialization aborts when admin entry cannot be decoded',
	setupScenario: setupDisableInitializationScenario,
	buildValidPayload: buildDisableInitializationPayload,
	assertStateUnchanged: (t, context, validPayload) =>
		assertDisableInitializationFailureState(t, context, { skipSync: true, validPayload }),
	expectedLogs: ['Failed to decode admin entry.']
}).performScenario();

new AdminOnlyGuardScenario({
	title: 'State.apply disableInitialization rejects non-admin nodes',
	setupScenario: setupDisableInitializationScenario,
	buildValidPayload: buildDisableInitializationPayload,
	assertStateUnchanged: (t, context, validPayload) =>
		assertDisableInitializationFailureState(t, context, { skipSync: true, validPayload }),
	expectedLogs: ['Node is not allowed to perform this operation. (ADMIN ONLY)']
}).performScenario();

new AdminPublicKeyDecodeFailureScenario({
	title: 'State.apply disableInitialization aborts when admin public key cannot be decoded',
	setupScenario: setupDisableInitializationScenario,
	buildValidPayload: buildDisableInitializationPayload,
	assertStateUnchanged: (t, context, validPayload) =>
		assertDisableInitializationFailureState(t, context, { skipSync: true, validPayload }),
	expectedLogs: ['Failed to decode admin public key.']
}).performScenario();

new AdminConsistencyMismatchScenario({
	title: 'State.apply disableInitialization rejects when admin key mismatch occurs',
	setupScenario: setupDisableInitializationScenario,
	buildValidPayload: buildDisableInitializationPayload,
	assertStateUnchanged: (t, context, validPayload) =>
		assertDisableInitializationFailureState(t, context, { skipSync: true, validPayload }),
	expectedLogs: ['System admin and node public keys do not match.']
}).performScenario();

new InvalidHashValidationScenario({
	title: 'State.apply disableInitialization requester message hash mismatch',
	setupScenario: setupDisableInitializationScenario,
	buildValidPayload: buildDisableInitializationPayload,
	assertStateUnchanged: assertDisableInitializationFailureState,
	expectedLogs: ['Message hash does not match the tx_hash.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply disableInitialization requester signature is invalid (foreign signature)',
	setupScenario: setupDisableInitializationScenario,
	buildValidPayload: buildDisableInitializationPayload,
	assertStateUnchanged: assertDisableInitializationFailureState,
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply disableInitialization requester signature is invalid (zero fill)',
	setupScenario: setupDisableInitializationScenario,
	buildValidPayload: buildDisableInitializationPayload,
	assertStateUnchanged: assertDisableInitializationFailureState,
	strategy: SignatureMutationStrategy.ZERO_FILL,
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply disableInitialization requester signature is invalid (type mismatch)',
	setupScenario: setupDisableInitializationScenario,
	buildValidPayload: buildDisableInitializationPayload,
	assertStateUnchanged: assertDisableInitializationFailureState,
	strategy: SignatureMutationStrategy.TYPE_MISMATCH,
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new IndexerSequenceStateInvalidScenario({
	title: 'State.apply disableInitialization rejects payload when indexer sequence state is invalid',
	setupScenario: setupDisableInitializationScenario,
	buildValidPayload: buildDisableInitializationPayload,
	assertStateUnchanged: (t, context, validPayload, invalidPayload) =>
		assertDisableInitializationFailureState(t, context, {
			skipSync: true,
			validPayload: invalidPayload ?? validPayload
		}),
	expectedLogs: ['Indexer sequence state is invalid.']
}).performScenario();

new TransactionValidityMismatchScenario({
	title: 'State.apply disableInitialization rejects payload when tx validity mismatches indexer state',
	setupScenario: setupDisableInitializationScenario,
	buildValidPayload: buildDisableInitializationPayload,
	assertStateUnchanged: assertDisableInitializationFailureState,
	rebuildPayloadWithTxValidity: ({ context, mutatedTxValidity }) =>
		buildDisableInitializationPayloadWithTxValidity(context, mutatedTxValidity),
	expectedLogs: ['Transaction was not executed.']
}).performScenario();

new OperationAlreadyAppliedScenario({
	title: 'State.apply disableInitialization rejects duplicate operations',
	setupScenario: setupDisableInitializationScenario,
	buildValidPayload: buildDisableInitializationPayload,
	assertStateUnchanged: async (t, context, validPayload) => {
		const adminNode = context.adminBootstrap;
		const readerNode = context.peers?.[1];
		await assertInitializationDisabledState(t, adminNode.base, validPayload);
		if (readerNode) {
			await context.sync();
			await assertInitializationDisabledState(t, readerNode.base, validPayload);
		}
	},
	beforeInvalidApply: ({ context }) => bypassDisableInitializationAlreadyDisabledGuardOnce(context),
	expectedLogs: ['Operation has already been applied.']
}).performScenario();

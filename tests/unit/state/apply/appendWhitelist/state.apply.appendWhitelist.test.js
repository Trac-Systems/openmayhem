import InvalidPayloadValidationScenario from '../common/payload-structure/invalidPayloadValidationScenario.js';
import InvalidAddressValidationScenario from '../common/payload-structure/invalidAddressValidationScenario.js';
import createAddressWithInvalidPublicKeyScenario from '../common/payload-structure/addressWithInvalidPublicKeyScenario.js';
import AdminEntryMissingScenario from '../common/access-control/adminEntryMissingScenario.js';
import AdminEntryDecodeFailureScenario from '../common/access-control/adminEntryDecodeFailureScenario.js';
import AdminOnlyGuardScenario from '../common/access-control/adminOnlyGuardScenario.js';
import AdminPublicKeyDecodeFailureScenario from '../common/access-control/adminPublicKeyDecodeFailureScenario.js';
import AdminConsistencyMismatchScenario from '../common/access-control/adminConsistencyMismatchScenario.js';
import InvalidHashValidationScenario from '../common/payload-structure/invalidHashValidationScenario.js';
import InvalidSignatureValidationScenario, { SignatureMutationStrategy } from '../common/payload-structure/invalidSignatureValidationScenario.js';
import TransactionValidityMismatchScenario from '../common/transactionValidityMismatchScenario.js';
import IndexerSequenceStateInvalidScenario from '../common/indexer/indexerSequenceStateInvalidScenario.js';
import OperationAlreadyAppliedScenario from '../common/operationAlreadyAppliedScenario.js';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import createAdminEntryUpdateFailureScenario from '../common/balances/adminEntryUpdateFailureScenario.js';
import createNodeEntryInitializationFailureScenario from '../common/nodeEntryInitializationFailureScenario.js';
import appendWhitelistHappyPathScenario from './appendWhitelistHappyPathScenario.js';
import appendWhitelistExistingReaderHappyPathScenario from './appendWhitelistExistingReaderHappyPathScenario.js';
import appendWhitelistBanAndReapplyScenario from './appendWhitelistBanAndReapplyScenario.js';
import appendWhitelistFeeAfterDisableScenario from './appendWhitelistFeeAfterDisableScenario.js';
import appendWhitelistNodeAlreadyWhitelistedScenario from './appendWhitelistNodeAlreadyWhitelistedScenario.js';
import appendWhitelistInsufficientAdminBalanceScenario from './appendWhitelistInsufficientAdminBalanceScenario.js';
import { buildDisableInitializationPayload } from '../disableInitialization/disableInitializationScenarioHelpers.js';
import setupAppendWhitelistScenario, {
	buildAppendWhitelistPayload,
	buildAppendWhitelistPayloadWithTxValidity,
	mutateAppendWhitelistPayloadForInvalidSchema,
	assertAppendWhitelistFailureState,
	assertAppendWhitelistSuccessState
} from './appendWhitelistScenarioHelpers.js';

appendWhitelistHappyPathScenario();
appendWhitelistExistingReaderHappyPathScenario();
appendWhitelistBanAndReapplyScenario();
appendWhitelistFeeAfterDisableScenario();
appendWhitelistNodeAlreadyWhitelistedScenario();

new InvalidPayloadValidationScenario({
	title: 'State.apply appendWhitelist rejects payloads that fail schema validation',
	setupScenario: setupAppendWhitelistScenario,
	buildValidPayload: context => buildAppendWhitelistPayload(context),
	mutatePayload: mutateAppendWhitelistPayloadForInvalidSchema,
	assertStateUnchanged: assertAppendWhitelistFailureState,
	expectedLogs: ['Contract schema validation failed.']
}).performScenario();

new InvalidAddressValidationScenario({
	title: 'State.apply appendWhitelist recipient address is invalid',
	setupScenario: setupAppendWhitelistScenario,
	buildValidPayload: context => buildAppendWhitelistPayload(context),
	assertStateUnchanged: assertAppendWhitelistFailureState,
	addressPath: ['address'],
	expectedLogs: ['Recipient address is invalid.']
}).performScenario();

createAddressWithInvalidPublicKeyScenario({
	title: 'State.apply appendWhitelist recipient public key is invalid',
	setupScenario: setupAppendWhitelistScenario,
	buildValidPayload: context => buildAppendWhitelistPayload(context),
	assertStateUnchanged: assertAppendWhitelistFailureState,
	addressPath: ['address'],
	expectedLogs: ['Failed to decode recipient public key.']
}).performScenario();

new AdminEntryMissingScenario({
	title: 'State.apply appendWhitelist aborts when admin entry cannot be verified',
	setupScenario: setupAppendWhitelistScenario,
	buildValidPayload: context => buildAppendWhitelistPayload(context),
	assertStateUnchanged: (t, context) =>
		assertAppendWhitelistFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Failed to verify admin entry.']
}).performScenario();

new AdminEntryDecodeFailureScenario({
	title: 'State.apply appendWhitelist aborts when admin entry cannot be decoded',
	setupScenario: setupAppendWhitelistScenario,
	buildValidPayload: context => buildAppendWhitelistPayload(context),
	assertStateUnchanged: (t, context) =>
		assertAppendWhitelistFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Failed to decode admin entry.']
}).performScenario();

new AdminOnlyGuardScenario({
	title: 'State.apply appendWhitelist rejects non-admin nodes',
	setupScenario: setupAppendWhitelistScenario,
	buildValidPayload: context => buildAppendWhitelistPayload(context),
	assertStateUnchanged: (t, context) =>
		assertAppendWhitelistFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Node is not allowed to perform this operation. (ADMIN ONLY)']
}).performScenario();

new AdminPublicKeyDecodeFailureScenario({
	title: 'State.apply appendWhitelist aborts when admin public key cannot be decoded',
	setupScenario: setupAppendWhitelistScenario,
	buildValidPayload: context => buildAppendWhitelistPayload(context),
	assertStateUnchanged: (t, context) =>
		assertAppendWhitelistFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Failed to decode admin public key.']
}).performScenario();

new AdminConsistencyMismatchScenario({
	title: 'State.apply appendWhitelist rejects when admin key mismatch occurs',
	setupScenario: setupAppendWhitelistScenario,
	buildValidPayload: context => buildAppendWhitelistPayload(context),
	assertStateUnchanged: (t, context) =>
		assertAppendWhitelistFailureState(t, context, { skipSync: true }),
	expectedLogs: ['System admin and node public keys do not match.']
}).performScenario();

new InvalidAddressValidationScenario({
	title: 'State.apply appendWhitelist node address is invalid',
	setupScenario: setupAppendWhitelistScenario,
	buildValidPayload: context => buildAppendWhitelistPayload(context),
	assertStateUnchanged: assertAppendWhitelistFailureState,
	addressPath: ['aco', 'ia'],
	expectedLogs: ['Failed to verify node address.']
}).performScenario();

createAddressWithInvalidPublicKeyScenario({
	title: 'State.apply appendWhitelist node public key is invalid',
	setupScenario: setupAppendWhitelistScenario,
	buildValidPayload: context => buildAppendWhitelistPayload(context),
	assertStateUnchanged: assertAppendWhitelistFailureState,
	addressPath: ['aco', 'ia'],
	expectedLogs: ['Failed to decode node public key.']
}).performScenario();

new InvalidHashValidationScenario({
	title: 'State.apply appendWhitelist requester message hash mismatch',
	setupScenario: setupAppendWhitelistScenario,
	buildValidPayload: context => buildAppendWhitelistPayload(context),
	assertStateUnchanged: assertAppendWhitelistFailureState,
	expectedLogs: ['Message hash does not match the tx_hash.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply appendWhitelist requester signature is invalid (foreign signature)',
	setupScenario: setupAppendWhitelistScenario,
	buildValidPayload: context => buildAppendWhitelistPayload(context),
	assertStateUnchanged: assertAppendWhitelistFailureState,
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply appendWhitelist requester signature is invalid (zero fill)',
	setupScenario: setupAppendWhitelistScenario,
	buildValidPayload: context => buildAppendWhitelistPayload(context),
	assertStateUnchanged: assertAppendWhitelistFailureState,
	strategy: SignatureMutationStrategy.ZERO_FILL,
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply appendWhitelist requester signature is invalid (type mismatch)',
	setupScenario: setupAppendWhitelistScenario,
	buildValidPayload: context => buildAppendWhitelistPayload(context),
	assertStateUnchanged: assertAppendWhitelistFailureState,
	strategy: SignatureMutationStrategy.TYPE_MISMATCH,
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new IndexerSequenceStateInvalidScenario({
	title: 'State.apply appendWhitelist rejects payloads when indexer sequence state is invalid',
	setupScenario: setupAppendWhitelistScenario,
	buildValidPayload: context => buildAppendWhitelistPayload(context),
	assertStateUnchanged: (t, context, validPayload, invalidPayload) =>
		assertAppendWhitelistFailureState(t, context, {
			skipSync: true,
			readerAddress: invalidPayload?.address
		}),
	expectedLogs: ['Indexer sequence state is invalid.']
}).performScenario();

new TransactionValidityMismatchScenario({
	title: 'State.apply appendWhitelist rejects payload when tx validity mismatches indexer state',
	setupScenario: setupAppendWhitelistScenario,
	buildValidPayload: context => buildAppendWhitelistPayload(context),
	assertStateUnchanged: assertAppendWhitelistFailureState,
	txValidityPath: ['aco', 'txv'],
	rebuildPayloadWithTxValidity: ({ context, mutatedTxValidity }) =>
		buildAppendWhitelistPayloadWithTxValidity(context, mutatedTxValidity),
	expectedLogs: ['Transaction was not executed.']
}).performScenario();

new OperationAlreadyAppliedScenario({
	title: 'State.apply appendWhitelist rejects duplicate operations',
	setupScenario: setupAppendWhitelistScenario,
	buildValidPayload: context => buildAppendWhitelistPayload(context),
	assertStateUnchanged: (t, context) =>
		assertAppendWhitelistSuccessState(t, context),
	expectedLogs: ['Operation has already been applied.']
}).performScenario();

appendWhitelistNodeAlreadyWhitelistedScenario();

appendWhitelistInsufficientAdminBalanceScenario();

createAdminEntryUpdateFailureScenario({
	title: 'State.apply appendWhitelist rejects when admin entry update fails',
	setupScenario: setupAppendWhitelistScenario,
	buildValidPayload: context => buildAppendWhitelistPayload(context),
	beforeApply: async context => {
		const adminNode = context.adminBootstrap;
		const disablePayload = await buildDisableInitializationPayload(context);
		await adminNode.base.append(disablePayload);
		await adminNode.base.update();
		await eventFlush();
	},
	assertStateUnchanged: (t, context) =>
		assertAppendWhitelistFailureState(t, context, { skipSync: true })
}).performScenario();

createNodeEntryInitializationFailureScenario({
	title: 'State.apply appendWhitelist rejects when node entry initialization fails',
	setupScenario: setupAppendWhitelistScenario,
	buildValidPayload: context => buildAppendWhitelistPayload(context),
	assertStateUnchanged: (t, context) =>
		assertAppendWhitelistFailureState(t, context, { expectedLicenseCount: 2, skipSync: true }),
	selectNode: context => context.adminBootstrap
}).performScenario();

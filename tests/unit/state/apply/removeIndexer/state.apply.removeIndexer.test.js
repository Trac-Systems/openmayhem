import removeIndexerHappyPathScenario from './removeIndexerHappyPathScenario.js';
import removeIndexerReAddAndRemoveAgainScenario from './removeIndexerReAddAndRemoveAgainScenario.js';
import removeIndexerRemoveMultipleIndexersScenario from './removeIndexerRemoveMultipleIndexersScenario.js';
import removeIndexerTargetNotIndexerScenario from './removeIndexerTargetNotIndexerScenario.js';
import removeIndexerWriterKeyMissingScenario from './removeIndexerWriterKeyMissingScenario.js';
import AdminControlOperationValidationScenario from '../common/adminControlOperationValidationScenario.js';
import RequesterAddressValidationScenario from '../common/requesterAddressValidationScenario.js';
import createRequesterPublicKeyValidationScenario from '../common/requesterPublicKeyValidationScenario.js';
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
import OperationAlreadyAppliedScenario from '../common/operationAlreadyAppliedScenario.js';
import RequesterNodeEntryMissingScenario from '../common/requester/requesterNodeEntryMissingScenario.js';
import RequesterBalanceScenarioBase from '../common/balances/base/requesterBalanceScenarioBase.js';
import RequesterBalanceInsufficientScenario from '../common/balances/requesterBalanceInsufficientScenario.js';
import RequesterBalanceFeeApplicationFailureScenario from '../common/balances/requesterBalanceFeeApplicationFailureScenario.js';
import {
	selectIndexerCandidatePeer,
	ensureIndexerRegistration
} from '../addIndexer/addIndexerScenarioHelpers.js';
import IndexerNodeEntryMissingScenario from '../common/indexer/indexerNodeEntryMissingScenario.js';
import IndexerNodeEntryDecodeFailureScenario from '../common/indexer/indexerNodeEntryDecodeFailureScenario.js';
import IndexerRoleUpdateFailureScenario from '../common/indexer/indexerRoleUpdateFailureScenario.js';
import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import { applyWithRequesterEntryCorruption } from '../addWriter/addWriterScenarioHelpers.js';
import {
	setupRemoveIndexerScenario,
	buildRemoveIndexerPayload,
	buildRemoveIndexerPayloadWithTxValidity,
	applyWithRemoveIndexerRoleUpdateFailure,
	assertRemoveIndexerSuccessState,
	assertRemoveIndexerFailureState,
	assertRemoveIndexerGuardFailureState
} from './removeIndexerScenarioHelpers.js';
import IndexerSequenceStateInvalidScenario from '../common/indexer/indexerSequenceStateInvalidScenario.js';

removeIndexerHappyPathScenario();
removeIndexerReAddAndRemoveAgainScenario();
removeIndexerRemoveMultipleIndexersScenario();

// Handler validation order (removeIndexer)
new AdminControlOperationValidationScenario({
	title: 'State.apply removeIndexer rejects payloads when contract schema validation fails',
	setupScenario: setupRemoveIndexerScenario,
	buildValidPayload: context => buildRemoveIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveIndexerGuardFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Contract schema validation failed.']
}).performScenario();

new RequesterAddressValidationScenario({
	title: 'State.apply removeIndexer rejects payloads when requester address is invalid',
	setupScenario: setupRemoveIndexerScenario,
	buildValidPayload: context => buildRemoveIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveIndexerGuardFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Requester address is invalid.']
}).performScenario();

createRequesterPublicKeyValidationScenario({
	title: 'State.apply removeIndexer rejects payloads when requester public key is invalid',
	setupScenario: setupRemoveIndexerScenario,
	buildValidPayload: context => buildRemoveIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveIndexerGuardFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Error while decoding requester public key.']
}).performScenario();

new InvalidAddressValidationScenario({
	title: 'State.apply removeIndexer rejects payloads when target indexer address is invalid',
	setupScenario: setupRemoveIndexerScenario,
	buildValidPayload: context => buildRemoveIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveIndexerGuardFailureState(t, context, { skipSync: true }),
	addressPath: ['aco', 'ia'],
	expectedLogs: ['Target indexer address is invalid.']
}).performScenario();

createAddressWithInvalidPublicKeyScenario({
	title: 'State.apply removeIndexer rejects payloads when target indexer public key is invalid',
	setupScenario: setupRemoveIndexerScenario,
	buildValidPayload: context => buildRemoveIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveIndexerGuardFailureState(t, context, { skipSync: true }),
	addressPath: ['aco', 'ia'],
	expectedLogs: ['Failed to decode target indexer public key.']
}).performScenario();

new AdminEntryMissingScenario({
	title: 'State.apply removeIndexer rejects payloads when admin entry is missing',
	setupScenario: setupRemoveIndexerScenario,
	buildValidPayload: context => buildRemoveIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveIndexerGuardFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Invalid admin entry.']
}).performScenario();

new AdminEntryDecodeFailureScenario({
	title: 'State.apply removeIndexer rejects payloads when admin entry cannot be decoded',
	setupScenario: setupRemoveIndexerScenario,
	buildValidPayload: context => buildRemoveIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveIndexerGuardFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Failed to decode admin entry.']
}).performScenario();

new AdminOnlyGuardScenario({
	title: 'State.apply removeIndexer rejects non-admin nodes',
	setupScenario: setupRemoveIndexerScenario,
	buildValidPayload: context => buildRemoveIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveIndexerGuardFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Node is not allowed to perform this operation. (ADMIN ONLY)']
}).performScenario();

new AdminPublicKeyDecodeFailureScenario({
	title: 'State.apply removeIndexer rejects payloads when admin public key cannot be decoded',
	setupScenario: setupRemoveIndexerScenario,
	buildValidPayload: context => buildRemoveIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveIndexerGuardFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Failed to decode admin public key.']
}).performScenario();

new AdminConsistencyMismatchScenario({
	title: 'State.apply removeIndexer rejects payloads when admin public key mismatches requester',
	setupScenario: setupRemoveIndexerScenario,
	buildValidPayload: context => buildRemoveIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveIndexerGuardFailureState(t, context, { skipSync: true }),
	expectedLogs: ['System admin and node public keys do not match.']
}).performScenario();

new InvalidHashValidationScenario({
	title: 'State.apply removeIndexer rejects payloads when message hash mismatches tx hash',
	setupScenario: setupRemoveIndexerScenario,
	buildValidPayload: context => buildRemoveIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveIndexerFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Message hash does not match the tx_hash.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply removeIndexer rejects payloads when admin signature is invalid (foreign signature)',
	setupScenario: setupRemoveIndexerScenario,
	buildValidPayload: context => buildRemoveIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveIndexerFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply removeIndexer rejects payloads when admin signature is invalid (zero fill)',
	setupScenario: setupRemoveIndexerScenario,
	buildValidPayload: context => buildRemoveIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveIndexerFailureState(t, context, { skipSync: true }),
	strategy: SignatureMutationStrategy.ZERO_FILL,
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply removeIndexer rejects payloads when admin signature is invalid (type mismatch)',
	setupScenario: setupRemoveIndexerScenario,
	buildValidPayload: context => buildRemoveIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveIndexerFailureState(t, context, { skipSync: true }),
	strategy: SignatureMutationStrategy.TYPE_MISMATCH,
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new IndexerSequenceStateInvalidScenario({
	title: 'State.apply removeIndexer rejects payloads when indexer sequence state is invalid',
	setupScenario: setupRemoveIndexerScenario,
	buildValidPayload: context => buildRemoveIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveIndexerGuardFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Indexer sequence state is invalid.']
}).performScenario();

new TransactionValidityMismatchScenario({
	title: 'State.apply removeIndexer rejects payloads when transaction validity mismatches indexer state',
	setupScenario: setupRemoveIndexerScenario,
	buildValidPayload: context => buildRemoveIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveIndexerFailureState(t, context, { skipSync: true }),
	txValidityPath: ['aco', 'txv'],
	rebuildPayloadWithTxValidity: ({ context, mutatedTxValidity }) =>
		buildRemoveIndexerPayloadWithTxValidity(context, mutatedTxValidity),
	expectedLogs: ['Transaction was not executed.']
}).performScenario();

new OperationAlreadyAppliedScenario({
	title: 'State.apply removeIndexer rejects payloads when operation was already applied',
	setupScenario: setupRemoveIndexerScenario,
	buildValidPayload: context => buildRemoveIndexerPayload(context),
	assertStateUnchanged: (t, context, validPayload) =>
		assertRemoveIndexerSuccessState(t, context, {
			indexerPeer: context.removeIndexerScenario?.indexerPeer ?? selectIndexerCandidatePeer(context),
			adminPeer: context.adminBootstrap,
			indexerEntryBefore: context.removeIndexerScenario?.indexerEntryBeforeRemoval,
			adminEntryBefore: context.removeIndexerScenario?.adminEntryBeforeRemoval,
			payload: validPayload,
			writersLengthBefore: context.removeIndexerScenario?.writersLengthBeforeRemoval,
			skipSync: true
		}),
	beforeInvalidApply: ({ context, node }) => {
		const writerKey = context.removeIndexerScenario?.indexerPeer?.base?.local?.key;
		if (!writerKey) return null;
		return ensureIndexerRegistration(node.base, writerKey);
	},
	expectedLogs: ['Operation has already been applied.']
}).performScenario();

new IndexerNodeEntryMissingScenario({
	title: 'State.apply removeIndexer rejects payloads when target indexer entry is missing',
	setupScenario: setupRemoveIndexerScenario,
	buildValidPayload: context => buildRemoveIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveIndexerFailureState(t, context, { skipSync: true }),
	selectPeer: context => context.removeIndexerScenario?.indexerPeer ?? selectIndexerCandidatePeer(context),
	expectedLogs: ['Failed to verify target indexer entry.']
}).performScenario();

new IndexerNodeEntryDecodeFailureScenario({
	title: 'State.apply removeIndexer rejects payloads when target indexer entry cannot be decoded',
	setupScenario: setupRemoveIndexerScenario,
	buildValidPayload: context => buildRemoveIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveIndexerFailureState(t, context, { skipSync: true }),
	selectPeer: context => context.removeIndexerScenario?.indexerPeer ?? selectIndexerCandidatePeer(context),
	expectedLogs: ['Failed to decode target indexer node entry.']
}).performScenario();

removeIndexerTargetNotIndexerScenario();

new IndexerRoleUpdateFailureScenario({
	title: 'State.apply removeIndexer rejects payloads when node role update fails',
	setupScenario: setupRemoveIndexerScenario,
	buildValidPayload: context => buildRemoveIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveIndexerGuardFailureState(t, context, { skipSync: true }),
	applyRoleMutationFailure: applyWithRemoveIndexerRoleUpdateFailure,
	expectedLogs: ['Failed to update node role.']
}).performScenario();

removeIndexerWriterKeyMissingScenario();

new RequesterNodeEntryMissingScenario({
	title: 'State.apply removeIndexer rejects payloads when requester node entry is missing',
	setupScenario: setupRemoveIndexerScenario,
	buildValidPayload: context => buildRemoveIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveIndexerFailureState(t, context, { skipSync: true }),
	selectPeer: context => context.adminBootstrap,
	expectedLogs: ['Invalid requester node entry.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply removeIndexer rejects payloads when requester node entry cannot be decoded',
	setupScenario: setupRemoveIndexerScenario,
	buildValidPayload: context => buildRemoveIndexerPayload(context),
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: (context, invalidPayload) =>
		applyWithRequesterEntryCorruption(context, invalidPayload, { peer: context.adminBootstrap }),
	assertStateUnchanged: (t, context) => assertRemoveIndexerFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Failed to decode requester node entry.']
}).performScenario();

new RequesterBalanceScenarioBase({
	title: 'State.apply removeIndexer rejects payloads when admin balance cannot be decoded',
	setupScenario: setupRemoveIndexerScenario,
	buildValidPayload: context => buildRemoveIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveIndexerFailureState(t, context, { skipSync: true }),
	mutateDecodedEntry: decoded => ({ ...decoded, balance: decoded.balance ? decoded.balance.subarray(0, 1) : null }),
	selectPeer: context => context.adminBootstrap,
	expectedLogs: ['Invalid admin balance.']
}).performScenario();

new RequesterBalanceInsufficientScenario({
	title: 'State.apply removeIndexer rejects payloads when requester balance is insufficient',
	setupScenario: setupRemoveIndexerScenario,
	buildValidPayload: context => buildRemoveIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveIndexerFailureState(t, context, { skipSync: true }),
	selectPeer: context => context.adminBootstrap
}).performScenario();

new RequesterBalanceFeeApplicationFailureScenario({
	title: 'State.apply removeIndexer rejects payloads when requester fee cannot be applied',
	setupScenario: setupRemoveIndexerScenario,
	buildValidPayload: context => buildRemoveIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveIndexerFailureState(t, context, { skipSync: true }),
	selectPeer: context => context.adminBootstrap
}).performScenario();

new RequesterBalanceScenarioBase({
	title: 'State.apply removeIndexer rejects payloads when requester balance update fails',
	setupScenario: setupRemoveIndexerScenario,
	buildValidPayload: context => buildRemoveIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveIndexerFailureState(t, context, { skipSync: true }),
	selectPeer: context => context.adminBootstrap,
	failNextBalanceUpdate: true,
	expectedLogs: ['Failed to update requester node.']
}).performScenario();

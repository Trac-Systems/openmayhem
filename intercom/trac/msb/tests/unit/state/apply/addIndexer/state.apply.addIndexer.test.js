import b4a from 'b4a';
import addIndexerHappyPathScenario from './addIndexerHappyPathScenario.js';
import addIndexerMultipleIndexersInTheNetworkScenario from './addIndexerMultipleIndexersInTheNetworkScenario.js';
import addIndexerRemoveAndReAddScenario from './addIndexerRemoveAndReAddScenario.js';
import addIndexerWriterKeyAlreadyRegisteredScenario from './addIndexerWriterKeyAlreadyRegisteredScenario.js';
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
import InvalidSignatureValidationScenario, {
	SignatureMutationStrategy
} from '../common/payload-structure/invalidSignatureValidationScenario.js';
import IndexerSequenceStateInvalidScenario from '../common/indexer/indexerSequenceStateInvalidScenario.js';
import TransactionValidityMismatchScenario from '../common/transactionValidityMismatchScenario.js';
import OperationAlreadyAppliedScenario from '../common/operationAlreadyAppliedScenario.js';
import RequesterBalanceScenarioBase from '../common/balances/base/requesterBalanceScenarioBase.js';
import RequesterBalanceInsufficientScenario from '../common/balances/requesterBalanceInsufficientScenario.js';
import RequesterBalanceFeeApplicationFailureScenario from '../common/balances/requesterBalanceFeeApplicationFailureScenario.js';
import addIndexerPretenderNotWriterScenario from './addIndexerPretenderNotWriterScenario.js';
import addIndexerPretenderAlreadyIndexerScenario from './addIndexerPretenderAlreadyIndexerScenario.js';
import IndexerNodeEntryMissingScenario from '../common/indexer/indexerNodeEntryMissingScenario.js';
import IndexerNodeEntryDecodeFailureScenario from '../common/indexer/indexerNodeEntryDecodeFailureScenario.js';
import IndexerRoleUpdateFailureScenario from '../common/indexer/indexerRoleUpdateFailureScenario.js';
import FeeDecodeFailureScenario from '../common/balances/feeDecodeFailureScenario.js';
import RequesterNodeEntryMissingScenario from '../common/requester/requesterNodeEntryMissingScenario.js';
import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import { applyWithRequesterEntryCorruption } from '../addWriter/addWriterScenarioHelpers.js';
import {
	setupAddIndexerScenario,
	buildAddIndexerPayload,
	buildAddIndexerPayloadWithTxValidity,
	assertAddIndexerFailureState,
	assertAddIndexerSuccessState,
	selectIndexerCandidatePeer,
	applyWithIndexerRoleUpdateFailure
} from './addIndexerScenarioHelpers.js';

addIndexerHappyPathScenario();
addIndexerMultipleIndexersInTheNetworkScenario();
addIndexerRemoveAndReAddScenario();

// Handler validation order
new AdminControlOperationValidationScenario({
	title: 'State.apply addIndexer rejects payloads when contract schema validation fails',
	setupScenario: setupAddIndexerScenario,
	buildValidPayload: context => buildAddIndexerPayload(context),
	assertStateUnchanged: assertAddIndexerFailureState,
	expectedLogs: ['Contract schema validation failed.']
}).performScenario();

new RequesterAddressValidationScenario({
	title: 'State.apply addIndexer rejects payloads when requester address is invalid',
	setupScenario: setupAddIndexerScenario,
	buildValidPayload: context => buildAddIndexerPayload(context),
	assertStateUnchanged: assertAddIndexerFailureState,
	expectedLogs: ['Requester address is invalid.']
}).performScenario();

createRequesterPublicKeyValidationScenario({
	title: 'State.apply addIndexer rejects payloads when requester public key is invalid',
	setupScenario: setupAddIndexerScenario,
	buildValidPayload: context => buildAddIndexerPayload(context),
	assertStateUnchanged: assertAddIndexerFailureState,
	expectedLogs: ['Error while decoding requester public key.']
}).performScenario();

new InvalidAddressValidationScenario({
	title: 'State.apply addIndexer rejects payloads when pretending indexer address is invalid',
	setupScenario: setupAddIndexerScenario,
	buildValidPayload: context => buildAddIndexerPayload(context),
	assertStateUnchanged: assertAddIndexerFailureState,
	addressPath: ['aco', 'ia'],
	expectedLogs: ['Pretending indexer address is invalid.']
}).performScenario();

createAddressWithInvalidPublicKeyScenario({
	title: 'State.apply addIndexer rejects payloads when pretending indexer public key is invalid',
	setupScenario: setupAddIndexerScenario,
	buildValidPayload: context => buildAddIndexerPayload(context),
	assertStateUnchanged: assertAddIndexerFailureState,
	addressPath: ['aco', 'ia'],
	expectedLogs: ['Failed to decode pretending indexer public key.']
}).performScenario();

new AdminEntryMissingScenario({
	title: 'State.apply addIndexer rejects payloads when admin entry is missing',
	setupScenario: setupAddIndexerScenario,
	buildValidPayload: context => buildAddIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertAddIndexerFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Invalid admin entry.']
}).performScenario();

new AdminEntryDecodeFailureScenario({
	title: 'State.apply addIndexer rejects payloads when admin entry cannot be decoded',
	setupScenario: setupAddIndexerScenario,
	buildValidPayload: context => buildAddIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertAddIndexerFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Failed to decode admin entry.']
}).performScenario();

new AdminOnlyGuardScenario({
	title: 'State.apply addIndexer rejects non-admin nodes',
	setupScenario: setupAddIndexerScenario,
	buildValidPayload: context => buildAddIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertAddIndexerFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Node is not allowed to perform this operation. (ADMIN ONLY)']
}).performScenario();

new AdminPublicKeyDecodeFailureScenario({
	title: 'State.apply addIndexer rejects payloads when admin public key cannot be decoded',
	setupScenario: setupAddIndexerScenario,
	buildValidPayload: context => buildAddIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertAddIndexerFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Failed to decode admin public key.']
}).performScenario();

new AdminConsistencyMismatchScenario({
	title: 'State.apply addIndexer rejects payloads when admin public key mismatches requester',
	setupScenario: setupAddIndexerScenario,
	buildValidPayload: context => buildAddIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertAddIndexerFailureState(t, context, { skipSync: true }),
	expectedLogs: ['System admin and node public keys do not match.']
}).performScenario();

new InvalidHashValidationScenario({
	title: 'State.apply addIndexer rejects payloads when message hash mismatches tx hash',
	setupScenario: setupAddIndexerScenario,
	buildValidPayload: context => buildAddIndexerPayload(context),
	assertStateUnchanged: assertAddIndexerFailureState,
	expectedLogs: ['Message hash does not match the tx_hash.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply addIndexer rejects payloads when admin signature is invalid (foreign signature)',
	setupScenario: setupAddIndexerScenario,
	buildValidPayload: context => buildAddIndexerPayload(context),
	assertStateUnchanged: assertAddIndexerFailureState,
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply addIndexer rejects payloads when admin signature is invalid (zero fill)',
	setupScenario: setupAddIndexerScenario,
	buildValidPayload: context => buildAddIndexerPayload(context),
	assertStateUnchanged: assertAddIndexerFailureState,
	strategy: SignatureMutationStrategy.ZERO_FILL,
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply addIndexer rejects payloads when admin signature is invalid (type mismatch)',
	setupScenario: setupAddIndexerScenario,
	buildValidPayload: context => buildAddIndexerPayload(context),
	assertStateUnchanged: assertAddIndexerFailureState,
	strategy: SignatureMutationStrategy.TYPE_MISMATCH,
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new IndexerSequenceStateInvalidScenario({
	title: 'State.apply addIndexer rejects payloads when indexer sequence state is invalid',
	setupScenario: setupAddIndexerScenario,
	buildValidPayload: context => buildAddIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertAddIndexerFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Indexer sequence state is invalid.']
}).performScenario();

new TransactionValidityMismatchScenario({
	title: 'State.apply addIndexer rejects payloads when transaction validity mismatches indexer state',
	setupScenario: setupAddIndexerScenario,
	buildValidPayload: context => buildAddIndexerPayload(context),
	assertStateUnchanged: assertAddIndexerFailureState,
	txValidityPath: ['aco', 'txv'],
	rebuildPayloadWithTxValidity: ({ context, mutatedTxValidity }) =>
		buildAddIndexerPayloadWithTxValidity(context, mutatedTxValidity),
	expectedLogs: ['Transaction was not executed.']
}).performScenario();

new OperationAlreadyAppliedScenario({
	title: 'State.apply addIndexer rejects payloads when operation was already applied',
	setupScenario: setupAddIndexerScenario,
	buildValidPayload: context => buildAddIndexerPayload(context),
	assertStateUnchanged: (t, context, validPayload) =>
		assertAddIndexerSuccessState(t, context, {
			writerPeer: context.addIndexerScenario?.writerPeer ?? selectIndexerCandidatePeer(context),
			adminPeer: context.adminBootstrap,
			writerEntryBefore: context.addIndexerScenario?.writerEntryBefore,
			adminEntryBefore: context.addIndexerScenario?.adminEntryBefore,
			payload: validPayload
		}),
	expectedLogs: ['Operation has already been applied.']
}).performScenario();

new IndexerNodeEntryMissingScenario({
	title: 'State.apply addIndexer rejects payloads when target indexer entry is missing',
	setupScenario: setupAddIndexerScenario,
	buildValidPayload: context => buildAddIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertAddIndexerFailureState(t, context, { skipSync: true }),
	selectPeer: context => context.addIndexerScenario?.writerPeer ?? selectIndexerCandidatePeer(context),
	expectedLogs: ['Failed to verify target indexer entry.']
}).performScenario();

new IndexerNodeEntryDecodeFailureScenario({
	title: 'State.apply addIndexer rejects payloads when target indexer entry cannot be decoded',
	setupScenario: setupAddIndexerScenario,
	buildValidPayload: context => buildAddIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertAddIndexerFailureState(t, context, { skipSync: true }),
	selectPeer: context => context.addIndexerScenario?.writerPeer ?? selectIndexerCandidatePeer(context),
	expectedLogs: ['Failed to decode pretender indexer node entry.']
}).performScenario();

addIndexerPretenderNotWriterScenario();
addIndexerPretenderAlreadyIndexerScenario();

new IndexerRoleUpdateFailureScenario({
	title: 'State.apply addIndexer rejects payloads when node role update fails',
	setupScenario: setupAddIndexerScenario,
	buildValidPayload: context => buildAddIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertAddIndexerFailureState(t, context, { skipSync: true }),
	applyRoleMutationFailure: applyWithIndexerRoleUpdateFailure,
	expectedLogs: ['Failed to update node role.']
}).performScenario();

addIndexerWriterKeyAlreadyRegisteredScenario();

new FeeDecodeFailureScenario({
	title: 'State.apply addIndexer rejects payloads when fee cannot be decoded',
	setupScenario: setupAddIndexerScenario,
	buildValidPayload: context => buildAddIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertAddIndexerFailureState(t, context, { skipSync: true }),
	selectNode: context => context.adminBootstrap,
	expectedLogs: ['Invalid fee amount.']
}).performScenario();

new RequesterNodeEntryMissingScenario({
	title: 'State.apply addIndexer rejects payloads when requester node entry is missing',
	setupScenario: setupAddIndexerScenario,
	buildValidPayload: context => buildAddIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertAddIndexerFailureState(t, context, { skipSync: true }),
	selectPeer: context => context.adminBootstrap,
	expectedLogs: ['Invalid requester node entry buffer.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply addIndexer rejects payloads when requester node entry cannot be decoded',
	setupScenario: setupAddIndexerScenario,
	buildValidPayload: context => buildAddIndexerPayload(context),
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: (context, invalidPayload) =>
		applyWithRequesterEntryCorruption(context, invalidPayload, { peer: context.adminBootstrap }),
	assertStateUnchanged: (t, context) => assertAddIndexerFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Failed to decode requester node entry.']
}).performScenario();

new RequesterBalanceScenarioBase({
	title: 'State.apply addIndexer rejects payloads when admin balance cannot be decoded',
	setupScenario: setupAddIndexerScenario,
	buildValidPayload: context => buildAddIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertAddIndexerFailureState(t, context, { skipSync: true }),
	mutateDecodedEntry: decoded => ({ ...decoded, balance: b4a.alloc(1) }),
	selectPeer: context => context.adminBootstrap,
	expectedLogs: ['Invalid admin balance.']
}).performScenario();

new RequesterBalanceInsufficientScenario({
	title: 'State.apply addIndexer rejects payloads when requester balance is insufficient',
	setupScenario: setupAddIndexerScenario,
	buildValidPayload: context => buildAddIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertAddIndexerFailureState(t, context, { skipSync: true }),
	selectPeer: context => context.adminBootstrap,
	expectedLogs: ['Insufficient requester balance.']
}).performScenario();

new RequesterBalanceFeeApplicationFailureScenario({
	title: 'State.apply addIndexer rejects payloads when requester fee cannot be applied',
	setupScenario: setupAddIndexerScenario,
	buildValidPayload: context => buildAddIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertAddIndexerFailureState(t, context, { skipSync: true }),
	selectPeer: context => context.adminBootstrap,
	expectedLogs: ['Failed to apply fee to requester balance.']
}).performScenario();

new RequesterBalanceScenarioBase({
	title: 'State.apply addIndexer rejects payloads when requester balance update fails',
	setupScenario: setupAddIndexerScenario,
	buildValidPayload: context => buildAddIndexerPayload(context),
	assertStateUnchanged: (t, context) => assertAddIndexerFailureState(t, context, { skipSync: true }),
	selectPeer: context => context.adminBootstrap,
	failNextBalanceUpdate: true,
	expectedLogs: ['Failed to update requester node.']
}).performScenario();

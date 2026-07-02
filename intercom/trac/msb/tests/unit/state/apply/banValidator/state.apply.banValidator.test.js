import banValidatorHappyPathScenario from './banValidatorHappyPathScenario.js';
import banValidatorWhitelistedNonWriterScenario from './banValidatorWhitelistedNonWriterScenario.js';
import banValidatorWhitelistedZeroBalanceScenario from './banValidatorWhitelistedZeroBalanceScenario.js';
import banValidatorSequentialBansScenario from './banValidatorSequentialBansScenario.js';
import banValidatorBanAndReWhitelistScenario from './banValidatorBanAndReWhitelistScenario.js';
import b4a from 'b4a';
import AdminControlOperationValidationScenario from '../common/adminControlOperationValidationScenario.js';
import RequesterAddressValidationScenario from '../common/requesterAddressValidationScenario.js';
import createRequesterPublicKeyValidationScenario from '../common/requesterPublicKeyValidationScenario.js';
import AdminEntryMissingScenario from '../common/access-control/adminEntryMissingScenario.js';
import AdminEntryDecodeFailureScenario from '../common/access-control/adminEntryDecodeFailureScenario.js';
import AdminOnlyGuardScenario from '../common/access-control/adminOnlyGuardScenario.js';
import AdminPublicKeyDecodeFailureScenario from '../common/access-control/adminPublicKeyDecodeFailureScenario.js';
import AdminConsistencyMismatchScenario from '../common/access-control/adminConsistencyMismatchScenario.js';
import InvalidHashValidationScenario from '../common/payload-structure/invalidHashValidationScenario.js';
import InvalidAddressValidationScenario from '../common/payload-structure/invalidAddressValidationScenario.js';
import InvalidSignatureValidationScenario, {
	SignatureMutationStrategy
} from '../common/payload-structure/invalidSignatureValidationScenario.js';
import IndexerSequenceStateInvalidScenario from '../common/indexer/indexerSequenceStateInvalidScenario.js';
import TransactionValidityMismatchScenario from '../common/transactionValidityMismatchScenario.js';
import OperationAlreadyAppliedScenario from '../common/operationAlreadyAppliedScenario.js';
import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import FeeDecodeFailureScenario from '../common/balances/feeDecodeFailureScenario.js';
import RequesterNodeEntryMissingScenario from '../common/requester/requesterNodeEntryMissingScenario.js';
import RequesterBalanceScenarioBase from '../common/balances/base/requesterBalanceScenarioBase.js';
import {
	setupBanValidatorScenario,
	buildBanValidatorPayload,
	buildBanValidatorPayloadWithTxValidity,
	assertBanValidatorFailureState,
	applyInvalidTargetAddressPayload
} from './banValidatorScenarioHelpers.js';
import banValidatorTargetNodeEntryMissingScenario from './banValidatorTargetNodeEntryMissingScenario.js';
import banValidatorTargetIndexerScenario from './banValidatorTargetIndexerScenario.js';
import banValidatorTargetRoleUpdateFailureScenario from './banValidatorTargetRoleUpdateFailureScenario.js';
import banValidatorTargetDecodeFailureScenario from './banValidatorTargetDecodeFailureScenario.js';
import banValidatorWithdrawFailureScenario from './banValidatorWithdrawFailureScenario.js';
import { applyWithRequesterEntryCorruption } from '../addWriter/addWriterScenarioHelpers.js';

banValidatorHappyPathScenario();
banValidatorWhitelistedNonWriterScenario();
banValidatorWhitelistedZeroBalanceScenario();
banValidatorSequentialBansScenario();
banValidatorBanAndReWhitelistScenario();

// Handler validation order
new AdminControlOperationValidationScenario({
	title: 'State.apply banValidator rejects payloads when contract schema validation fails',
	setupScenario: setupBanValidatorScenario,
	buildValidPayload: context => buildBanValidatorPayload(context),
	assertStateUnchanged: assertBanValidatorFailureState,
	expectedLogs: ['Contract schema validation failed.']
}).performScenario();

new RequesterAddressValidationScenario({
	title: 'State.apply banValidator rejects payloads when requester address is invalid',
	setupScenario: setupBanValidatorScenario,
	buildValidPayload: context => buildBanValidatorPayload(context),
	assertStateUnchanged: assertBanValidatorFailureState,
	expectedLogs: ['Requester address is invalid.']
}).performScenario();

createRequesterPublicKeyValidationScenario({
	title: 'State.apply banValidator rejects payloads when requester public key is invalid',
	setupScenario: setupBanValidatorScenario,
	buildValidPayload: context => buildBanValidatorPayload(context),
	assertStateUnchanged: assertBanValidatorFailureState,
	expectedLogs: ['Error while decoding requester public key.']
}).performScenario();

new AdminEntryMissingScenario({
	title: 'State.apply banValidator rejects payloads when admin entry is missing',
	setupScenario: setupBanValidatorScenario,
	buildValidPayload: context => buildBanValidatorPayload(context),
	assertStateUnchanged: (t, context) => assertBanValidatorFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Invalid admin entry.']
}).performScenario();

new AdminEntryDecodeFailureScenario({
	title: 'State.apply banValidator rejects payloads when admin entry cannot be decoded',
	setupScenario: setupBanValidatorScenario,
	buildValidPayload: context => buildBanValidatorPayload(context),
	assertStateUnchanged: (t, context) => assertBanValidatorFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Failed to decode admin node entry.']
}).performScenario();

new AdminPublicKeyDecodeFailureScenario({
	title: 'State.apply banValidator rejects payloads when admin public key cannot be decoded',
	setupScenario: setupBanValidatorScenario,
	buildValidPayload: context => buildBanValidatorPayload(context),
	assertStateUnchanged: (t, context) => assertBanValidatorFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Failed to decode admin public key.']
}).performScenario();

new AdminOnlyGuardScenario({
	title: 'State.apply banValidator rejects non-admin nodes',
	setupScenario: setupBanValidatorScenario,
	buildValidPayload: context => buildBanValidatorPayload(context),
	assertStateUnchanged: (t, context) =>
		assertBanValidatorFailureState(t, context, {
			expectedRoles: { isWhitelisted: true, isWriter: false, isIndexer: false },
			allowEntryMutation: true,
			skipSync: true
		}),
	expectedLogs: ['Node is not allowed to perform this operation. (ADMIN ONLY)']
}).performScenario();

new AdminConsistencyMismatchScenario({
	title: 'State.apply banValidator rejects payloads when admin public key mismatches requester',
	setupScenario: setupBanValidatorScenario,
	buildValidPayload: context => buildBanValidatorPayload(context),
	assertStateUnchanged: (t, context) => assertBanValidatorFailureState(t, context, { skipSync: true }),
	expectedLogs: ['System admin and node public keys do not match.']
}).performScenario();

new InvalidHashValidationScenario({
	title: 'State.apply banValidator rejects payloads when message hash mismatches tx hash',
	setupScenario: setupBanValidatorScenario,
	buildValidPayload: context => buildBanValidatorPayload(context),
	assertStateUnchanged: assertBanValidatorFailureState,
	expectedLogs: ['Message hash does not match the tx_hash.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply banValidator rejects payloads when admin signature is invalid (foreign signature)',
	setupScenario: setupBanValidatorScenario,
	buildValidPayload: context => buildBanValidatorPayload(context),
	assertStateUnchanged: assertBanValidatorFailureState,
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply banValidator rejects payloads when admin signature is invalid (zero fill)',
	setupScenario: setupBanValidatorScenario,
	buildValidPayload: context => buildBanValidatorPayload(context),
	assertStateUnchanged: assertBanValidatorFailureState,
	strategy: SignatureMutationStrategy.ZERO_FILL,
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply banValidator rejects payloads when admin signature is invalid (type mismatch)',
	setupScenario: setupBanValidatorScenario,
	buildValidPayload: context => buildBanValidatorPayload(context),
	assertStateUnchanged: assertBanValidatorFailureState,
	strategy: SignatureMutationStrategy.TYPE_MISMATCH,
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new IndexerSequenceStateInvalidScenario({
	title: 'State.apply banValidator rejects payloads when indexer sequence state is invalid',
	setupScenario: setupBanValidatorScenario,
	buildValidPayload: context => buildBanValidatorPayload(context),
	assertStateUnchanged: (t, context) => assertBanValidatorFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Indexer sequence state is invalid.']
}).performScenario();

new TransactionValidityMismatchScenario({
	title: 'State.apply banValidator rejects payloads when transaction validity mismatches indexer state',
	setupScenario: setupBanValidatorScenario,
	buildValidPayload: context => buildBanValidatorPayload(context),
	assertStateUnchanged: assertBanValidatorFailureState,
	txValidityPath: ['aco', 'txv'],
	rebuildPayloadWithTxValidity: ({ context, mutatedTxValidity }) =>
		buildBanValidatorPayloadWithTxValidity(context, mutatedTxValidity),
	expectedLogs: ['Transaction was not executed.']
}).performScenario();

new OperationAlreadyAppliedScenario({
	title: 'State.apply banValidator rejects payloads when operation was already applied',
	setupScenario: setupBanValidatorScenario,
	buildValidPayload: context => buildBanValidatorPayload(context),
	assertStateUnchanged: (t, context) =>
		assertBanValidatorFailureState(t, context, {
			expectedRoles: { isWhitelisted: false, isWriter: false, isIndexer: false },
			allowEntryMutation: true,
			skipSync: true
		}),
	expectedLogs: ['Operation has already been applied.']
}).performScenario();

new InvalidAddressValidationScenario({
	title: 'State.apply banValidator rejects payloads when target node address is invalid',
	setupScenario: setupBanValidatorScenario,
	buildValidPayload: context => buildBanValidatorPayload(context),
	assertStateUnchanged: assertBanValidatorFailureState,
	addressPath: ['aco', 'ia'],
	expectedLogs: ['Failed to verify target node address.'],
	applyInvalidPayload: applyInvalidTargetAddressPayload
}).performScenario();

banValidatorTargetNodeEntryMissingScenario();
banValidatorTargetIndexerScenario();
banValidatorTargetRoleUpdateFailureScenario();
banValidatorTargetDecodeFailureScenario();

new FeeDecodeFailureScenario({
	title: 'State.apply banValidator rejects payloads when fee amount is invalid',
	setupScenario: setupBanValidatorScenario,
	buildValidPayload: context => buildBanValidatorPayload(context),
	assertStateUnchanged: (t, context) => assertBanValidatorFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Invalid fee amount.']
}).performScenario();

new RequesterNodeEntryMissingScenario({
	title: 'State.apply banValidator rejects payloads when admin node entry is missing',
	setupScenario: setupBanValidatorScenario,
	buildValidPayload: context => buildBanValidatorPayload(context),
	assertStateUnchanged: (t, context) => assertBanValidatorFailureState(t, context, { skipSync: true }),
	selectPeer: context => context.adminBootstrap,
	expectedLogs: ['Invalid admin node entry buffer.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply banValidator rejects payloads when admin node entry cannot be decoded',
	setupScenario: setupBanValidatorScenario,
	buildValidPayload: context => buildBanValidatorPayload(context),
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: (context, invalidPayload) =>
		applyWithRequesterEntryCorruption(context, invalidPayload, { peer: context.adminBootstrap }),
	assertStateUnchanged: (t, context) => assertBanValidatorFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Failed to verify admin node entry.']
}).performScenario();

new RequesterBalanceScenarioBase({
	title: 'State.apply banValidator rejects payloads when admin balance cannot be decoded',
	setupScenario: setupBanValidatorScenario,
	buildValidPayload: context => buildBanValidatorPayload(context),
	assertStateUnchanged: (t, context) => assertBanValidatorFailureState(t, context, { skipSync: true }),
	mutateDecodedEntry: decoded => ({ ...decoded, balance: decoded.balance ? decoded.balance.subarray(0, 1) : null }),
	selectPeer: context => context.adminBootstrap,
	expectedLogs: ['Invalid admin balance']
}).performScenario();

new RequesterBalanceScenarioBase({
	title: 'State.apply banValidator rejects payloads when admin balance is insufficient',
	setupScenario: setupBanValidatorScenario,
	buildValidPayload: context => buildBanValidatorPayload(context),
	assertStateUnchanged: (t, context) => assertBanValidatorFailureState(t, context, { skipSync: true }),
	mutateDecodedEntry: decoded => ({ ...decoded, balance: b4a.alloc(decoded.balance.length) }),
	selectPeer: context => context.adminBootstrap,
	expectedLogs: ['Insufficient admin balance.']
}).performScenario();

new RequesterBalanceScenarioBase({
	title: 'State.apply banValidator rejects payloads when admin fee cannot be applied',
	setupScenario: setupBanValidatorScenario,
	buildValidPayload: context => buildBanValidatorPayload(context),
	assertStateUnchanged: (t, context) => assertBanValidatorFailureState(t, context, { skipSync: true }),
	selectPeer: context => context.adminBootstrap,
	failNextBalanceSub: true,
	expectedLogs: ['Failed to apply fee to admin balance.']
}).performScenario();

new RequesterBalanceScenarioBase({
	title: 'State.apply banValidator rejects payloads when admin balance update fails',
	setupScenario: setupBanValidatorScenario,
	buildValidPayload: context => buildBanValidatorPayload(context),
	assertStateUnchanged: (t, context) => assertBanValidatorFailureState(t, context, { skipSync: true }),
	selectPeer: context => context.adminBootstrap,
	failNextBalanceUpdate: true,
	expectedLogs: ['Failed to update admin node balance.']
}).performScenario();

banValidatorWithdrawFailureScenario();

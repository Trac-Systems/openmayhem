import removeWriterHappyPathScenario from './removeWriterHappyPathScenario.js';
import removeWriterAndAddWriterAgainScenario from './removeWriterAndAddWriterAgainScenario.js';
import removeWriterThroughWriterValidatorScenario from './removeWriterThroughWriterValidatorScenario.js';
import removeWriterInvalidValidatorSignatureScenario from './removeWriterInvalidValidatorSignatureScenario.js';
import removeWriterRequesterEntryMissingScenario from './removeWriterRequesterEntryMissingScenario.js';
import removeWriterRequesterEntryDecodeFailureScenario from './removeWriterRequesterEntryDecodeFailureScenario.js';
import removeWriterRequesterNotWriterScenario from './removeWriterRequesterNotWriterScenario.js';
import removeWriterRequesterIndexerScenario from './removeWriterRequesterIndexerScenario.js';
import removeWriterRequesterRoleUpdateFailureScenario from './removeWriterRequesterRoleUpdateFailureScenario.js';
import removeWriterWriterKeyRegistryMissingScenario from './removeWriterWriterKeyRegistryMissingScenario.js';
import removeWriterWriterKeyMismatchScenario from './removeWriterWriterKeyMismatchScenario.js';
import removeWriterWriterKeyOwnershipScenario from './removeWriterWriterKeyOwnershipScenario.js';
import removeWriterUnstakeFailureScenario from './removeWriterUnstakeFailureScenario.js';
import removeWriterRequesterBalanceInsufficientScenario from './removeWriterRequesterBalanceInsufficientScenario.js';
import RoleAccessOperationValidationScenario from '../common/access-control/roleAccessOperationValidationScenario.js';
import RequesterAddressValidationScenario from '../common/requesterAddressValidationScenario.js';
import createRequesterPublicKeyValidationScenario from '../common/requesterPublicKeyValidationScenario.js';
import PartialOperationValidationScenario, {
	PartialOperationMutationStrategy
} from '../common/payload-structure/partialOperationValidationScenario.js';
import InvalidHashValidationScenario from '../common/payload-structure/invalidHashValidationScenario.js';
import InvalidSignatureValidationScenario, {
	SignatureMutationStrategy
} from '../common/payload-structure/invalidSignatureValidationScenario.js';
import InvalidAddressValidationScenario from '../common/payload-structure/invalidAddressValidationScenario.js';
import createAddressWithInvalidPublicKeyScenario from '../common/payload-structure/addressWithInvalidPublicKeyScenario.js';
import TransactionValidityMismatchScenario from '../common/transactionValidityMismatchScenario.js';
import IndexerSequenceStateInvalidScenario from '../common/indexer/indexerSequenceStateInvalidScenario.js';
import OperationAlreadyAppliedScenario from '../common/operationAlreadyAppliedScenario.js';
import RequesterBalanceDecodeFailureScenario from '../common/balances/requesterBalanceDecodeFailureScenario.js';
import RequesterBalanceFeeApplicationFailureScenario from '../common/balances/requesterBalanceFeeApplicationFailureScenario.js';
import RequesterBalanceUpdateFailureScenario from '../common/balances/requesterBalanceUpdateFailureScenario.js';
import ValidatorEntryMissingScenario from '../common/validatorConsistency/validatorEntryMissingScenario.js';
import ValidatorEntryDecodeFailureScenario from '../common/validatorConsistency/validatorEntryDecodeFailureScenario.js';
import ValidatorInactiveScenario from '../common/validatorConsistency/validatorInactiveScenario.js';
import ValidatorWriterKeyMismatchScenario from '../common/validatorConsistency/validatorWriterKeyMismatchScenario.js';
import ValidatorEntryInvalidBalanceScenario from '../common/validatorEntryValidation/validatorEntryInvalidBalanceScenario.js';
import ValidatorEntryRewardFailureScenario from '../common/balances/validatorEntryRewardFailureScenario.js';
import ValidatorEntryUpdateFailureScenario from '../common/balances/validatorEntryUpdateFailureScenario.js';
import {
	setupRemoveWriterScenario,
	buildRemoveWriterPayload,
	buildRemoveWriterPayloadWithTxValidity,
	assertRemoveWriterFailureState,
	snapshotDowngradedWriterEntry,
	assertDowngradedWriterSnapshot,
	selectWriterPeer
} from './removeWriterScenarioHelpers.js';
import {
	applyWithMissingComponentBypass,
	selectValidatorPeerWithoutEntry
} from '../addWriter/addWriterScenarioHelpers.js';

removeWriterHappyPathScenario();
removeWriterAndAddWriterAgainScenario();
removeWriterThroughWriterValidatorScenario();

new RoleAccessOperationValidationScenario({
	title: 'State.apply removeWriter rejects payloads when contract schema validation fails',
	setupScenario: setupRemoveWriterScenario,
	buildValidPayload: context => buildRemoveWriterPayload(context),
	assertStateUnchanged: assertRemoveWriterFailureState,
	expectedLogs: ['Contract schema validation failed.']
}).performScenario();

new PartialOperationValidationScenario({
	title: 'State.apply removeWriter rejects incomplete validator co-signatures',
	setupScenario: setupRemoveWriterScenario,
	buildValidPayload: context => buildRemoveWriterPayload(context),
	assertStateUnchanged: assertRemoveWriterFailureState,
	strategy: PartialOperationMutationStrategy.MISSING_COMPONENT,
	parentKey: 'rao',
	applyInvalidPayload: applyWithMissingComponentBypass,
	expectedLogs: ['Operation is not complete.']
}).performScenario();

new PartialOperationValidationScenario({
	title: 'State.apply removeWriter rejects payloads when nonces match',
	setupScenario: setupRemoveWriterScenario,
	buildValidPayload: context => buildRemoveWriterPayload(context),
	assertStateUnchanged: assertRemoveWriterFailureState,
	strategy: PartialOperationMutationStrategy.NONCE_MATCH,
	parentKey: 'rao',
	expectedLogs: ['Nonces should not be the same.']
}).performScenario();

new PartialOperationValidationScenario({
	title: 'State.apply removeWriter rejects payloads when validator shares requester address',
	setupScenario: setupRemoveWriterScenario,
	buildValidPayload: context => buildRemoveWriterPayload(context),
	assertStateUnchanged: assertRemoveWriterFailureState,
	strategy: PartialOperationMutationStrategy.ADDRESS_MATCH,
	parentKey: 'rao',
	expectedLogs: ['Addresses should be different.']
}).performScenario();

new PartialOperationValidationScenario({
	title: 'State.apply removeWriter rejects payloads when validator signature duplicates requester signature',
	setupScenario: setupRemoveWriterScenario,
	buildValidPayload: context => buildRemoveWriterPayload(context),
	assertStateUnchanged: assertRemoveWriterFailureState,
	strategy: PartialOperationMutationStrategy.SIGNATURE_MATCH,
	parentKey: 'rao',
	expectedLogs: ['Signatures should be different.']
}).performScenario();

new RequesterAddressValidationScenario({
	title: 'State.apply removeWriter rejects payloads when requester address is invalid',
	setupScenario: setupRemoveWriterScenario,
	buildValidPayload: context => buildRemoveWriterPayload(context),
	assertStateUnchanged: assertRemoveWriterFailureState,
	expectedLogs: ['Requester address is invalid.']
}).performScenario();

createRequesterPublicKeyValidationScenario({
	title: 'State.apply removeWriter rejects payloads when requester public key is invalid',
	setupScenario: setupRemoveWriterScenario,
	buildValidPayload: context => buildRemoveWriterPayload(context),
	assertStateUnchanged: assertRemoveWriterFailureState,
	expectedLogs: ['Error while decoding requester public key.']
}).performScenario();

new InvalidHashValidationScenario({
	title: 'State.apply removeWriter rejects payloads when requester message hash mismatches',
	setupScenario: setupRemoveWriterScenario,
	buildValidPayload: context => buildRemoveWriterPayload(context),
	assertStateUnchanged: assertRemoveWriterFailureState,
	expectedLogs: ['Message hash does not match the tx_hash.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply removeWriter rejects payloads when requester signature is invalid (foreign signature)',
	setupScenario: setupRemoveWriterScenario,
	buildValidPayload: context => buildRemoveWriterPayload(context),
	assertStateUnchanged: assertRemoveWriterFailureState,
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply removeWriter rejects payloads when requester signature is invalid (zero fill)',
	setupScenario: setupRemoveWriterScenario,
	buildValidPayload: context => buildRemoveWriterPayload(context),
	assertStateUnchanged: assertRemoveWriterFailureState,
	strategy: SignatureMutationStrategy.ZERO_FILL,
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply removeWriter rejects payloads when requester signature is invalid (type mismatch)',
	setupScenario: setupRemoveWriterScenario,
	buildValidPayload: context => buildRemoveWriterPayload(context),
	assertStateUnchanged: assertRemoveWriterFailureState,
	strategy: SignatureMutationStrategy.TYPE_MISMATCH,
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new InvalidAddressValidationScenario({
	title: 'State.apply removeWriter rejects payloads when validator address is invalid',
	setupScenario: setupRemoveWriterScenario,
	buildValidPayload: context => buildRemoveWriterPayload(context),
	assertStateUnchanged: assertRemoveWriterFailureState,
	addressPath: ['rao', 'va'],
	expectedLogs: ['Failed to verify validator address.']
}).performScenario();

createAddressWithInvalidPublicKeyScenario({
	title: 'State.apply removeWriter rejects payloads when validator public key is invalid',
	setupScenario: setupRemoveWriterScenario,
	buildValidPayload: context => buildRemoveWriterPayload(context),
	assertStateUnchanged: assertRemoveWriterFailureState,
	addressPath: ['rao', 'va'],
	expectedLogs: ['Failed to decode validator public key.']
}).performScenario();

removeWriterInvalidValidatorSignatureScenario();

new IndexerSequenceStateInvalidScenario({
	title: 'State.apply removeWriter rejects payloads when indexer sequence state is invalid',
	setupScenario: setupRemoveWriterScenario,
	buildValidPayload: context => buildRemoveWriterPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveWriterFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Indexer sequence state is invalid.']
}).performScenario();

new TransactionValidityMismatchScenario({
	title: 'State.apply removeWriter rejects payloads when tx validity mismatches indexer state',
	setupScenario: setupRemoveWriterScenario,
	buildValidPayload: context => buildRemoveWriterPayload(context),
	assertStateUnchanged: assertRemoveWriterFailureState,
	txValidityPath: ['rao', 'txv'],
	rebuildPayloadWithTxValidity: ({ context, mutatedTxValidity }) =>
		buildRemoveWriterPayloadWithTxValidity(context, mutatedTxValidity),
	expectedLogs: ['Transaction was not executed.']
}).performScenario();

new ValidatorEntryMissingScenario({
	title: 'State.apply removeWriter rejects payloads when validator entry is missing',
	setupScenario: t => setupRemoveWriterScenario(t, { nodes: 3 }),
	buildValidPayload: context => {
		const validatorPeer = selectValidatorPeerWithoutEntry(context);
		if (!validatorPeer) {
			throw new Error('Validator entry missing scenario requires an extra peer.');
		}
		return buildRemoveWriterPayload(context, { validatorPeer });
	},
	assertStateUnchanged: assertRemoveWriterFailureState,
	expectedLogs: ['Incoming validator entry is null.']
}).performScenario();

new ValidatorEntryDecodeFailureScenario({
	title: 'State.apply removeWriter rejects payloads when validator entry cannot be decoded',
	setupScenario: setupRemoveWriterScenario,
	buildValidPayload: context => buildRemoveWriterPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveWriterFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Failed to decode validator entry.']
}).performScenario();

new ValidatorInactiveScenario({
	title: 'State.apply removeWriter rejects payloads when validator is not an active writer',
	setupScenario: setupRemoveWriterScenario,
	buildValidPayload: context => buildRemoveWriterPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveWriterFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Operation validator is not active']
}).performScenario();

new ValidatorWriterKeyMismatchScenario({
	title: 'State.apply removeWriter rejects payloads when validator writer key mismatches requester',
	setupScenario: setupRemoveWriterScenario,
	buildValidPayload: context => buildRemoveWriterPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveWriterFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Validator cannot be the same as requester.']
}).performScenario();

new OperationAlreadyAppliedScenario({
	title: 'State.apply removeWriter rejects duplicate operations',
	setupScenario: setupRemoveWriterScenario,
	buildValidPayload: context => buildRemoveWriterPayload(context),
	beforeInvalidApply: async ({ context }) => {
		await snapshotDowngradedWriterEntry(context);
	},
	assertStateUnchanged: (t, context) => assertDowngradedWriterSnapshot(t, context),
	expectedLogs: ['Operation has already been applied.']
}).performScenario();

removeWriterRequesterEntryMissingScenario();
removeWriterRequesterEntryDecodeFailureScenario();

removeWriterRequesterNotWriterScenario();
removeWriterRequesterIndexerScenario();
removeWriterWriterKeyRegistryMissingScenario();
removeWriterWriterKeyMismatchScenario();
removeWriterWriterKeyOwnershipScenario();
removeWriterUnstakeFailureScenario();

new RequesterBalanceDecodeFailureScenario({
	title: 'State.apply removeWriter rejects payloads when requester balance cannot be verified',
	setupScenario: setupRemoveWriterScenario,
	buildValidPayload: context => buildRemoveWriterPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveWriterFailureState(t, context, { skipSync: true }),
	selectPeer: selectWriterPeer,
	expectedLogs: ['Invalid requester balance.']
}).performScenario();

removeWriterRequesterBalanceInsufficientScenario();

new RequesterBalanceFeeApplicationFailureScenario({
	title: 'State.apply removeWriter rejects payloads when fee cannot be applied to requester balance',
	setupScenario: setupRemoveWriterScenario,
	buildValidPayload: context => buildRemoveWriterPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveWriterFailureState(t, context, { skipSync: true }),
	selectPeer: selectWriterPeer,
	expectedLogs: ['Failed to apply fee to requester balance.']
}).performScenario();

removeWriterRequesterRoleUpdateFailureScenario();

new RequesterBalanceUpdateFailureScenario({
	title: 'State.apply removeWriter rejects payloads when requester balance cannot be updated',
	setupScenario: setupRemoveWriterScenario,
	buildValidPayload: context => buildRemoveWriterPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveWriterFailureState(t, context, { skipSync: true }),
	selectPeer: selectWriterPeer,
	expectedLogs: ['Failed to update node balance.']
}).performScenario();

new ValidatorEntryInvalidBalanceScenario({
	title: 'State.apply removeWriter rejects payloads when validator balance is invalid',
	setupScenario: setupRemoveWriterScenario,
	buildValidPayload: context => buildRemoveWriterPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveWriterFailureState(t, context, { skipSync: true })
}).performScenario();

new ValidatorEntryRewardFailureScenario({
	title: 'State.apply removeWriter rejects payloads when validator fee transfer fails',
	setupScenario: setupRemoveWriterScenario,
	buildValidPayload: context => buildRemoveWriterPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveWriterFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Failed to transfer fee to validator balance.']
}).performScenario();

new ValidatorEntryUpdateFailureScenario({
	title: 'State.apply removeWriter rejects payloads when validator balance update fails',
	setupScenario: setupRemoveWriterScenario,
	buildValidPayload: context => buildRemoveWriterPayload(context),
	assertStateUnchanged: (t, context) => assertRemoveWriterFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Failed to update validator balance.']
}).performScenario();

import addWriterHappyPathScenario from './addWriterHappyPathScenario.js';
import addWriterNewWkScenario from './addWriterNewWkScenario.js';
import addWriterValidatorRewardScenario from './addWriterValidatorRewardScenario.js';
import addWriterZeroWriterKeyScenario from './addWriterZeroWriterKeyScenario.js';
import addWriterInvalidValidatorSignatureScenario from './addWriterInvalidValidatorSignatureScenario.js';
import addWriterRequesterEntryMissingScenario from './addWriterRequesterEntryMissingScenario.js';
import addWriterRequesterEntryDecodeFailureScenario from './addWriterRequesterEntryDecodeFailureScenario.js';
import addWriterRequesterNotWhitelistedScenario from './addWriterRequesterNotWhitelistedScenario.js';
import addWriterRequesterAlreadyWriterScenario from './addWriterRequesterAlreadyWriterScenario.js';
import addWriterRequesterIndexerScenario from './addWriterRequesterIndexerScenario.js';
import addWriterWriterKeyOwnershipScenario from './addWriterWriterKeyOwnershipScenario.js';
import addWriterWriterKeyMismatchScenario from './addWriterWriterKeyMismatchScenario.js';
import addWriterStakeInvalidEntryScenario from './addWriterStakeInvalidEntryScenario.js';
import addWriterStakeInvalidBalanceScenario from './addWriterStakeInvalidBalanceScenario.js';
import addWriterStakeInsufficientBalanceScenario from './addWriterStakeInsufficientBalanceScenario.js';
import addWriterStakeSubtractFailureScenario from './addWriterStakeSubtractFailureScenario.js';
import addWriterStakeBalanceUpdateFailureScenario from './addWriterStakeBalanceUpdateFailureScenario.js';
import addWriterStakeStakedBalanceFailureScenario from './addWriterStakeStakedBalanceFailureScenario.js';
import {
	setupAddWriterScenario,
	buildAddWriterPayload,
	buildAddWriterPayloadWithTxValidity,
	assertAddWriterFailureState,
	assertAddWriterSuccessState,
	applyWithRoleAccessBypass,
	applyWithMissingComponentBypass,
	selectValidatorPeerWithoutEntry,
	selectWriterPeer
} from './addWriterScenarioHelpers.js';
import PartialOperationValidationScenario, { PartialOperationMutationStrategy } from '../common/payload-structure/partialOperationValidationScenario.js';
import RequesterAddressValidationScenario from '../common/requesterAddressValidationScenario.js';
import createRequesterPublicKeyValidationScenario from '../common/requesterPublicKeyValidationScenario.js';
import InvalidAddressValidationScenario from '../common/payload-structure/invalidAddressValidationScenario.js';
import createAddressWithInvalidPublicKeyScenario from '../common/payload-structure/addressWithInvalidPublicKeyScenario.js';
import InvalidHashValidationScenario from '../common/payload-structure/invalidHashValidationScenario.js';
import InvalidSignatureValidationScenario, { SignatureMutationStrategy } from '../common/payload-structure/invalidSignatureValidationScenario.js';
import TransactionValidityMismatchScenario from '../common/transactionValidityMismatchScenario.js';
import IndexerSequenceStateInvalidScenario from '../common/indexer/indexerSequenceStateInvalidScenario.js';
import OperationAlreadyAppliedScenario from '../common/operationAlreadyAppliedScenario.js';
import ValidatorEntryMissingScenario from '../common/validatorConsistency/validatorEntryMissingScenario.js';
import ValidatorEntryDecodeFailureScenario from '../common/validatorConsistency/validatorEntryDecodeFailureScenario.js';
import ValidatorEntryInvalidBalanceScenario from '../common/validatorEntryValidation/validatorEntryInvalidBalanceScenario.js';
import ValidatorEntryRewardFailureScenario from '../common/balances/validatorEntryRewardFailureScenario.js';
import ValidatorEntryUpdateFailureScenario from '../common/balances/validatorEntryUpdateFailureScenario.js';
import ValidatorInactiveScenario from '../common/validatorConsistency/validatorInactiveScenario.js';
import ValidatorWriterKeyMismatchScenario from '../common/validatorConsistency/validatorWriterKeyMismatchScenario.js';
import RequesterBalanceDecodeFailureScenario from '../common/balances/requesterBalanceDecodeFailureScenario.js';
import RequesterBalanceFeeApplicationFailureScenario from '../common/balances/requesterBalanceFeeApplicationFailureScenario.js';
import RequesterBalanceUpdateFailureScenario from '../common/balances/requesterBalanceUpdateFailureScenario.js';
import addWriterRequesterBalanceInsufficientScenario from './addWriterRequesterBalanceInsufficientScenario.js';

addWriterHappyPathScenario();
addWriterNewWkScenario();
addWriterValidatorRewardScenario();

new PartialOperationValidationScenario({
	title: 'State.apply addWriter rejects incomplete validator co-signatures',
	setupScenario: setupAddWriterScenario,
	buildValidPayload: buildAddWriterPayload,
	assertStateUnchanged: assertAddWriterFailureState,
	strategy: PartialOperationMutationStrategy.MISSING_COMPONENT,
	parentKey: 'rao',
	applyInvalidPayload: applyWithMissingComponentBypass,
	expectedLogs: ['Operation is not complete.']
}).performScenario();


new PartialOperationValidationScenario({
	title: 'State.apply addWriter rejects payloads when nonces match',
	setupScenario: setupAddWriterScenario,
	buildValidPayload: buildAddWriterPayload,
	assertStateUnchanged: assertAddWriterFailureState,
	strategy: PartialOperationMutationStrategy.NONCE_MATCH,
	parentKey: 'rao',
	expectedLogs: ['Nonces should not be the same.']
}).performScenario();


new PartialOperationValidationScenario({
	title: 'State.apply addWriter rejects payloads when verifier shares requester address',
	setupScenario: setupAddWriterScenario,
	buildValidPayload: buildAddWriterPayload,
	assertStateUnchanged: assertAddWriterFailureState,
	strategy: PartialOperationMutationStrategy.ADDRESS_MATCH,
	parentKey: 'rao',
	expectedLogs: ['Addresses should be different.']
}).performScenario();


new PartialOperationValidationScenario({
	title: 'State.apply addWriter rejects payloads when validator signature duplicates requester signature',
	setupScenario: setupAddWriterScenario,
	buildValidPayload: buildAddWriterPayload,
	assertStateUnchanged: assertAddWriterFailureState,
	strategy: PartialOperationMutationStrategy.SIGNATURE_MATCH,
	parentKey: 'rao',
	expectedLogs: ['Signatures should be different.']
}).performScenario();

new RequesterAddressValidationScenario({
	title: 'State.apply addWriter requester address is invalid',
	setupScenario: setupAddWriterScenario,
	buildValidPayload: buildAddWriterPayload,
	assertStateUnchanged: assertAddWriterFailureState,
	expectedLogs: ['Requester address is invalid.']
}).performScenario();

createRequesterPublicKeyValidationScenario({
	title: 'State.apply addWriter requester public key is invalid',
	setupScenario: setupAddWriterScenario,
	buildValidPayload: buildAddWriterPayload,
	assertStateUnchanged: assertAddWriterFailureState,
	expectedLogs: ['Error while decoding requester public key.']
}).performScenario();

addWriterZeroWriterKeyScenario();

new InvalidHashValidationScenario({
	title: 'State.apply addWriter requester message hash mismatch',
	setupScenario: setupAddWriterScenario,
	buildValidPayload: buildAddWriterPayload,
	assertStateUnchanged: assertAddWriterFailureState,
	expectedLogs: ['Message hash does not match the tx_hash.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply addWriter requester signature is invalid (foreign signature)',
	setupScenario: setupAddWriterScenario,
	buildValidPayload: buildAddWriterPayload,
	assertStateUnchanged: assertAddWriterFailureState,
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply addWriter requester signature is invalid (zero fill)',
	setupScenario: setupAddWriterScenario,
	buildValidPayload: buildAddWriterPayload,
	assertStateUnchanged: assertAddWriterFailureState,
	strategy: SignatureMutationStrategy.ZERO_FILL,
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply addWriter requester signature is invalid (type mismatch)',
	setupScenario: setupAddWriterScenario,
	buildValidPayload: buildAddWriterPayload,
	assertStateUnchanged: assertAddWriterFailureState,
	strategy: SignatureMutationStrategy.TYPE_MISMATCH,
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

// validator address validation scenario
new InvalidAddressValidationScenario({
	title: 'State.apply addWriter validator address is invalid',
	setupScenario: setupAddWriterScenario,
	buildValidPayload: buildAddWriterPayload,
	assertStateUnchanged: assertAddWriterFailureState,
	addressPath: ['rao', 'va'],
	expectedLogs: ['Failed to validate validator address.']
}).performScenario();

// validator public key validation scenario
createAddressWithInvalidPublicKeyScenario({
	title: 'State.apply addWriter validator public key is invalid',
	setupScenario: setupAddWriterScenario,
	buildValidPayload: buildAddWriterPayload,
	assertStateUnchanged: assertAddWriterFailureState,
	addressPath: ['rao', 'va'],
	expectedLogs: ['Failed to decode validator public key.']
}).performScenario();

addWriterInvalidValidatorSignatureScenario();

new IndexerSequenceStateInvalidScenario({
	title: 'State.apply addWriter rejects payloads when indexer sequence state is invalid',
	setupScenario: setupAddWriterScenario,
	buildValidPayload: buildAddWriterPayload,
	assertStateUnchanged: (t, context) =>
		assertAddWriterFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Indexer sequence state is invalid.']
}).performScenario();

new TransactionValidityMismatchScenario({
	title: 'State.apply addWriter rejects payloads when tx validity mismatches indexer state',
	setupScenario: setupAddWriterScenario,
	buildValidPayload: buildAddWriterPayload,
	assertStateUnchanged: assertAddWriterFailureState,
	txValidityPath: ['rao', 'txv'],
	rebuildPayloadWithTxValidity: ({ context, mutatedTxValidity }) =>
		buildAddWriterPayloadWithTxValidity(context, mutatedTxValidity),
	expectedLogs: ['Transaction was not executed.']
}).performScenario();

new ValidatorEntryMissingScenario({
	title: 'State.apply addWriter rejects payloads when validator entry is missing',
	setupScenario: t => setupAddWriterScenario(t, { nodes: 3 }),
	buildValidPayload: context => {
		const validatorPeer = selectValidatorPeerWithoutEntry(context);
		if (!validatorPeer) {
			throw new Error('Validator entry missing scenario requires an extra peer.');
		}
		return buildAddWriterPayload(context, { validatorPeer });
	},
	assertStateUnchanged: assertAddWriterFailureState,
	expectedLogs: ['Incoming validator entry is null.']
}).performScenario();

new ValidatorEntryDecodeFailureScenario({
	title: 'State.apply addWriter rejects payloads when validator entry cannot be decoded',
	setupScenario: setupAddWriterScenario,
	buildValidPayload: buildAddWriterPayload,
	assertStateUnchanged: (t, context) => assertAddWriterFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Failed to decode validator entry.']
}).performScenario();

new ValidatorInactiveScenario({
	title: 'State.apply addWriter rejects payloads when validator is not an active writer',
	setupScenario: setupAddWriterScenario,
	buildValidPayload: buildAddWriterPayload,
	assertStateUnchanged: (t, context) => assertAddWriterFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Operation validator is not active']
}).performScenario();

new ValidatorWriterKeyMismatchScenario({
	title: 'State.apply addWriter rejects payloads when validator writer key mismatches requester',
	setupScenario: setupAddWriterScenario,
	buildValidPayload: buildAddWriterPayload,
	assertStateUnchanged: (t, context) => assertAddWriterFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Validator cannot be the same as requester.']
}).performScenario();

new OperationAlreadyAppliedScenario({
	title: 'State.apply addWriter rejects duplicate operations',
	setupScenario: setupAddWriterScenario,
	buildValidPayload: buildAddWriterPayload,
	assertStateUnchanged: (t, context, validPayload) =>
		assertAddWriterSuccessState(t, context, {
			payload: validPayload
		}),
	expectedLogs: ['Operation has already been applied.']
}).performScenario();

addWriterRequesterEntryMissingScenario();
addWriterRequesterEntryDecodeFailureScenario();

addWriterWriterKeyOwnershipScenario();
addWriterWriterKeyMismatchScenario();


addWriterRequesterNotWhitelistedScenario();
addWriterRequesterAlreadyWriterScenario();
addWriterRequesterIndexerScenario();

new RequesterBalanceDecodeFailureScenario({
	title: 'State.apply addWriter rejects payloads when requester balance cannot be verified',
	setupScenario: setupAddWriterScenario,
	buildValidPayload: buildAddWriterPayload,
	assertStateUnchanged: (t, context) => assertAddWriterFailureState(t, context, { skipSync: true }),
	selectPeer: selectWriterPeer
}).performScenario();

addWriterRequesterBalanceInsufficientScenario();

new RequesterBalanceFeeApplicationFailureScenario({
	title: 'State.apply addWriter rejects payloads when requester fee cannot be applied',
	setupScenario: setupAddWriterScenario,
	buildValidPayload: buildAddWriterPayload,
	assertStateUnchanged: (t, context) => assertAddWriterFailureState(t, context, { skipSync: true }),
	selectPeer: selectWriterPeer
}).performScenario();

new RequesterBalanceUpdateFailureScenario({
	title: 'State.apply addWriter rejects payloads when requester balance cannot be written',
	setupScenario: setupAddWriterScenario,
	buildValidPayload: buildAddWriterPayload,
	assertStateUnchanged: (t, context) => assertAddWriterFailureState(t, context, { skipSync: true }),
	selectPeer: selectWriterPeer
}).performScenario();

new ValidatorEntryInvalidBalanceScenario({
	title: 'State.apply addWriter rejects payloads when validator balance is invalid',
	setupScenario: setupAddWriterScenario,
	buildValidPayload: buildAddWriterPayload,
	assertStateUnchanged: (t, context) => assertAddWriterFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Invalid validator balance.']
}).performScenario();

new ValidatorEntryRewardFailureScenario({
	title: 'State.apply addWriter rejects payloads when validator reward transfer fails',
	setupScenario: setupAddWriterScenario,
	buildValidPayload: buildAddWriterPayload,
	assertStateUnchanged: (t, context) => assertAddWriterFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Failed to transfer fee to validator.']
}).performScenario();

new ValidatorEntryUpdateFailureScenario({
	title: 'State.apply addWriter rejects payloads when validator entry update fails',
	setupScenario: setupAddWriterScenario,
	buildValidPayload: buildAddWriterPayload,
	assertStateUnchanged: (t, context) => assertAddWriterFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Failed to update validator entry.']
}).performScenario();

addWriterStakeInvalidEntryScenario();
addWriterStakeInvalidBalanceScenario();
addWriterStakeInsufficientBalanceScenario();
addWriterStakeSubtractFailureScenario();
addWriterStakeBalanceUpdateFailureScenario();
addWriterStakeStakedBalanceFailureScenario();

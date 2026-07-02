import b4a from 'b4a';
import txOperationStandardHappyPathScenario from './txOperationStandardHappyPathScenario.js';
import txOperationValidatorCreatorHappyPathScenario from './txOperationValidatorCreatorHappyPathScenario.js';
import txOperationRequesterCreatorHappyPathScenario from './txOperationRequesterCreatorHappyPathScenario.js';
import txOperationDifferentValidatorCreatorHappyPathScenario from './txOperationDifferentValidatorCreatorHappyPathScenario.js';
import InvalidPayloadValidationScenario from '../common/payload-structure/invalidPayloadValidationScenario.js';
import InvalidHashValidationScenario from '../common/payload-structure/invalidHashValidationScenario.js';
import PartialOperationValidationScenario, {
	PartialOperationMutationStrategy
} from '../common/payload-structure/partialOperationValidationScenario.js';
import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import InvalidSignatureValidationScenario, {
	SignatureMutationStrategy
} from '../common/payload-structure/invalidSignatureValidationScenario.js';
import RequesterAddressValidationScenario from '../common/requesterAddressValidationScenario.js';
import createRequesterPublicKeyValidationScenario from '../common/requesterPublicKeyValidationScenario.js';
import InvalidAddressValidationScenario from '../common/payload-structure/invalidAddressValidationScenario.js';
import createAddressWithInvalidPublicKeyScenario from '../common/payload-structure/addressWithInvalidPublicKeyScenario.js';
import IndexerSequenceStateInvalidScenario from '../common/indexer/indexerSequenceStateInvalidScenario.js';
import TransactionValidityMismatchScenario from '../common/transactionValidityMismatchScenario.js';
import OperationAlreadyAppliedScenario from '../common/operationAlreadyAppliedScenario.js';
import ValidatorConsistencyScenarioBase, {
	ValidatorEntryMutation
} from '../common/validatorConsistency/base/validatorConsistencyScenarioBase.js';
import ValidatorEntryDecodeFailureScenario from '../common/validatorConsistency/validatorEntryDecodeFailureScenario.js';
import ValidatorInactiveScenario from '../common/validatorConsistency/validatorInactiveScenario.js';
import ValidatorWriterKeyMismatchScenario from '../common/validatorConsistency/validatorWriterKeyMismatchScenario.js';
import { safeDecodeApplyOperation, safeEncodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import {
	setupTxOperationScenario,
	buildTxOperationPayload,
	buildTxOperationPayloadWithTxValidity,
	assertTxOperationFailureState,
	assertTxOperationSuccessState,
	mutateBootstrapEqualMbs,
	mutateMbsMismatch,
	appendInvalidTxPayload,
	mutateValidatorSignature
} from './txOperationScenarioHelpers.js';
import txOperationBootstrapNotRegisteredScenario from './txOperationBootstrapNotRegisteredScenario.js';
import txOperationInvalidDeploymentEntryScenario from './txOperationInvalidDeploymentEntryScenario.js';
import txOperationInvalidSubnetCreatorAddressScenario from './txOperationInvalidSubnetCreatorAddressScenario.js';
import txOperationInvalidFeeAmountScenario from './txOperationInvalidFeeAmountScenario.js';
import txOperationTransferFeeInvalidRequesterEntryScenario from './txOperationTransferFeeInvalidRequesterEntryScenario.js';
import txOperationTransferFeeDecodeRequesterEntryScenario from './txOperationTransferFeeDecodeRequesterEntryScenario.js';
import txOperationTransferFeeInvalidRequesterBalanceScenario from './txOperationTransferFeeInvalidRequesterBalanceScenario.js';
import txOperationTransferFeeInsufficientRequesterBalanceScenario from './txOperationTransferFeeInsufficientRequesterBalanceScenario.js';
import txOperationTransferFeeSubtractFailureScenario from './txOperationTransferFeeSubtractFailureScenario.js';
import txOperationTransferFeeUpdateFailureScenario from './txOperationTransferFeeUpdateFailureScenario.js';
import txOperationTransferFeeDecodeValidatorEntryScenario from './txOperationTransferFeeDecodeValidatorEntryScenario.js';
import txOperationTransferFeeInvalidValidatorBalanceScenario from './txOperationTransferFeeInvalidValidatorBalanceScenario.js';
import txOperationTransferFeeAddValidatorBalanceFailureScenario from './txOperationTransferFeeAddValidatorBalanceFailureScenario.js';
import txOperationTransferFeeUpdateValidatorBalanceFailureScenario from './txOperationTransferFeeUpdateValidatorBalanceFailureScenario.js';
import txOperationTransferFeeAddValidatorBonusFailureScenario from './txOperationTransferFeeAddValidatorBonusFailureScenario.js';
import txOperationTransferFeeUpdateValidatorBonusFailureScenario from './txOperationTransferFeeUpdateValidatorBonusFailureScenario.js';
import txOperationTransferFeeMissingCreatorEntryScenario from './txOperationTransferFeeMissingCreatorEntryScenario.js';
import txOperationTransferFeeDecodeCreatorEntryScenario from './txOperationTransferFeeDecodeCreatorEntryScenario.js';
import txOperationTransferFeeInvalidCreatorBalanceScenario from './txOperationTransferFeeInvalidCreatorBalanceScenario.js';
import txOperationTransferFeeAddCreatorBalanceFailureScenario from './txOperationTransferFeeAddCreatorBalanceFailureScenario.js';
import txOperationTransferFeeUpdateCreatorBalanceFailureScenario from './txOperationTransferFeeUpdateCreatorBalanceFailureScenario.js';
import {
	txOperationTransferFeeResultNullScenario,
	txOperationTransferFeeResultIgnoredScenario,
	txOperationTransferFeeRequesterEntryMissingScenario,
	txOperationTransferFeeValidatorEntryMissingScenario
} from './txOperationTransferFeeGuardBypassScenario.js';

txOperationStandardHappyPathScenario();
txOperationValidatorCreatorHappyPathScenario();
txOperationRequesterCreatorHappyPathScenario();
txOperationDifferentValidatorCreatorHappyPathScenario();

new InvalidPayloadValidationScenario({
	title: 'State.apply txOperation rejects payloads that fail contract validation',
	setupScenario: setupTxOperationScenario,
	buildValidPayload: buildTxOperationPayload,
	mutatePayload: (t, validPayload) => {
		const decoded = safeDecodeApplyOperation(validPayload);
		t.ok(decoded, 'valid payload decodes before mutation');
		if (!decoded) return validPayload;
		return safeEncodeApplyOperation({ ...decoded, address: b4a.alloc(1) });
	},
	applyInvalidPayload: appendInvalidTxPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertTxOperationFailureState(t, context, { payload: invalidPayload }),
	expectedLogs: ['Contract schema validation failed.']
}).performScenario();

// validator cannot sign own tx
new PartialOperationValidationScenario({
	title: 'State.apply txOperation rejects payloads when validator signs own transaction',
	setupScenario: setupTxOperationScenario,
	buildValidPayload: buildTxOperationPayload,
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTxOperationFailureState(t, context, { payload: invalid }),
	strategy: PartialOperationMutationStrategy.ADDRESS_MATCH,
	parentKey: 'txo',
	expectedLogs: ['Validator cannot sign its own transaction.']
}).performScenario();

// nonces and signatures equality checks
new PartialOperationValidationScenario({
	title: 'State.apply txOperation rejects payloads when nonces match',
	setupScenario: setupTxOperationScenario,
	buildValidPayload: buildTxOperationPayload,
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTxOperationFailureState(t, context, { payload: invalid }),
	strategy: PartialOperationMutationStrategy.NONCE_MATCH,
	parentKey: 'txo',
	expectedLogs: ['Nonces should not be the same.']
}).performScenario();

new PartialOperationValidationScenario({
	title: 'State.apply txOperation rejects payloads when signatures match',
	setupScenario: setupTxOperationScenario,
	buildValidPayload: buildTxOperationPayload,
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTxOperationFailureState(t, context, { payload: invalid }),
	strategy: PartialOperationMutationStrategy.SIGNATURE_MATCH,
	parentKey: 'txo',
	expectedLogs: ['Signatures should not be the same.']
}).performScenario();

// bootstrap consistency
new OperationValidationScenarioBase({
	title: 'State.apply txOperation rejects payloads when external bootstrap matches MSB bootstrap',
	setupScenario: setupTxOperationScenario,
	buildValidPayload: buildTxOperationPayload,
	mutatePayload: (t, validPayload) => mutateBootstrapEqualMbs(t, validPayload),
	applyInvalidPayload: appendInvalidTxPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertTxOperationFailureState(t, context, { payload: invalidPayload }),
	expectedLogs: ['Network and external bootstrap cannot be the same.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply txOperation rejects payloads when declared MSB bootstrap mismatches network',
	setupScenario: setupTxOperationScenario,
	buildValidPayload: buildTxOperationPayload,
	mutatePayload: (t, validPayload) => mutateMbsMismatch(t, validPayload),
	applyInvalidPayload: appendInvalidTxPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertTxOperationFailureState(t, context, { payload: invalidPayload }),
	expectedLogs: ['Declared MSB bootstrap is different than real MSB bootstrap.']
}).performScenario();

// requester identity
new RequesterAddressValidationScenario({
	title: 'State.apply txOperation rejects invalid requester address',
	setupScenario: setupTxOperationScenario,
	buildValidPayload: buildTxOperationPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertTxOperationFailureState(t, context, { payload: invalidPayload }),
	expectedLogs: ['Invalid requester address.']
}).performScenario();

createRequesterPublicKeyValidationScenario({
	title: 'State.apply txOperation rejects requester public key decode failures',
	setupScenario: setupTxOperationScenario,
	buildValidPayload: buildTxOperationPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertTxOperationFailureState(t, context, { payload: invalidPayload }),
	expectedLogs: ['Failed to decode requester public key.']
}).performScenario();

new InvalidHashValidationScenario({
	title: 'State.apply txOperation rejects payloads when requester hash mismatches tx_hash',
	setupScenario: setupTxOperationScenario,
	buildValidPayload: buildTxOperationPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertTxOperationFailureState(t, context, { payload: invalidPayload }),
	applyInvalidPayload: appendInvalidTxPayload,
	expectedLogs: ['Message hash does not match the tx_hash.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply txOperation rejects payloads when requester signature verification fails',
	setupScenario: setupTxOperationScenario,
	buildValidPayload: buildTxOperationPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertTxOperationFailureState(t, context, { payload: invalidPayload }),
	applyInvalidPayload: appendInvalidTxPayload,
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

// validator identity
new InvalidAddressValidationScenario({
	title: 'State.apply txOperation rejects invalid validator address',
	setupScenario: setupTxOperationScenario,
	buildValidPayload: buildTxOperationPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertTxOperationFailureState(t, context, { payload: invalidPayload }),
	addressPath: ['txo', 'va'],
	expectedLogs: ['Invalid validator address.']
}).performScenario();

createAddressWithInvalidPublicKeyScenario({
	title: 'State.apply txOperation rejects validator public key decode failures',
	setupScenario: setupTxOperationScenario,
	buildValidPayload: buildTxOperationPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertTxOperationFailureState(t, context, { payload: invalidPayload }),
	addressPath: ['txo', 'va'],
	expectedLogs: ['Failed to decode validator public key.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply txOperation rejects validator message signature verification failures',
	setupScenario: setupTxOperationScenario,
	buildValidPayload: buildTxOperationPayload,
	mutatePayload: (t, validPayload) => mutateValidatorSignature(t, validPayload),
	applyInvalidPayload: appendInvalidTxPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertTxOperationFailureState(t, context, { payload: invalidPayload }),
	expectedLogs: ['Failed to verify validator message signature.']
}).performScenario();

// indexer sequence + tx validity
new IndexerSequenceStateInvalidScenario({
	title: 'State.apply txOperation rejects payloads when indexer sequence state is invalid',
	setupScenario: setupTxOperationScenario,
	buildValidPayload: buildTxOperationPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertTxOperationFailureState(t, context, { payload: invalidPayload }),
	expectedLogs: ['Indexer sequence state is invalid.']
}).performScenario();

new TransactionValidityMismatchScenario({
	title: 'State.apply txOperation rejects payloads when tx validity mismatches indexer state',
	setupScenario: setupTxOperationScenario,
	buildValidPayload: buildTxOperationPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertTxOperationFailureState(t, context, { payload: invalidPayload }),
	txValidityPath: ['txo', 'txv'],
	rebuildPayloadWithTxValidity: ({ context, mutatedTxValidity }) =>
		buildTxOperationPayloadWithTxValidity(context, mutatedTxValidity),
	expectedLogs: ['Transaction was not executed.']
}).performScenario();

new ValidatorConsistencyScenarioBase({
	title: 'State.apply txOperation rejects payloads when validator entry is missing',
	setupScenario: setupTxOperationScenario,
	buildValidPayload: buildTxOperationPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertTxOperationFailureState(t, context, { payload: invalidPayload }),
	mutateEntry: () => ValidatorEntryMutation.DELETE,
	validatorAddressPath: ['txo', 'va'],
	expectedLogs: ['Incoming validator entry is null.']
}).performScenario();

new ValidatorEntryDecodeFailureScenario({
	title: 'State.apply txOperation rejects payloads when validator entry cannot be decoded',
	setupScenario: setupTxOperationScenario,
	buildValidPayload: buildTxOperationPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertTxOperationFailureState(t, context, { payload: invalidPayload, skipSync: true }),
	validatorAddressPath: ['txo', 'va'],
	expectedLogs: ['Failed to decode validator entry.']
}).performScenario();

new ValidatorInactiveScenario({
	title: 'State.apply txOperation rejects payloads when validator is not an active writer',
	setupScenario: setupTxOperationScenario,
	buildValidPayload: buildTxOperationPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertTxOperationFailureState(t, context, { payload: invalidPayload, skipSync: true }),
	validatorAddressPath: ['txo', 'va'],
	expectedLogs: ['Operation validator is not active']
}).performScenario();

new ValidatorWriterKeyMismatchScenario({
	title: 'State.apply txOperation rejects payloads when validator writer key mismatches requester',
	setupScenario: setupTxOperationScenario,
	buildValidPayload: buildTxOperationPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertTxOperationFailureState(t, context, { payload: invalidPayload, skipSync: true }),
	validatorAddressPath: ['txo', 'va'],
	expectedLogs: ['Validator cannot be the same as requester.']
}).performScenario();

new OperationAlreadyAppliedScenario({
	title: 'State.apply txOperation rejects duplicate operations',
	setupScenario: setupTxOperationScenario,
	buildValidPayload: buildTxOperationPayload,
	selectNode: context => context.txOperation?.validatorPeer ?? context.peers?.[1],
	assertStateUnchanged: (t, context, validPayload) =>
		assertTxOperationSuccessState(t, context, { payload: validPayload, skipSync: true }),
	expectedLogs: ['Operation has already been applied.']
}).performScenario();

txOperationBootstrapNotRegisteredScenario();
txOperationInvalidDeploymentEntryScenario();
txOperationInvalidSubnetCreatorAddressScenario();
txOperationInvalidFeeAmountScenario();
// transferFeeTxOperation
txOperationTransferFeeInvalidRequesterEntryScenario();
txOperationTransferFeeDecodeRequesterEntryScenario();
txOperationTransferFeeInvalidRequesterBalanceScenario();
txOperationTransferFeeInsufficientRequesterBalanceScenario();
txOperationTransferFeeSubtractFailureScenario();
txOperationTransferFeeUpdateFailureScenario();
txOperationTransferFeeDecodeValidatorEntryScenario();
txOperationTransferFeeInvalidValidatorBalanceScenario();
txOperationTransferFeeAddValidatorBalanceFailureScenario();
txOperationTransferFeeUpdateValidatorBalanceFailureScenario();
txOperationTransferFeeAddValidatorBonusFailureScenario();
txOperationTransferFeeUpdateValidatorBonusFailureScenario();
txOperationTransferFeeMissingCreatorEntryScenario();
txOperationTransferFeeDecodeCreatorEntryScenario();
txOperationTransferFeeInvalidCreatorBalanceScenario();
txOperationTransferFeeAddCreatorBalanceFailureScenario();
txOperationTransferFeeUpdateCreatorBalanceFailureScenario();
txOperationTransferFeeResultNullScenario().performScenario();
txOperationTransferFeeResultIgnoredScenario().performScenario();
txOperationTransferFeeRequesterEntryMissingScenario().performScenario();
txOperationTransferFeeValidatorEntryMissingScenario().performScenario();
// Post-transferFee guards for null requester/validator entries are unreachable with current transferFeeTxOperation
// (it returns null/IGNORE on earlier failures), so no dedicated scenarios exist yet.

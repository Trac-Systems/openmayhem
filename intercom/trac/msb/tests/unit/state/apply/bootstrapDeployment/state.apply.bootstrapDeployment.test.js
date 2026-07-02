import bootstrapDeploymentHappyPathScenario from './bootstrapDeploymentHappyPathScenario.js';
import bootstrapDeploymentMultipleBootstrapScenario from './bootstrapDeploymentMultipleBootstrapScenario.js';
import bootstrapDeploymentIncompleteOperationScenario from './bootstrapDeploymentIncompleteOperationScenario.js';
import b4a from 'b4a';
import PartialOperationValidationScenario, {
	PartialOperationMutationStrategy
} from '../common/payload-structure/partialOperationValidationScenario.js';
import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import RequesterAddressValidationScenario from '../common/requesterAddressValidationScenario.js';
import createRequesterPublicKeyValidationScenario from '../common/requesterPublicKeyValidationScenario.js';
import InvalidHashValidationScenario from '../common/payload-structure/invalidHashValidationScenario.js';
import InvalidPayloadValidationScenario from '../common/payload-structure/invalidPayloadValidationScenario.js';
import InvalidAddressValidationScenario from '../common/payload-structure/invalidAddressValidationScenario.js';
import createAddressWithInvalidPublicKeyScenario from '../common/payload-structure/addressWithInvalidPublicKeyScenario.js';
import TransactionValidityMismatchScenario from '../common/transactionValidityMismatchScenario.js';
import IndexerSequenceStateInvalidScenario from '../common/indexer/indexerSequenceStateInvalidScenario.js';
import InvalidSignatureValidationScenario, {
	SignatureMutationStrategy
} from '../common/payload-structure/invalidSignatureValidationScenario.js';
import ValidatorEntryDecodeFailureScenario from '../common/validatorConsistency/validatorEntryDecodeFailureScenario.js';
import ValidatorInactiveScenario from '../common/validatorConsistency/validatorInactiveScenario.js';
import ValidatorWriterKeyMismatchScenario from '../common/validatorConsistency/validatorWriterKeyMismatchScenario.js';
import ValidatorConsistencyScenarioBase, {
	ValidatorEntryMutation
} from '../common/validatorConsistency/base/validatorConsistencyScenarioBase.js';
import OperationAlreadyAppliedScenario from '../common/operationAlreadyAppliedScenario.js';
import bootstrapDeploymentDuplicateRegistrationScenario from './bootstrapDeploymentDuplicateRegistrationScenario.js';
import bootstrapDeploymentInvalidDeploymentEntryScenario from './bootstrapDeploymentInvalidDeploymentEntryScenario.js';
import FeeDecodeFailureScenario from '../common/balances/feeDecodeFailureScenario.js';
import RequesterBalanceInsufficientScenario from '../common/balances/requesterBalanceInsufficientScenario.js';
import RequesterBalanceUpdateFailureScenario from '../common/balances/requesterBalanceUpdateFailureScenario.js';
import RequesterNodeEntryBufferMissingScenario from '../common/requester/requesterNodeEntryBufferMissingScenario.js';
import RequesterNodeEntryDecodeFailureScenario from '../common/requester/requesterNodeEntryDecodeFailureScenario.js';
import ValidatorEntryInvalidBalanceScenario from '../common/validatorEntryValidation/validatorEntryInvalidBalanceScenario.js';
import ValidatorEntryRewardFailureScenario from '../common/balances/validatorEntryRewardFailureScenario.js';
import ValidatorEntryUpdateFailureScenario from '../common/balances/validatorEntryUpdateFailureScenario.js';
import RequesterBalanceDecodeFailureScenario from '../common/balances/requesterBalanceDecodeFailureScenario.js';
import RequesterBalanceFeeApplicationFailureScenario from '../common/balances/requesterBalanceFeeApplicationFailureScenario.js';
import ValidatorNodeEntryDecodeFailureScenario from '../common/balances/validatorNodeEntryDecodeFailureScenario.js';
import {
	setupBootstrapDeploymentScenario,
	buildBootstrapDeploymentPayload,
	buildBootstrapDeploymentPayloadWithTxValidity,
	assertBootstrapDeploymentFailureState,
	assertBootstrapDeploymentSuccessState,
	mutateToNetworkBootstrap,
	appendInvalidPayload
} from './bootstrapDeploymentScenarioHelpers.js';
import bootstrapDeploymentInvalidValidatorNodeEntryScenario from './invalidValidatorNodeEntryScenario.js';
import { safeDecodeApplyOperation, safeEncodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';

bootstrapDeploymentHappyPathScenario();
bootstrapDeploymentMultipleBootstrapScenario();

new InvalidPayloadValidationScenario({
	title: 'State.apply bootstrapDeployment rejects payloads that fail contract validation',
	setupScenario: setupBootstrapDeploymentScenario,
	buildValidPayload: buildBootstrapDeploymentPayload,
	mutatePayload: (t, validPayload) => {
		const decoded = safeDecodeApplyOperation(validPayload);
		t.ok(decoded, 'valid payload decodes before mutation');
		if (!decoded) return validPayload;
		const invalid = b4a.alloc(1);
		return safeEncodeApplyOperation({ ...decoded, address: invalid });
	},
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertBootstrapDeploymentFailureState(t, context, { payload: invalidPayload }),
	expectedLogs: ['Contract schema validation failed.']
}).performScenario();

bootstrapDeploymentIncompleteOperationScenario();

new OperationValidationScenarioBase({
	title: 'State.apply bootstrapDeployment rejects attempts to deploy existing MSB bootstrap',
	setupScenario: setupBootstrapDeploymentScenario,
	buildValidPayload: buildBootstrapDeploymentPayload,
	mutatePayload: (t, payload, context) => mutateToNetworkBootstrap(t, payload, context),
	applyInvalidPayload: appendInvalidPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertBootstrapDeploymentFailureState(t, context, {
			payload: invalidPayload,
			bootstrapBufferOverride: context.bootstrap?.base?.local?.key ?? null
		}),
	expectedLogs: ['Cannot deploy bootstrap on existing same bootstrap.']
}).performScenario();

new PartialOperationValidationScenario({
	title: 'State.apply bootstrapDeployment rejects matching requester and validator nonces',
	setupScenario: setupBootstrapDeploymentScenario,
	buildValidPayload: buildBootstrapDeploymentPayload,
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertBootstrapDeploymentFailureState(t, context, { payload: invalid }),
	strategy: PartialOperationMutationStrategy.NONCE_MATCH,
	parentKey: 'bdo',
	expectedLogs: ['Nonces should not be the same.']
}).performScenario();

new PartialOperationValidationScenario({
	title: 'State.apply bootstrapDeployment rejects matching requester and validator addresses',
	setupScenario: setupBootstrapDeploymentScenario,
	buildValidPayload: buildBootstrapDeploymentPayload,
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertBootstrapDeploymentFailureState(t, context, { payload: invalid }),
	strategy: PartialOperationMutationStrategy.ADDRESS_MATCH,
	parentKey: 'bdo',
	expectedLogs: ['Addresses should be different.']
}).performScenario();

new PartialOperationValidationScenario({
	title: 'State.apply bootstrapDeployment rejects matching requester and validator signatures',
	setupScenario: setupBootstrapDeploymentScenario,
	buildValidPayload: buildBootstrapDeploymentPayload,
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertBootstrapDeploymentFailureState(t, context, { payload: invalid }),
	strategy: PartialOperationMutationStrategy.SIGNATURE_MATCH,
	parentKey: 'bdo',
	expectedLogs: ['Signatures should be different.']
}).performScenario();

new RequesterAddressValidationScenario({
	title: 'State.apply bootstrapDeployment rejects invalid requester address',
	setupScenario: setupBootstrapDeploymentScenario,
	buildValidPayload: buildBootstrapDeploymentPayload,
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertBootstrapDeploymentFailureState(t, context, { payload: invalid }),
	expectedLogs: ['Requester address is invalid.']
}).performScenario();

createRequesterPublicKeyValidationScenario({
	title: 'State.apply bootstrapDeployment rejects requester public key decode failures',
	setupScenario: setupBootstrapDeploymentScenario,
	buildValidPayload: buildBootstrapDeploymentPayload,
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertBootstrapDeploymentFailureState(t, context, { payload: invalid }),
	expectedLogs: ['Failed to decode requester public key.']
}).performScenario();

new InvalidHashValidationScenario({
	title: 'State.apply bootstrapDeployment rejects payloads when tx hash does not match message',
	setupScenario: setupBootstrapDeploymentScenario,
	buildValidPayload: buildBootstrapDeploymentPayload,
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertBootstrapDeploymentFailureState(t, context, { payload: invalid }),
	expectedLogs: ['Message hash does not match the tx_hash.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply bootstrapDeployment requester signature is invalid (foreign signature)',
	setupScenario: setupBootstrapDeploymentScenario,
	buildValidPayload: buildBootstrapDeploymentPayload,
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertBootstrapDeploymentFailureState(t, context, { payload: invalid }),
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply bootstrapDeployment requester signature is invalid (zero fill)',
	setupScenario: setupBootstrapDeploymentScenario,
	buildValidPayload: buildBootstrapDeploymentPayload,
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertBootstrapDeploymentFailureState(t, context, { payload: invalid }),
	strategy: SignatureMutationStrategy.ZERO_FILL,
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply bootstrapDeployment requester signature is invalid (type mismatch)',
	setupScenario: setupBootstrapDeploymentScenario,
	buildValidPayload: buildBootstrapDeploymentPayload,
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertBootstrapDeploymentFailureState(t, context, { payload: invalid }),
	strategy: SignatureMutationStrategy.TYPE_MISMATCH,
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new InvalidAddressValidationScenario({
	title: 'State.apply bootstrapDeployment validator address is invalid',
	setupScenario: setupBootstrapDeploymentScenario,
	buildValidPayload: buildBootstrapDeploymentPayload,
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertBootstrapDeploymentFailureState(t, context, { payload: invalid }),
	addressPath: ['bdo', 'va'],
	expectedLogs: ['Invalid validator address.']
}).performScenario();

createAddressWithInvalidPublicKeyScenario({
	title: 'State.apply bootstrapDeployment validator public key is invalid',
	setupScenario: setupBootstrapDeploymentScenario,
	buildValidPayload: buildBootstrapDeploymentPayload,
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertBootstrapDeploymentFailureState(t, context, { payload: invalid }),
	addressPath: ['bdo', 'va'],
	expectedLogs: ['Failed to decode validator public key.']
}).performScenario();

function mutateValidatorSignature(t, validPayload) {
	const decoded = safeDecodeApplyOperation(validPayload);
	t.ok(decoded, 'fixtures decode');
	const parent = decoded?.bdo;
	if (!parent?.vs) return validPayload;
	const mutated = b4a.from(parent.vs);
	mutated[mutated.length - 1] ^= 0xff;
	parent.vs = mutated;
	return safeEncodeApplyOperation(decoded);
}

new OperationValidationScenarioBase({
	title: 'State.apply bootstrapDeployment validator signature is invalid',
	setupScenario: setupBootstrapDeploymentScenario,
	buildValidPayload: buildBootstrapDeploymentPayload,
	mutatePayload: mutateValidatorSignature,
	applyInvalidPayload: appendInvalidPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertBootstrapDeploymentFailureState(t, context, { payload: invalidPayload }),
	expectedLogs: ['Failed to verify validator message signature.']
}).performScenario();

//TODO: Add to another tests
new IndexerSequenceStateInvalidScenario({
	title: 'State.apply bootstrapDeployment rejects payloads when indexer sequence state is invalid',
	setupScenario: setupBootstrapDeploymentScenario,
	buildValidPayload: buildBootstrapDeploymentPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertBootstrapDeploymentFailureState(t, context, { payload: invalidPayload }),
	expectedLogs: ['Indexer sequence state is invalid.']
}).performScenario();

new TransactionValidityMismatchScenario({
	title: 'State.apply bootstrapDeployment rejects payloads when tx validity mismatches indexer state',
	setupScenario: setupBootstrapDeploymentScenario,
	buildValidPayload: buildBootstrapDeploymentPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertBootstrapDeploymentFailureState(t, context, { payload: invalidPayload }),
	txValidityPath: ['bdo', 'txv'],
	rebuildPayloadWithTxValidity: ({ context, mutatedTxValidity }) =>
		buildBootstrapDeploymentPayloadWithTxValidity(context, mutatedTxValidity),
	expectedLogs: ['Transaction was not executed.']
}).performScenario();

new ValidatorConsistencyScenarioBase({
	title: 'State.apply bootstrapDeployment rejects payloads when validator entry is missing',
	setupScenario: setupBootstrapDeploymentScenario,
	buildValidPayload: buildBootstrapDeploymentPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertBootstrapDeploymentFailureState(t, context, { payload: invalidPayload }),
	mutateEntry: () => ValidatorEntryMutation.DELETE,
	validatorAddressPath: ['bdo', 'va'],
	expectedLogs: ['Incoming validator entry is null.']
}).performScenario();

new ValidatorEntryDecodeFailureScenario({
	title: 'State.apply bootstrapDeployment rejects payloads when validator entry cannot be decoded',
	setupScenario: setupBootstrapDeploymentScenario,
	buildValidPayload: buildBootstrapDeploymentPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertBootstrapDeploymentFailureState(t, context, { payload: invalidPayload, skipSync: true }),
	validatorAddressPath: ['bdo', 'va'],
	expectedLogs: ['Failed to decode validator entry.']
}).performScenario();

new ValidatorInactiveScenario({
	title: 'State.apply bootstrapDeployment rejects payloads when validator is not an active writer',
	setupScenario: setupBootstrapDeploymentScenario,
	buildValidPayload: buildBootstrapDeploymentPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertBootstrapDeploymentFailureState(t, context, { payload: invalidPayload, skipSync: true }),
	validatorAddressPath: ['bdo', 'va'],
	expectedLogs: ['Operation validator is not active']
}).performScenario();

new ValidatorWriterKeyMismatchScenario({
	title: 'State.apply bootstrapDeployment rejects payloads when validator writer key mismatches requester',
	setupScenario: setupBootstrapDeploymentScenario,
	buildValidPayload: buildBootstrapDeploymentPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertBootstrapDeploymentFailureState(t, context, { payload: invalidPayload, skipSync: true }),
	validatorAddressPath: ['bdo', 'va'],
	expectedLogs: ['Validator cannot be the same as requester.']
}).performScenario();

new OperationAlreadyAppliedScenario({
	title: 'State.apply bootstrapDeployment rejects payloads that were already applied',
	setupScenario: setupBootstrapDeploymentScenario,
	buildValidPayload: buildBootstrapDeploymentPayload,
	assertStateUnchanged: (t, context, validPayload) =>
		assertBootstrapDeploymentSuccessState(t, context, { payload: validPayload }),
	selectNode: context => context.bootstrapDeployment?.validatorPeer ?? context.bootstrap ?? context.peers?.[0],
	expectedLogs: ['Operation has already been applied.']
}).performScenario();

bootstrapDeploymentDuplicateRegistrationScenario();
bootstrapDeploymentInvalidDeploymentEntryScenario();

new FeeDecodeFailureScenario({
	title: 'State.apply bootstrapDeployment rejects payloads when fee cannot be decoded',
	setupScenario: setupBootstrapDeploymentScenario,
	buildValidPayload: buildBootstrapDeploymentPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertBootstrapDeploymentFailureState(t, context, {
			payload: invalidPayload,
			skipValidatorEquality: true
		}),
	selectNode: context => context.bootstrapDeployment?.validatorPeer ?? context.bootstrap ?? context.peers?.[0]
}).performScenario();

new RequesterNodeEntryBufferMissingScenario({
	title: 'State.apply bootstrapDeployment rejects payloads when requester node entry buffer is missing',
	setupScenario: setupBootstrapDeploymentScenario,
	buildValidPayload: buildBootstrapDeploymentPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertBootstrapDeploymentFailureState(t, context, {
			payload: invalidPayload,
			skipSync: true,
			skipValidatorEquality: true
		}),
	selectNode: context => context.bootstrapDeployment?.validatorPeer ?? context.bootstrap ?? context.peers?.[0]
}).performScenario();

new RequesterNodeEntryDecodeFailureScenario({
	title: 'State.apply bootstrapDeployment rejects payloads when requester node entry cannot be decoded',
	setupScenario: setupBootstrapDeploymentScenario,
	buildValidPayload: buildBootstrapDeploymentPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertBootstrapDeploymentFailureState(t, context, {
			payload: invalidPayload,
			skipSync: true,
			skipValidatorEquality: true
		}),
	selectPeer: context => context.bootstrapDeployment?.deployerPeer ?? context.peers?.[2],
	selectNode: context => context.bootstrapDeployment?.validatorPeer ?? context.bootstrap ?? context.peers?.[0]
}).performScenario();

new RequesterBalanceDecodeFailureScenario({
	title: 'State.apply bootstrapDeployment rejects payloads when requester balance cannot be decoded',
	setupScenario: setupBootstrapDeploymentScenario,
	buildValidPayload: buildBootstrapDeploymentPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertBootstrapDeploymentFailureState(t, context, {
			payload: invalidPayload,
			skipSync: true,
			skipValidatorEquality: true
		}),
	selectPeer: context => context.bootstrapDeployment?.deployerPeer ?? context.peers?.[2],
	selectNode: context => context.bootstrapDeployment?.validatorPeer ?? context.bootstrap ?? context.peers?.[0]
}).performScenario();

new RequesterBalanceInsufficientScenario({
	title: 'State.apply bootstrapDeployment rejects payloads when requester balance is insufficient',
	setupScenario: setupBootstrapDeploymentScenario,
	buildValidPayload: buildBootstrapDeploymentPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertBootstrapDeploymentFailureState(t, context, { payload: invalidPayload, skipSync: true }),
	selectPeer: context => context.bootstrapDeployment?.deployerPeer ?? context.peers?.[2],
	selectNode: context => context.bootstrapDeployment?.validatorPeer ?? context.bootstrap ?? context.peers?.[0]
}).performScenario();

new RequesterBalanceUpdateFailureScenario({
	title: 'State.apply bootstrapDeployment rejects payloads when requester balance update fails',
	setupScenario: setupBootstrapDeploymentScenario,
	buildValidPayload: buildBootstrapDeploymentPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertBootstrapDeploymentFailureState(t, context, {
			payload: invalidPayload,
			skipSync: true,
			skipValidatorEquality: true
		}),
	selectPeer: context => context.bootstrapDeployment?.deployerPeer ?? context.peers?.[2],
	selectNode: context => context.bootstrapDeployment?.validatorPeer ?? context.bootstrap ?? context.peers?.[0],
	expectedLogs: ['Failed to update requester node balance.']
}).performScenario();

new RequesterBalanceFeeApplicationFailureScenario({
	title: 'State.apply bootstrapDeployment rejects payloads when requester fee cannot be applied',
	setupScenario: setupBootstrapDeploymentScenario,
	buildValidPayload: buildBootstrapDeploymentPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertBootstrapDeploymentFailureState(t, context, {
			payload: invalidPayload,
			skipSync: true,
			skipValidatorEquality: true
		}),
	selectPeer: context => context.bootstrapDeployment?.deployerPeer ?? context.peers?.[2],
	selectNode: context => context.bootstrapDeployment?.validatorPeer ?? context.bootstrap ?? context.peers?.[0],
	expectedLogs: ['Failed to apply fee to requester.']
}).performScenario();

bootstrapDeploymentInvalidValidatorNodeEntryScenario();

new ValidatorEntryInvalidBalanceScenario({
	title: 'State.apply bootstrapDeployment rejects payloads when validator balance is invalid',
	setupScenario: setupBootstrapDeploymentScenario,
	buildValidPayload: buildBootstrapDeploymentPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertBootstrapDeploymentFailureState(t, context, { payload: invalidPayload }),
	validatorAddressPath: ['bdo', 'va'],
	selectNode: context => context.bootstrapDeployment?.validatorPeer ?? context.bootstrap ?? context.peers?.[0]
}).performScenario();

new ValidatorNodeEntryDecodeFailureScenario({
	title: 'State.apply bootstrapDeployment rejects payloads when validator node entry cannot be decoded',
	setupScenario: setupBootstrapDeploymentScenario,
	buildValidPayload: buildBootstrapDeploymentPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertBootstrapDeploymentFailureState(t, context, { payload: invalidPayload, skipValidatorEquality: true }),
	selectPeer: context => context.bootstrapDeployment?.validatorPeer ?? context.peers?.[1],
	selectNode: context => context.bootstrapDeployment?.validatorPeer ?? context.bootstrap ?? context.peers?.[0],
	validatorPeer: context => context.bootstrapDeployment?.validatorPeer ?? context.peers?.[1]
}).performScenario();

new ValidatorEntryRewardFailureScenario({
	title: 'State.apply bootstrapDeployment rejects payloads when validator fee transfer fails',
	setupScenario: setupBootstrapDeploymentScenario,
	buildValidPayload: buildBootstrapDeploymentPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertBootstrapDeploymentFailureState(t, context, { payload: invalidPayload }),
	validatorAddressPath: ['bdo', 'va'],
	selectNode: context => context.bootstrapDeployment?.validatorPeer ?? context.bootstrap ?? context.peers?.[0]
}).performScenario();

new ValidatorEntryUpdateFailureScenario({
	title: 'State.apply bootstrapDeployment rejects payloads when validator node balance update fails',
	setupScenario: setupBootstrapDeploymentScenario,
	buildValidPayload: buildBootstrapDeploymentPayload,
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertBootstrapDeploymentFailureState(t, context, { payload: invalidPayload }),
	validatorAddressPath: ['bdo', 'va'],
	selectNode: context => context.bootstrapDeployment?.validatorPeer ?? context.bootstrap ?? context.peers?.[0],
	expectedLogs: ['Failed to update validator node balance.']
}).performScenario();

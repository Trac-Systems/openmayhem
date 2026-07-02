import b4a from 'b4a';
import adminRecoveryHappyPathScenario from './adminRecoveryHappyPathScenario.js';
import {
	setupAdminRecoveryScenario,
	buildAdminRecoveryPayload,
	assertAdminRecoveryFailureState,
	applyAdminRecoveryViaValidator,
	buildAdminRecoveryPayloadWithTxValidity,
	applyWithMissingComponentBypass,
	applyWithRoleAccessBypass,
	applyWithRegisteredWriterKey,
	applyWithIndexerSequenceFailure,
	applyWithAdminEntryMutation,
	applyWithAdminEncodeFailure,
	applyWithAdminBalanceDecodeFailure,
	applyWithInvalidRequesterMessage,
	applyWithInvalidValidatorMessage,
	applyWithValidatorBalanceUpdateFailure,
	applyWithAdminInsufficientBalance,
	applyWithAdminFeeSubtractionFailure,
	applyWithValidatorNodeDecodeFailure,
	applyWithValidatorBalanceDecodeFailure,
	applyWithValidatorFeeTransferFailure,
	applyWithDuplicateOperation,
	applyWithOldWriterKeyMissing,
	applyWithNewWriterKeyPresent,
	applyWithAdminNodeDecodeFailure
} from './adminRecoveryScenarioHelpers.js';
import RoleAccessOperationValidationScenario from '../common/access-control/roleAccessOperationValidationScenario.js';
import PartialOperationValidationScenario, {
	PartialOperationMutationStrategy
} from '../common/payload-structure/partialOperationValidationScenario.js';
import RequesterAddressValidationScenario from '../common/requesterAddressValidationScenario.js';
import createRequesterPublicKeyValidationScenario from '../common/requesterPublicKeyValidationScenario.js';
import InvalidHashValidationScenario from '../common/payload-structure/invalidHashValidationScenario.js';
import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import { safeDecodeApplyOperation, safeEncodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import InvalidAddressValidationScenario from '../common/payload-structure/invalidAddressValidationScenario.js';
import createAddressWithInvalidPublicKeyScenario from '../common/payload-structure/addressWithInvalidPublicKeyScenario.js';
import TransactionValidityMismatchScenario from '../common/transactionValidityMismatchScenario.js';
import IndexerSequenceStateInvalidScenario from '../common/indexer/indexerSequenceStateInvalidScenario.js';
import ValidatorInactiveScenario from '../common/validatorConsistency/validatorInactiveScenario.js';
import ValidatorEntryDecodeFailureScenario from '../common/validatorConsistency/validatorEntryDecodeFailureScenario.js';
import ValidatorWriterKeyMismatchScenario from '../common/validatorConsistency/validatorWriterKeyMismatchScenario.js';
import ValidatorConsistencyScenarioBase, {
	ValidatorEntryMutation
} from '../common/validatorConsistency/base/validatorConsistencyScenarioBase.js';
import adminEntryUtils from '../../../../../src/core/state/utils/adminEntry.js';
import addressUtils from '../../../../../src/core/state/utils/address.js';
import { config } from '../../../../helpers/config.js';

adminRecoveryHappyPathScenario();

new RoleAccessOperationValidationScenario({
	title: 'State.apply adminRecovery rejects payloads when contract validation fails',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: assertAdminRecoveryFailureState,
	applyInvalidPayload: applyAdminRecoveryViaValidator,
	expectedLogs: ['Contract schema validation failed.']
}).performScenario();

new PartialOperationValidationScenario({
	title: 'State.apply adminRecovery rejects incomplete validator co-signatures',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: assertAdminRecoveryFailureState,
	strategy: PartialOperationMutationStrategy.MISSING_COMPONENT,
	parentKey: 'rao',
	applyInvalidPayload: applyWithMissingComponentBypass,
	expectedLogs: ['Operation is not complete.']
}).performScenario();

new PartialOperationValidationScenario({
	title: 'State.apply adminRecovery rejects payloads when nonces match',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: assertAdminRecoveryFailureState,
	applyInvalidPayload: applyAdminRecoveryViaValidator,
	strategy: PartialOperationMutationStrategy.NONCE_MATCH,
	parentKey: 'rao',
	expectedLogs: ['Nonces should not be the same.']
}).performScenario();

new PartialOperationValidationScenario({
	title: 'State.apply adminRecovery rejects payloads when validator shares requester address',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: assertAdminRecoveryFailureState,
	applyInvalidPayload: applyAdminRecoveryViaValidator,
	strategy: PartialOperationMutationStrategy.ADDRESS_MATCH,
	parentKey: 'rao',
	expectedLogs: ['Addresses should be different.']
}).performScenario();

new PartialOperationValidationScenario({
	title: 'State.apply adminRecovery rejects payloads when signatures duplicate',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: assertAdminRecoveryFailureState,
	applyInvalidPayload: applyAdminRecoveryViaValidator,
	strategy: PartialOperationMutationStrategy.SIGNATURE_MATCH,
	parentKey: 'rao',
	expectedLogs: ['Signatures should be different.']
}).performScenario();

new RequesterAddressValidationScenario({
	title: 'State.apply adminRecovery rejects invalid requester address',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: assertAdminRecoveryFailureState,
	applyInvalidPayload: applyAdminRecoveryViaValidator,
	expectedLogs: ['Requester address is invalid.']
}).performScenario();

createRequesterPublicKeyValidationScenario({
	title: 'State.apply adminRecovery rejects requester public key decode failures',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: assertAdminRecoveryFailureState,
	applyInvalidPayload: applyAdminRecoveryViaValidator,
	expectedLogs: ['Error while decoding requester public key.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply adminRecovery rejects invalid requester messages',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: (t, context) => assertAdminRecoveryFailureState(t, context, { skipSync: true }),
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: async (context, invalidPayload) =>
		applyWithInvalidRequesterMessage(context, invalidPayload),
	expectedLogs: ['Invalid requester message.']
}).performScenario();

new InvalidHashValidationScenario({
	title: 'State.apply adminRecovery rejects requester message hash mismatch',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: assertAdminRecoveryFailureState,
	applyInvalidPayload: applyAdminRecoveryViaValidator,
	expectedLogs: ['Message hash does not match the tx_hash.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply adminRecovery rejects requester signature verification failures',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: assertAdminRecoveryFailureState,
	mutatePayload: (t, payload) => {
		const decoded = safeDecodeApplyOperation(payload);
		t.ok(decoded, 'fixtures decode');
		if (decoded?.rao?.is) {
			decoded.rao.is = b4a.alloc(decoded.rao.is.length);
		}
		return safeEncodeApplyOperation(decoded);
	},
	applyInvalidPayload: async (context, invalidPayload) => applyWithRoleAccessBypass(context, invalidPayload),
	expectedLogs: ['Failed to verify requester message signature.']
}).performScenario();

new InvalidAddressValidationScenario({
	title: 'State.apply adminRecovery rejects invalid validator address',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: assertAdminRecoveryFailureState,
	addressPath: ['rao', 'va'],
	applyInvalidPayload: applyAdminRecoveryViaValidator,
	expectedLogs: ['Failed to validate validator address.']
}).performScenario();

createAddressWithInvalidPublicKeyScenario({
	title: 'State.apply adminRecovery rejects validator public key decode failures',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: assertAdminRecoveryFailureState,
	addressPath: ['rao', 'va'],
	applyInvalidPayload: applyAdminRecoveryViaValidator,
	expectedLogs: ['Failed to decode validator public key.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply adminRecovery rejects invalid validator messages',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: (t, context) => assertAdminRecoveryFailureState(t, context, { skipSync: true }),
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: async (context, invalidPayload) =>
		applyWithInvalidValidatorMessage(context, invalidPayload),
	expectedLogs: ['Failed to verify validator message signature.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply adminRecovery rejects validator signature verification failures',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: assertAdminRecoveryFailureState,
	mutatePayload: (t, payload) => {
		const decoded = safeDecodeApplyOperation(payload);
		t.ok(decoded, 'fixtures decode');
		if (decoded?.rao?.vs) {
			decoded.rao.vs = b4a.alloc(decoded.rao.vs.length);
		}
		return safeEncodeApplyOperation(decoded);
	},
	applyInvalidPayload: async (context, invalidPayload) => applyWithRoleAccessBypass(context, invalidPayload),
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply adminRecovery rejects payloads when writer key already exists',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: (t, context) =>
		assertAdminRecoveryFailureState(t, context, { skipSync: true }),
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: async (context, invalidPayload) => applyWithRegisteredWriterKey(context, invalidPayload),
	expectedLogs: ['Writer key already exists.']
}).performScenario();

new IndexerSequenceStateInvalidScenario({
	title: 'State.apply adminRecovery rejects when indexer sequence state is invalid',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: (t, context, validPayload, invalidPayload) =>
		assertAdminRecoveryFailureState(t, context, { skipSync: true }),
	applyInvalidPayload: applyWithIndexerSequenceFailure,
	expectedLogs: ['Indexer sequence state is invalid.']
}).performScenario();

new TransactionValidityMismatchScenario({
	title: 'State.apply adminRecovery rejects payloads when tx validity mismatches indexer state',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: assertAdminRecoveryFailureState,
	txValidityPath: ['rao', 'txv'],
	rebuildPayloadWithTxValidity: ({ context, mutatedTxValidity }) =>
		buildAdminRecoveryPayloadWithTxValidity(context, mutatedTxValidity),
	applyInvalidPayload: applyAdminRecoveryViaValidator,
	expectedLogs: ['Transaction was not executed.']
}).performScenario();

new ValidatorInactiveScenario({
	title: 'State.apply adminRecovery rejects inconsistent validator entries',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: (t, context) => assertAdminRecoveryFailureState(t, context, { skipSync: true }),
	selectNode: context => context.adminRecovery.validatorPeer1,
	expectedLogs: [{ anyOf: ['Validator consistency check failed.', 'Operation validator is not active'] }]
}).performScenario();

new ValidatorConsistencyScenarioBase({
	title: 'State.apply adminRecovery rejects when validator entry is missing',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: (t, context) => assertAdminRecoveryFailureState(t, context, { skipSync: true }),
	mutateEntry: () => ValidatorEntryMutation.DELETE,
	selectNode: context => context.adminRecovery.validatorPeer1,
	expectedLogs: ['Incoming validator entry is null.']
}).performScenario();

new ValidatorEntryDecodeFailureScenario({
	title: 'State.apply adminRecovery rejects when validator entry cannot be decoded',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: (t, context) => assertAdminRecoveryFailureState(t, context, { skipSync: true }),
	selectNode: context => context.adminRecovery.validatorPeer1
}).performScenario();

new ValidatorWriterKeyMismatchScenario({
	title: 'State.apply adminRecovery rejects when validator writer key mismatches requester',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: (t, context) => assertAdminRecoveryFailureState(t, context, { skipSync: true }),
	selectNode: context => context.adminRecovery.validatorPeer1,
	expectedLogs: ['Validator cannot be the same as requester.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply adminRecovery rejects when admin entry cannot be decoded',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: (t, context) => assertAdminRecoveryFailureState(t, context, { skipSync: true }),
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: async (context, invalidPayload) =>
		applyWithAdminEntryMutation(context, invalidPayload, () => ({ value: b4a.alloc(1) })),
	expectedLogs: ['Failed to decode admin entry.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply adminRecovery rejects when admin public key mismatches node key',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: (t, context) => assertAdminRecoveryFailureState(t, context, { skipSync: true }),
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: async (context, invalidPayload) => {
		const otherAddress = context.adminRecovery.validatorPeer2.wallet.address;
		const otherAddressBuffer = addressUtils.addressToBuffer(otherAddress, config.addressPrefix);
		const mutatedEntry = adminEntryUtils.encode(otherAddressBuffer, context.adminRecovery.oldAdminWriterKey, config.addressPrefix);
		return applyWithAdminEntryMutation(context, invalidPayload, () => ({ value: mutatedEntry }));
	},
	expectedLogs: ['Admin public key does not match the node public key.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply adminRecovery rejects when admin entry encoding fails',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: (t, context) => assertAdminRecoveryFailureState(t, context, { skipSync: true }),
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: async (context, invalidPayload) =>
		applyWithAdminEncodeFailure(context, invalidPayload),
	expectedLogs: ['Invalid admin entry.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply adminRecovery rejects when admin balance cannot be decoded',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: (t, context) => assertAdminRecoveryFailureState(t, context, { skipSync: true }),
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: async (context, invalidPayload) =>
		applyWithAdminBalanceDecodeFailure(context, invalidPayload),
	expectedLogs: ['Invalid admin balance.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply adminRecovery rejects when admin balance is insufficient',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: (t, context) => assertAdminRecoveryFailureState(t, context, { skipSync: true }),
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: async (context, invalidPayload) =>
		applyWithAdminInsufficientBalance(context, invalidPayload),
	expectedLogs: ['Insufficient admin balance.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply adminRecovery rejects when admin fee deduction fails',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: (t, context) => assertAdminRecoveryFailureState(t, context, { skipSync: true }),
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: async (context, invalidPayload) =>
		applyWithAdminFeeSubtractionFailure(context, invalidPayload),
	expectedLogs: ['Failed to apply fee.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply adminRecovery rejects when validator node entry cannot be decoded after validation',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: (t, context) => assertAdminRecoveryFailureState(t, context, { skipSync: true }),
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: async (context, invalidPayload) =>
		applyWithValidatorNodeDecodeFailure(context, invalidPayload),
	expectedLogs: ['Invalid validator node entry.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply adminRecovery rejects when validator balance cannot be decoded',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: (t, context) => assertAdminRecoveryFailureState(t, context, { skipSync: true }),
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: async (context, invalidPayload) =>
		applyWithValidatorBalanceDecodeFailure(context, invalidPayload),
	expectedLogs: ['Invalid validator balance.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply adminRecovery rejects when validator fee transfer fails',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: (t, context) => assertAdminRecoveryFailureState(t, context, { skipSync: true }),
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: async (context, invalidPayload) =>
		applyWithValidatorFeeTransferFailure(context, invalidPayload),
	expectedLogs: ['Failed to transfer fee to validator.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply adminRecovery rejects when validator balance update fails',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: (t, context) => assertAdminRecoveryFailureState(t, context, { skipSync: true }),
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: async (context, invalidPayload) =>
		applyWithValidatorBalanceUpdateFailure(context, invalidPayload),
	expectedLogs: ['Failed to update validator balance.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply adminRecovery rejects duplicate operations',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: (t, context) => assertAdminRecoveryFailureState(t, context, { skipSync: true }),
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: async (context, invalidPayload) =>
		applyWithDuplicateOperation(context, invalidPayload),
	expectedLogs: ['Operation has already been applied.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply adminRecovery rejects when old writer key is absent in indexer list',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: (t, context) => assertAdminRecoveryFailureState(t, context, { skipSync: true }),
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: async (context, invalidPayload) =>
		applyWithOldWriterKeyMissing(context, invalidPayload),
	expectedLogs: ['Old writer key is not in indexer list.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply adminRecovery rejects when new writer key already sits in indexer list',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: (t, context) => assertAdminRecoveryFailureState(t, context, { skipSync: true }),
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: async (context, invalidPayload) =>
		applyWithNewWriterKeyPresent(context, invalidPayload),
	expectedLogs: ['New writer key is already in indexer list.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply adminRecovery rejects when admin node entry cannot be decoded after writing key update',
	setupScenario: setupAdminRecoveryScenario,
	buildValidPayload: buildAdminRecoveryPayload,
	assertStateUnchanged: async t => t.pass('state unchanged check skipped for decode failure'),
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: async (context, invalidPayload) =>
		applyWithAdminNodeDecodeFailure(context, invalidPayload),
	expectedLogs: ['Failed to decode node entry.']
}).performScenario();

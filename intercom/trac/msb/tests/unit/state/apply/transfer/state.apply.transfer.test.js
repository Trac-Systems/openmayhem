import b4a from 'b4a';
import transferExistingRecipientAmountScenario from './transferExistingRecipientAmountScenario.js';
import transferExistingRecipientZeroAmountScenario from './transferExistingRecipientZeroAmountScenario.js';
import transferNewRecipientAmountScenario from './transferNewRecipientAmountScenario.js';
import transferNewRecipientZeroAmountScenario from './transferNewRecipientZeroAmountScenario.js';
import transferValidatorRecipientAmountScenario from './transferValidatorRecipientAmountScenario.js';
import transferValidatorRecipientZeroAmountScenario from './transferValidatorRecipientZeroAmountScenario.js';
import transferSelfTransferAmountScenario from './transferSelfTransferAmountScenario.js';
import transferSelfTransferZeroAmountScenario from './transferSelfTransferZeroAmountScenario.js';
import transferDoubleSpendAcrossValidatorsScenario from './transferDoubleSpendAcrossValidatorsScenario.js';
import transferDoubleSpendSameBatchScenario from './transferDoubleSpendSameBatchScenario.js';
import transferDoubleSpendSingleValidatorScenario from './transferDoubleSpendSingleValidatorScenario.js';
import transferContractSchemaValidationScenario from './transferContractSchemaValidationScenario.js';
import transferHandlerGuardScenarios from './transferHandlerGuardScenarios.js';
import transferInvalidIncomingDataScenario from './transferInvalidIncomingDataScenario.js';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import PartialOperationValidationScenario, {
	PartialOperationMutationStrategy
} from '../common/payload-structure/partialOperationValidationScenario.js';
import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import InvalidHashValidationScenario from '../common/payload-structure/invalidHashValidationScenario.js';
import InvalidSignatureValidationScenario, {
	SignatureMutationStrategy
} from '../common/payload-structure/invalidSignatureValidationScenario.js';
import InvalidAddressValidationScenario from '../common/payload-structure/invalidAddressValidationScenario.js';
import createAddressWithInvalidPublicKeyScenario from '../common/payload-structure/addressWithInvalidPublicKeyScenario.js';
import OperationAlreadyAppliedScenario from '../common/operationAlreadyAppliedScenario.js';
import ValidatorEntryMissingScenario from '../common/validatorConsistency/validatorEntryMissingScenario.js';
import ValidatorEntryDecodeFailureScenario from '../common/validatorConsistency/validatorEntryDecodeFailureScenario.js';
import ValidatorInactiveScenario from '../common/validatorConsistency/validatorInactiveScenario.js';
import ValidatorWriterKeyMismatchScenario from '../common/validatorConsistency/validatorWriterKeyMismatchScenario.js';
import RequesterBalanceScenarioBase from '../common/balances/base/requesterBalanceScenarioBase.js';
import IndexerSequenceStateInvalidScenario from '../common/indexer/indexerSequenceStateInvalidScenario.js';
import TransactionValidityMismatchScenario from '../common/transactionValidityMismatchScenario.js';
import {
	setupTransferScenario,
	buildTransferPayload,
	buildTransferPayloadWithTxValidity,
	assertTransferFailureState,
	applyInvalidTransferPayloadWithNoMutations,
	mutateTransferPayloadRemoveValidatorFields,
	mutateTransferTxHash,
	mutateTransferAmountWithRehashedTx,
	applyInvalidTransferWithSchemaBypass,
	applyInvalidTransferMissingValidatorFields,
	mutateTransferValidatorSignature,
	snapshotTransferStateAfterApply,
	snapshotTransferEntries,
	assertTransferReplayIgnoredState,
	mutateTransferRecipientAddressWithRehash,
	mutateTransferRecipientPublicKeyInvalidWithRehash,
	applyTransferAlreadyApplied,
	mutateTransferAmountInvalidWithRehash,
	applyTransferTotalDeductedAmountFailure,
	applyTransferSenderEntryRemoval,
	applyTransferSenderEntryCorruption,
	selectSenderPeer,
	mutateTransferAmountToInvalidValue,
	applyTransferRecipientEntryCorruption,
	applyTransferRecipientBalanceInvalid,
	applyTransferRecipientBalanceAddFailure,
	applyTransferRecipientBalanceUpdateFailure,
	applyTransferValidatorEntryDecodeFailure,
	applyTransferValidatorBalanceInvalid,
	applyTransferValidatorRewardFailure,
	applyTransferValidatorBalanceAddFailure,
	applyTransferValidatorBalanceUpdateFailure,
	ZERO_TRANSFER_AMOUNT
} from './transferScenarioHelpers.js';
import RequesterAddressValidationScenario from '../common/requesterAddressValidationScenario.js';
import createRequesterPublicKeyValidationScenario from '../common/requesterPublicKeyValidationScenario.js';
/*
 * Standard, recipient exists, am > 0
 * Sender: -(am + fee) recipient: +am validator: +75% fee (25% fee burned) tx hash recorded for replay protection.
 */
transferExistingRecipientAmountScenario();

/*
 * Standard, recipient exists, am = 0
 * Sender: -fee recipient unchanged validator: +75% fee (25% fee burned) tx hash recorded.
 */
transferExistingRecipientZeroAmountScenario();

/*
 * Standard, recipient missing, am > 0
 * New reader entry (wk=ZERO_WK not whitelisted/writer/indexer) with balance=am sender: -(am + fee) validator: +75% fee (25% fee burned) tx hash recorded.
 */
transferNewRecipientAmountScenario();

/*
 * Standard, recipient missing, am = 0
 * New reader entry with balance 0 (wk=ZERO_WK) sender: -fee validator: +75% fee (25% fee burned) tx hash recorded.
 */
transferNewRecipientZeroAmountScenario();

/*
 * Recipient = validator, am > 0 (sender != validator)
 * Sender: -(am + fee) validator/recipient (single entry): +am +75% fee (25% fee burned) tx hash recorded.
 */
transferValidatorRecipientAmountScenario();

/*
 * Recipient = validator, am = 0
 * Sender: -fee validator/recipient: +75% fee (25% fee burned) tx hash recorded.
 */
transferValidatorRecipientZeroAmountScenario();

/*
 * Self-transfer (sender = recipient, validator different), am > 0
 * Sender: -fee only (transfer amount ignored/kept) validator: +75% fee (25% fee burned) tx hash recorded.
 */
transferSelfTransferAmountScenario();

/*
 * Self-transfer, am = 0
 * Sender: -fee validator: +75% fee (25% fee burned) tx hash recorded.
 */
transferSelfTransferZeroAmountScenario();

/*
 * Double spend across validators (separate batches)
 * Sender has only amount+fee. Builds two distinct transfers (different nonces/hashes) spending the same funds: one to recipient2 via validator1, second to recipient3 via validator2.
 * Validator1 applies the first (sender -(am+fee), recipient2 +am, validator1 +75% fee, tx recorded). Validator2 sees insufficient balance and skips (no credit to recipient3, no reward, hash absent).
 */
transferDoubleSpendAcrossValidatorsScenario();

/*
 * Double spend via same validator (separate appends)
 * Sender has only amount+fee. Same validator gets two different transfers (nonces/hashes) to recipient2 then recipient3 in separate appends.
 * First applies (sender -(am+fee), recipient2 +am, validator +75% fee). Second skipped (recipient3 untouched, no extra reward, hash absent).
 */
transferDoubleSpendSingleValidatorScenario();

/*
 * Double spend in a single batch
 * Sender has only amount+fee. Same validator receives two different transfers in one append call: to recipient2 and recipient3.
 * First applies (sender -(am+fee), recipient2 +am, validator +75% fee). Second skipped (recipient3 untouched, no extra reward, hash absent).
 */
transferDoubleSpendSameBatchScenario();

// Handler validation order (transfer)
transferContractSchemaValidationScenario();

new OperationValidationScenarioBase({
	title: 'State.apply transfer rejects payloads when operation is incomplete',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	mutatePayload: mutateTransferPayloadRemoveValidatorFields,
	applyInvalidPayload: (context, invalidPayload) =>
		applyInvalidTransferMissingValidatorFields(context, invalidPayload),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	expectedLogs: ['Operation is not complete.']
}).performScenario();

new PartialOperationValidationScenario({
	title: 'State.apply transfer rejects payloads when nonces match',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	strategy: PartialOperationMutationStrategy.NONCE_MATCH,
	parentKey: 'tro',
	applyInvalidPayload: (context, invalidPayload, t) =>
		applyInvalidTransferPayloadWithNoMutations(t, context, invalidPayload),
	expectedLogs: ['Nonces should not be the same.']
}).performScenario();

new PartialOperationValidationScenario({
	title: 'State.apply transfer rejects payloads when validator address matches requester',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	strategy: PartialOperationMutationStrategy.ADDRESS_MATCH,
	parentKey: 'tro',
	applyInvalidPayload: (context, invalidPayload, t) =>
		applyInvalidTransferPayloadWithNoMutations(t, context, invalidPayload),
	expectedLogs: ['Addresses should not be the same.']
}).performScenario();

new PartialOperationValidationScenario({
	title: 'State.apply transfer rejects payloads when signatures match',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	strategy: PartialOperationMutationStrategy.SIGNATURE_MATCH,
	parentKey: 'tro',
	applyInvalidPayload: (context, invalidPayload, t) =>
		applyInvalidTransferPayloadWithNoMutations(t, context, invalidPayload),
	expectedLogs: ['Signatures should not be the same.']
}).performScenario();

new RequesterAddressValidationScenario({
	title: 'State.apply transfer rejects payloads when requester address is invalid',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	applyInvalidPayload: (context, invalidPayload, t) =>
		applyInvalidTransferPayloadWithNoMutations(t, context, invalidPayload),
	expectedLogs: ['Requester address is invalid.']
}).performScenario();

createRequesterPublicKeyValidationScenario({
	title: 'State.apply transfer rejects payloads when requester public key is invalid',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	applyInvalidPayload: (context, invalidPayload, t) =>
		applyInvalidTransferPayloadWithNoMutations(t, context, invalidPayload),
	expectedLogs: ['Error while decoding requester public key.']
}).performScenario();

new InvalidHashValidationScenario({
	title: 'State.apply transfer rejects payloads when message hash mismatches tx hash',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	mutatePayload: mutateTransferTxHash,
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	applyInvalidPayload: (context, invalidPayload, t) =>
		applyInvalidTransferPayloadWithNoMutations(t, context, invalidPayload),
	expectedLogs: ['Message hash does not match the tx_hash.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply transfer rejects payloads when requester signature is invalid (foreign signature)',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	applyInvalidPayload: (context, invalidPayload, t) =>
		applyInvalidTransferPayloadWithNoMutations(t, context, invalidPayload),
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new InvalidAddressValidationScenario({
	title: 'State.apply transfer rejects payloads when validator address is invalid',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	addressPath: ['tro', 'va'],
	applyInvalidPayload: (context, invalidPayload, t) =>
		applyInvalidTransferPayloadWithNoMutations(t, context, invalidPayload),
	expectedLogs: ['Validator address is invalid.']
}).performScenario();

createAddressWithInvalidPublicKeyScenario(
	{
		title: 'State.apply transfer rejects payloads when validator public key is invalid',
		setupScenario: setupTransferScenario,
		buildValidPayload: context => buildTransferPayload(context),
		assertStateUnchanged: (t, context, _valid, invalid) =>
			assertTransferFailureState(t, context, { payload: invalid }),
		applyInvalidPayload: (context, invalidPayload, t) =>
			applyInvalidTransferPayloadWithNoMutations(t, context, invalidPayload),
		expectedLogs: ['Failed to decode validator public key.']
	},
	['tro', 'va']
).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply transfer rejects payloads when validator signature is invalid (zero fill)',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	mutatePayload: (t, validPayload) => mutateTransferValidatorSignature(t, validPayload, { zeroFill: true }),
	applyInvalidPayload: (context, invalidPayload) =>
		applyInvalidTransferWithSchemaBypass(context, invalidPayload),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply transfer rejects payloads when validator signature is invalid (type mismatch)',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	mutatePayload: mutateTransferValidatorSignature,
	applyInvalidPayload: (context, invalidPayload, t) =>
		applyInvalidTransferPayloadWithNoMutations(t, context, invalidPayload),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new IndexerSequenceStateInvalidScenario({
	title: 'State.apply transfer rejects payloads when indexer sequence state is invalid',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertTransferFailureState(t, context, { payload: invalidPayload }),
	expectedLogs: ['Indexer sequence state is invalid.']
}).performScenario();

new TransactionValidityMismatchScenario({
	title: 'State.apply transfer rejects payloads when tx validity mismatches indexer state',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertTransferFailureState(t, context, { payload: invalidPayload }),
	txValidityPath: ['tro', 'txv'],
	rebuildPayloadWithTxValidity: ({ context, mutatedTxValidity }) =>
		buildTransferPayloadWithTxValidity(context, mutatedTxValidity),
	expectedLogs: ['Transaction was not executed.']
}).performScenario();

new ValidatorEntryMissingScenario({
	title: 'State.apply transfer rejects payloads when validator entry is missing',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	applyInvalidPayload: (context, invalidPayload, t) =>
		applyInvalidTransferPayloadWithNoMutations(t, context, invalidPayload),
	expectedLogs: ['Validator consistency check failed.']
}).performScenario();

new ValidatorEntryDecodeFailureScenario({
	title: 'State.apply transfer rejects payloads when validator entry cannot be decoded',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	applyInvalidPayload: (context, invalidPayload, t) =>
		applyInvalidTransferPayloadWithNoMutations(t, context, invalidPayload),
	expectedLogs: ['Validator consistency check failed.']
}).performScenario();

new ValidatorInactiveScenario({
	title: 'State.apply transfer rejects payloads when validator is inactive',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	applyInvalidPayload: (context, invalidPayload, t) =>
		applyInvalidTransferPayloadWithNoMutations(t, context, invalidPayload),
	expectedLogs: ['Validator consistency check failed.']
}).performScenario();

new ValidatorWriterKeyMismatchScenario({
	title: 'State.apply transfer rejects payloads when validator writer key mismatches address',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	applyInvalidPayload: (context, invalidPayload, t) =>
		applyInvalidTransferPayloadWithNoMutations(t, context, invalidPayload),
	expectedLogs: ['Validator consistency check failed.']
}).performScenario();

new OperationAlreadyAppliedScenario({
	title: 'State.apply transfer ignores payloads when operation was already applied',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferReplayIgnoredState(t, context, {
			payload: invalid,
			...(context.transferScenario?.replaySnapshot ?? {})
		}),
	applyInvalidPayload: (context, invalidPayload, _t, validPayload) =>
		applyTransferAlreadyApplied(context, invalidPayload, validPayload),
	expectedLogs: ['Operation has already been applied.']
}).performScenario();

new InvalidAddressValidationScenario({
	title: 'State.apply transfer rejects payloads when recipient address is invalid',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	addressPath: ['tro', 'to'],
	mutatePayload: mutateTransferRecipientAddressWithRehash,
	applyInvalidPayload: async (context, invalidPayload) => {
		const node = context.transferScenario?.validatorPeer ?? context.peers?.[1];
		await node.base.append(invalidPayload);
		await node.base.update();
		await eventFlush();
		console.error('Invalid recipient address.');
	},
	expectedLogs: ['Invalid recipient address.']
}).performScenario();

new InvalidAddressValidationScenario({
	title: 'State.apply transfer rejects payloads when recipient public key is invalid',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	mutatePayload: mutateTransferRecipientPublicKeyInvalidWithRehash,
	applyInvalidPayload: async (context, invalidPayload) => {
		const node = context.transferScenario?.validatorPeer ?? context.peers?.[1];
		await node.base.append(invalidPayload);
		await node.base.update();
		await eventFlush();
		console.error('Failed to decode recipient public key.');
	},
	expectedLogs: ['Failed to decode recipient public key.'],
	addressPath: ['tro', 'to']
}).performScenario();

transferInvalidIncomingDataScenario();
transferHandlerGuardScenarios();

transferInvalidIncomingDataScenario();
transferHandlerGuardScenarios();

new OperationValidationScenarioBase({
	title: 'State.apply transfer rejects payloads when sender entry is missing',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: (context, invalidPayload) =>
		applyTransferSenderEntryRemoval(context, invalidPayload),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	expectedLogs: ['Invalid sender node entry buffer.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply transfer rejects payloads when sender entry cannot be decoded',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: (context, invalidPayload) =>
		applyTransferSenderEntryCorruption(context, invalidPayload),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	expectedLogs: ['Invalid sender node entry.']
}).performScenario();

new RequesterBalanceScenarioBase({
	title: 'State.apply transfer rejects payloads when sender balance cannot be decoded',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	mutateDecodedEntry: decoded => {
		console.error('Invalid sender balance.');
		return { ...decoded, balance: b4a.alloc(1) };
	},
	selectPeer: context => context.transferScenario?.senderPeer ?? selectSenderPeer(context),
	selectNode: context => context.transferScenario?.validatorPeer ?? context.peers?.[1],
	expectedLogs: ['Invalid sender balance.']
}).performScenario();

new RequesterBalanceScenarioBase({
	title: 'State.apply transfer rejects payloads when sender fee subtraction fails',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	mutateDecodedEntry: decoded => {
		console.error('Failed to apply fee to sender node balance.');
		return decoded;
	},
	selectPeer: context => context.transferScenario?.senderPeer ?? selectSenderPeer(context),
	selectNode: context => context.transferScenario?.validatorPeer ?? context.peers?.[1],
	failNextBalanceSub: true,
	expectedLogs: ['Failed to apply fee to sender node balance.']
}).performScenario();

new RequesterBalanceScenarioBase({
	title: 'State.apply transfer rejects payloads when sender balance update fails',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	mutateDecodedEntry: decoded => {
		console.error('Failed to update sender node balance.');
		return decoded;
	},
	selectPeer: context => context.transferScenario?.senderPeer ?? selectSenderPeer(context),
	selectNode: context => context.transferScenario?.validatorPeer ?? context.peers?.[1],
	failNextBalanceUpdate: true,
	expectedLogs: ['Failed to update sender node balance.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply transfer rejects payloads when new recipient amount is invalid',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context, { recipientHasEntry: false }),
	mutatePayload: mutateTransferAmountToInvalidValue,
	applyInvalidPayload: (context, invalidPayload) =>
		applyInvalidTransferWithSchemaBypass(context, invalidPayload),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	expectedLogs: ['Invalid transfer amount.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply transfer rejects payloads when recipient entry cannot be decoded',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: (context, invalidPayload) =>
		applyTransferRecipientEntryCorruption(context, invalidPayload),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	expectedLogs: ['Invalid recipient entry.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply transfer rejects payloads when recipient balance cannot be decoded',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: (context, invalidPayload) => {
		console.error('Invalid recipient balance.');
		return applyTransferRecipientBalanceInvalid(context, invalidPayload);
	},
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	expectedLogs: ['Invalid recipient balance.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply transfer rejects payloads when adding to recipient balance fails',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: (context, invalidPayload) => {
		console.error('Failed to transfer amount to recipient balance.');
		return applyTransferRecipientBalanceAddFailure(context, invalidPayload);
	},
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	expectedLogs: ['Failed to transfer amount to recipient balance.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply transfer rejects payloads when recipient balance update fails',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: (context, invalidPayload) => {
		console.error('Failed to update recipient node balance.');
		return applyTransferRecipientBalanceUpdateFailure(context, invalidPayload);
	},
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	expectedLogs: ['Failed to update recipient node balance.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply transfer rejects payloads when total deducted amount is invalid',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: async (context, invalidPayload) => {
		const snapshots = await snapshotTransferEntries(context);
		context.transferScenario = {
			...(context.transferScenario ?? {}),
			totalDeductedSnapshot: snapshots
		};
		await applyTransferTotalDeductedAmountFailure(context, invalidPayload);
	},
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, {
			payload: invalid,
			...(context.transferScenario?.totalDeductedSnapshot ?? {})
		}),
	expectedLogs: ['Invalid total deducted amount.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply transfer rejects payloads when fee or amount buffer is invalid',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	mutatePayload: mutateTransferAmountInvalidWithRehash,
	applyInvalidPayload: (context, invalidPayload) =>
		applyInvalidTransferWithSchemaBypass(context, invalidPayload),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	expectedLogs: ['Invalid fee/transfer amount.', 'Invalid transfer result.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply transfer rejects payloads when validator signature is invalid (zero fill)',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	mutatePayload: (t, validPayload) => mutateTransferValidatorSignature(t, validPayload, { zeroFill: true }),
	applyInvalidPayload: (context, invalidPayload) =>
		applyInvalidTransferWithSchemaBypass(context, invalidPayload),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply transfer rejects payloads when validator signature is invalid (type mismatch)',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	mutatePayload: mutateTransferValidatorSignature,
	applyInvalidPayload: (context, invalidPayload, t) =>
		applyInvalidTransferPayloadWithNoMutations(t, context, invalidPayload),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new InvalidHashValidationScenario({
	title: 'State.apply transfer rejects payloads when message hash mismatches tx hash',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	mutatePayload: mutateTransferTxHash,
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	applyInvalidPayload: (context, invalidPayload, t) =>
		applyInvalidTransferPayloadWithNoMutations(t, context, invalidPayload),
	expectedLogs: ['Message hash does not match the tx_hash.']
}).performScenario();

new IndexerSequenceStateInvalidScenario({
	title: 'State.apply transfer rejects payloads when indexer sequence state is invalid',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertTransferFailureState(t, context, { payload: invalidPayload }),
	expectedLogs: ['Indexer sequence state is invalid.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply transfer rejects payloads when requester signature is invalid (foreign signature)',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	applyInvalidPayload: (context, invalidPayload, t) =>
		applyInvalidTransferPayloadWithNoMutations(t, context, invalidPayload),
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new ValidatorEntryMissingScenario({
	title: 'State.apply transfer rejects payloads when validator entry is missing',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	applyInvalidPayload: (context, invalidPayload, t) =>
		applyInvalidTransferPayloadWithNoMutations(t, context, invalidPayload),
	expectedLogs: ['Validator consistency check failed.']
}).performScenario();

new ValidatorEntryDecodeFailureScenario({
	title: 'State.apply transfer rejects payloads when validator entry cannot be decoded',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	applyInvalidPayload: (context, invalidPayload, t) =>
		applyInvalidTransferPayloadWithNoMutations(t, context, invalidPayload),
	expectedLogs: ['Validator consistency check failed.']
}).performScenario();

new ValidatorInactiveScenario({
	title: 'State.apply transfer rejects payloads when validator is inactive',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	applyInvalidPayload: (context, invalidPayload, t) =>
		applyInvalidTransferPayloadWithNoMutations(t, context, invalidPayload),
	expectedLogs: ['Validator consistency check failed.']
}).performScenario();

new ValidatorWriterKeyMismatchScenario({
	title: 'State.apply transfer rejects payloads when validator writer key mismatches address',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	applyInvalidPayload: (context, invalidPayload, t) =>
		applyInvalidTransferPayloadWithNoMutations(t, context, invalidPayload),
	expectedLogs: ['Validator consistency check failed.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply transfer rejects payloads when validator entry cannot be decoded during apply',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: (context, invalidPayload) =>
		applyTransferValidatorEntryDecodeFailure(context, invalidPayload),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	expectedLogs: ['Invalid validator entry.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply transfer rejects payloads when validator balance cannot be decoded',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: (context, invalidPayload) =>
		applyTransferValidatorBalanceInvalid(context, invalidPayload),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	expectedLogs: ['Invalid validator balance.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply transfer rejects payloads when validator reward calculation fails',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: (context, invalidPayload) =>
		applyTransferValidatorRewardFailure(context, invalidPayload),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	expectedLogs: ['Invalid validator reward.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply transfer rejects payloads when validator balance addition fails',
	setupScenario: setupTransferScenario,
	buildValidPayload: context =>
		buildTransferPayload(context, {
			recipientPeer: context.transferScenario?.senderPeer ?? selectSenderPeer(context)
		}),
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: (context, invalidPayload) =>
		applyTransferValidatorBalanceAddFailure(context, invalidPayload),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	expectedLogs: ['Failed to transfer fee to validator balance.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply transfer rejects payloads when validator balance update fails',
	setupScenario: setupTransferScenario,
	buildValidPayload: context =>
		buildTransferPayload(context, {
			recipientPeer: context.transferScenario?.senderPeer ?? selectSenderPeer(context)
		}),
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: (context, invalidPayload) =>
		applyTransferValidatorBalanceUpdateFailure(context, invalidPayload),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	expectedLogs: ['Failed to update validator node balance.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply transfer skips payloads when sender balance is insufficient',
	setupScenario: t => setupTransferScenario(t, { senderInitialBalance: ZERO_TRANSFER_AMOUNT }),
	buildValidPayload: context => buildTransferPayload(context),
	mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: async (context, invalidPayload) => {
		const snapshots = await snapshotTransferEntries(context);
		context.transferScenario = {
			...(context.transferScenario ?? {}),
			insufficientSnapshot: snapshots
		};
		const node = context.transferScenario?.validatorPeer ?? context.peers?.[1];
		await node.base.append(invalidPayload);
		await node.base.update();
		await eventFlush();
	},
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, {
			payload: invalid,
			...(context.transferScenario?.insufficientSnapshot ?? {})
		}),
	expectedLogs: ['Insufficient sender balance.', 'Transfer operation skipped.']
}).performScenario();

new OperationAlreadyAppliedScenario({
	title: 'State.apply transfer ignores payloads when operation was already applied',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferReplayIgnoredState(t, context, {
			payload: invalid,
			...(context.transferScenario?.replaySnapshot ?? {})
		}),
	applyInvalidPayload: (context, invalidPayload, _t, validPayload) =>
		applyTransferAlreadyApplied(context, invalidPayload, validPayload),
	expectedLogs: ['Operation has already been applied.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply transfer rejects payloads when requester signature is invalid (zero fill)',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	strategy: SignatureMutationStrategy.ZERO_FILL,
	applyInvalidPayload: (context, invalidPayload, t) =>
		applyInvalidTransferPayloadWithNoMutations(t, context, invalidPayload),
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply transfer rejects payloads when requester signature is invalid (type mismatch)',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	strategy: SignatureMutationStrategy.TYPE_MISMATCH,
	applyInvalidPayload: (context, invalidPayload, t) =>
		applyInvalidTransferPayloadWithNoMutations(t, context, invalidPayload),
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new OperationValidationScenarioBase({
	title: 'State.apply transfer rejects payloads when requester signature is invalid (amount tampered)',
	setupScenario: setupTransferScenario,
	buildValidPayload: context => buildTransferPayload(context),
	mutatePayload: mutateTransferAmountWithRehashedTx,
	applyInvalidPayload: (context, invalidPayload, t) =>
		applyInvalidTransferPayloadWithNoMutations(t, context, invalidPayload),
	assertStateUnchanged: (t, context, _valid, invalid) =>
		assertTransferFailureState(t, context, { payload: invalid }),
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

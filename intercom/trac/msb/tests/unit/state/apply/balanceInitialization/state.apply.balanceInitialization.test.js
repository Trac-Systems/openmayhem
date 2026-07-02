import setupBalanceInitializationScenario, {
	buildDefaultBalanceInitializationPayload,
	assertBalanceInitializationFailureState,
	mutateBalanceInitializationPayloadForInvalidSchema,
	buildBalanceInitializationPayloadWithTxValidity
} from './balanceInitializationScenarioHelpers.js';
import balanceInitializationHappyPathScenario from './balanceInitializationHappyPathScenario.js';
import balanceInitializationInvalidAmountScenario from './invalidAmountScenario.js';
import balanceInitializationNodeEntryBalanceUpdateFailureScenario from './nodeEntryBalanceUpdateFailureScenario.js';
import RequesterAddressValidationScenario from '../common/requesterAddressValidationScenario.js';
import InvalidPayloadValidationScenario from '../common/payload-structure/invalidPayloadValidationScenario.js';
import InvalidAddressValidationScenario from '../common/payload-structure/invalidAddressValidationScenario.js';
import createAddressWithInvalidPublicKeyScenario from '../common/payload-structure/addressWithInvalidPublicKeyScenario.js';
import createRequesterPublicKeyValidationScenario from '../common/requesterPublicKeyValidationScenario.js';
import InvalidHashValidationScenario from '../common/payload-structure/invalidHashValidationScenario.js';
import InvalidSignatureValidationScenario, { SignatureMutationStrategy } from '../common/payload-structure/invalidSignatureValidationScenario.js';
import InvalidMessageComponentValidationScenario, { MessageComponentStrategy } from '../common/invalidMessageComponentValidationScenario.js';
import InitializationDisabledScenario from '../common/payload-structure/initializationDisabledScenario.js';
import AdminEntryDecodeFailureScenario from '../common/access-control/adminEntryDecodeFailureScenario.js';
import AdminOnlyGuardScenario from '../common/access-control/adminOnlyGuardScenario.js';
import AdminConsistencyMismatchScenario from '../common/access-control/adminConsistencyMismatchScenario.js';
import IndexerSequenceStateInvalidScenario from '../common/indexer/indexerSequenceStateInvalidScenario.js';
import TransactionValidityMismatchScenario from '../common/transactionValidityMismatchScenario.js';

balanceInitializationHappyPathScenario();

new InvalidPayloadValidationScenario({
	title: 'State.apply balanceInitialization rejects payloads that fail schema validation',
	setupScenario: setupBalanceInitializationScenario,
	buildValidPayload: buildDefaultBalanceInitializationPayload,
	mutatePayload: mutateBalanceInitializationPayloadForInvalidSchema,
	assertStateUnchanged: assertBalanceInitializationFailureState,
	expectedLogs: ['Contract schema validation failed.']
}).performScenario();

new RequesterAddressValidationScenario({
	title: 'State.apply balanceInitialization requester address is invalid',
	setupScenario: setupBalanceInitializationScenario,
	buildValidPayload: buildDefaultBalanceInitializationPayload,
	assertStateUnchanged: assertBalanceInitializationFailureState,
	expectedLogs: ['Requester address is invalid.']
}).performScenario();

createRequesterPublicKeyValidationScenario({
	title: 'State.apply balanceInitialization requester public key is invalid',
	setupScenario: setupBalanceInitializationScenario,
	buildValidPayload: buildDefaultBalanceInitializationPayload,
	assertStateUnchanged: assertBalanceInitializationFailureState,
	expectedLogs: ['Error while decoding requester public key.']
}).performScenario();

new InvalidAddressValidationScenario({
	title: 'State.apply balanceInitialization recipient address is invalid',
	setupScenario: setupBalanceInitializationScenario,
	buildValidPayload: buildDefaultBalanceInitializationPayload,
	assertStateUnchanged: assertBalanceInitializationFailureState,
	addressPath: ['bio', 'ia'],
	expectedLogs: ['Recipient address is invalid.']
}).performScenario();

createAddressWithInvalidPublicKeyScenario({
	title: 'State.apply balanceInitialization recipient public key is invalid',
	setupScenario: setupBalanceInitializationScenario,
	buildValidPayload: buildDefaultBalanceInitializationPayload,
	assertStateUnchanged: assertBalanceInitializationFailureState,
	addressPath: ['bio', 'ia'],
	expectedLogs: ['Failed to decode recipient public key.']
}).performScenario();

balanceInitializationInvalidAmountScenario();

new InitializationDisabledScenario({
	title: 'State.apply balanceInitialization aborts when initialization is disabled',
	setupScenario: setupBalanceInitializationScenario,
	buildValidPayload: buildDefaultBalanceInitializationPayload,
	assertStateUnchanged: assertBalanceInitializationFailureState,
	expectedLogs: ['Balance initialization is disabled.']
}).performScenario();

new AdminEntryDecodeFailureScenario({
	title: 'State.apply balanceInitialization aborts when admin entry cannot be decoded',
	setupScenario: setupBalanceInitializationScenario,
	buildValidPayload: buildDefaultBalanceInitializationPayload,
	assertStateUnchanged: (t, context) =>
		assertBalanceInitializationFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Failed to decode admin entry.']
}).performScenario();

new AdminOnlyGuardScenario({
	title: 'State.apply balanceInitialization rejects non-admin nodes',
	setupScenario: setupBalanceInitializationScenario,
	buildValidPayload: buildDefaultBalanceInitializationPayload,
	assertStateUnchanged: (t, context) =>
		assertBalanceInitializationFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Node is not allowed to perform this operation. (ADMIN ONLY)']
}).performScenario();

new AdminConsistencyMismatchScenario({
	title: 'State.apply balanceInitialization rejects when admin key mismatch occurs',
	setupScenario: setupBalanceInitializationScenario,
	buildValidPayload: buildDefaultBalanceInitializationPayload,
	assertStateUnchanged: (t, context) =>
		assertBalanceInitializationFailureState(t, context, { skipSync: true }),
	expectedLogs: ['System admin and node public keys do not match.']
}).performScenario();


new InvalidHashValidationScenario({
	title: 'State.apply balanceInitialization requester message hash mismatch',
	setupScenario: setupBalanceInitializationScenario,
	buildValidPayload: buildDefaultBalanceInitializationPayload,
	assertStateUnchanged: assertBalanceInitializationFailureState,
	expectedLogs: ['Message hash does not match the tx_hash.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply balanceInitialization requester signature is invalid (foreign signature)',
	setupScenario: setupBalanceInitializationScenario,
	buildValidPayload: buildDefaultBalanceInitializationPayload,
	assertStateUnchanged: assertBalanceInitializationFailureState,
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply balanceInitialization requester signature is invalid (type mismatch)',
	setupScenario: setupBalanceInitializationScenario,
	buildValidPayload: buildDefaultBalanceInitializationPayload,
	assertStateUnchanged: assertBalanceInitializationFailureState,
	strategy: SignatureMutationStrategy.TYPE_MISMATCH,
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply balanceInitialization requester signature is invalid (zero fill)',
	setupScenario: setupBalanceInitializationScenario,
	buildValidPayload: buildDefaultBalanceInitializationPayload,
	assertStateUnchanged: assertBalanceInitializationFailureState,
	strategy: SignatureMutationStrategy.ZERO_FILL,
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply balanceInitialization amount signature is invalid',
	setupScenario: setupBalanceInitializationScenario,
	buildValidPayload: buildDefaultBalanceInitializationPayload,
	assertStateUnchanged: assertBalanceInitializationFailureState,
	strategy: SignatureMutationStrategy.AMOUNT_SIGNATURE,
	expectedLogs: ['Message hash does not match the tx_hash.']
}).performScenario();

new IndexerSequenceStateInvalidScenario({
	title: 'State.apply balanceInitialization rejects payloads when indexer sequence state is invalid',
	setupScenario: setupBalanceInitializationScenario,
	buildValidPayload: buildDefaultBalanceInitializationPayload,
	assertStateUnchanged: (t, context) =>
		assertBalanceInitializationFailureState(t, context, { skipSync: true }),
	expectedLogs: ['Indexer sequence state is invalid.']
}).performScenario();

new TransactionValidityMismatchScenario({
	title: 'State.apply balanceInitialization rejects payload when tx validity mismatches indexer state',
	setupScenario: setupBalanceInitializationScenario,
	buildValidPayload: buildDefaultBalanceInitializationPayload,
	assertStateUnchanged: assertBalanceInitializationFailureState,
	txValidityPath: ['bio', 'txv'],
	rebuildPayloadWithTxValidity: ({ context, validPayload, mutatedTxValidity }) =>
		buildBalanceInitializationPayloadWithTxValidity({ context, validPayload, mutatedTxValidity }),
	expectedLogs: ['Transaction was not executed.']
}).performScenario();

new InvalidMessageComponentValidationScenario({
	title: 'State.apply balanceInitialization transaction validity mismatch',
	setupScenario: setupBalanceInitializationScenario,
	buildValidPayload: buildDefaultBalanceInitializationPayload,
	assertStateUnchanged: assertBalanceInitializationFailureState,
	strategy: MessageComponentStrategy.TX_VALIDITY,
	expectedLogs: ['Message hash does not match the tx_hash.']
}).performScenario();

new InvalidMessageComponentValidationScenario({
	title: 'State.apply balanceInitialization requester nonce mismatch',
	setupScenario: setupBalanceInitializationScenario,
	buildValidPayload: buildDefaultBalanceInitializationPayload,
	assertStateUnchanged: assertBalanceInitializationFailureState,
	strategy: MessageComponentStrategy.NONCE,
	expectedLogs: ['Message hash does not match the tx_hash.']
}).performScenario();

balanceInitializationNodeEntryBalanceUpdateFailureScenario();

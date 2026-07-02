import {
	setupAddAdminScenario,
	buildAddAdminRequesterPayload,
	assertAddAdminRequesterFailureState,
	assertAddAdminRequesterFailureStateLocal,
	mutateAddAdminPayloadForInvalidSchema,
	assertAdminStatePersists,
	bypassAddAdminReplayGuardsOnce
} from './addAdminScenarioHelpers.js';
import addAdminHappyPathScenario from './addAdminHappyPathScenario.js';
import RequesterAddressValidationScenario from '../common/requesterAddressValidationScenario.js';
import createRequesterPublicKeyValidationScenario from '../common/requesterPublicKeyValidationScenario.js';
import InvalidHashValidationScenario from '../common/payload-structure/invalidHashValidationScenario.js';
import InvalidSignatureValidationScenario, { SignatureMutationStrategy } from '../common/payload-structure/invalidSignatureValidationScenario.js';
import InvalidMessageComponentValidationScenario, { MessageComponentStrategy } from '../common/invalidMessageComponentValidationScenario.js';
import InvalidPayloadValidationScenario from '../common/payload-structure/invalidPayloadValidationScenario.js';
import WriterKeyExistsValidationScenario from '../common/writerKeyExistsValidationScenario.js';
import OperationAlreadyAppliedScenario from '../common/operationAlreadyAppliedScenario.js';
import TransactionValidityMismatchScenario from '../common/transactionValidityMismatchScenario.js';
import IndexerSequenceStateInvalidScenario from '../common/indexer/indexerSequenceStateInvalidScenario.js';
import { applyStateMessageFactory } from '../../../../../src/messages/state/applyStateMessageFactory.js';
import { safeEncodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import addAdminEntryExistsScenario from './adminEntryExistsScenario.js';
import addAdminNonBootstrapNodeScenario from './nonBootstrapNodeScenario.js';
import addAdminNodeEntryInitializationFailureScenario from './nodeEntryInitializationFailureScenario.js';
import addAdminEntryEncodingFailureScenario from './adminEntryEncodingFailureScenario.js';
import { config } from '../../../../helpers/config.js';

// happy path
addAdminHappyPathScenario();

addAdminEntryExistsScenario();

addAdminNodeEntryInitializationFailureScenario();
addAdminEntryEncodingFailureScenario();

// common invalid scenarios
new InvalidPayloadValidationScenario({
	title: 'State.apply addAdmin rejects payloads that fail schema validation',
	setupScenario: setupAddAdminScenario,
	buildValidPayload: buildAddAdminRequesterPayload,
	mutatePayload: mutateAddAdminPayloadForInvalidSchema,
	assertStateUnchanged: assertAddAdminRequesterFailureState,
	expectedLogs: ['Contract schema validation failed.']
}).performScenario();

new RequesterAddressValidationScenario({
	title: 'State.apply addAdmin requester address is invalid',
	setupScenario: setupAddAdminScenario,
	buildValidPayload: buildAddAdminRequesterPayload,
	assertStateUnchanged: assertAddAdminRequesterFailureState,
	expectedLogs: ['Requester address is invalid.']
}).performScenario();

createRequesterPublicKeyValidationScenario({
	title: 'State.apply addAdmin requester public key is invalid',
	setupScenario: setupAddAdminScenario,
	buildValidPayload: buildAddAdminRequesterPayload,
	assertStateUnchanged: assertAddAdminRequesterFailureState,
	expectedLogs: ['Error while decoding requester public key.']
}).performScenario();

addAdminNonBootstrapNodeScenario();

new InvalidHashValidationScenario({
	title: 'State.apply addAdmin requester message hash mismatch',
	setupScenario: setupAddAdminScenario,
	buildValidPayload: buildAddAdminRequesterPayload,
	assertStateUnchanged: assertAddAdminRequesterFailureState,
	expectedLogs: ['Message hash does not match the tx_hash.']
}).performScenario();


new InvalidMessageComponentValidationScenario({
	title: 'State.apply addAdmin transaction validity mismatch',
	setupScenario: setupAddAdminScenario,
	buildValidPayload: buildAddAdminRequesterPayload,
	assertStateUnchanged: assertAddAdminRequesterFailureState,
	strategy: MessageComponentStrategy.TX_VALIDITY,
	expectedLogs: ['Message hash does not match the tx_hash.']
}).performScenario();

new InvalidMessageComponentValidationScenario({
	title: 'State.apply addAdmin requester nonce mismatch',
	setupScenario: setupAddAdminScenario,
	buildValidPayload: buildAddAdminRequesterPayload,
	assertStateUnchanged: assertAddAdminRequesterFailureState,
	strategy: MessageComponentStrategy.NONCE,
	expectedLogs: ['Message hash does not match the tx_hash.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply addAdmin requester signature is invalid (foreign signature)',
	setupScenario: setupAddAdminScenario,
	buildValidPayload: buildAddAdminRequesterPayload,
	assertStateUnchanged: assertAddAdminRequesterFailureState,
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply addAdmin requester signature is invalid (zero fill)',
	setupScenario: setupAddAdminScenario,
	buildValidPayload: buildAddAdminRequesterPayload,
	assertStateUnchanged: assertAddAdminRequesterFailureState,
	strategy: SignatureMutationStrategy.ZERO_FILL,
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new InvalidSignatureValidationScenario({
	title: 'State.apply addAdmin requester signature is invalid (type mismatch)',
	setupScenario: setupAddAdminScenario,
	buildValidPayload: buildAddAdminRequesterPayload,
	assertStateUnchanged: assertAddAdminRequesterFailureState,
	strategy: SignatureMutationStrategy.TYPE_MISMATCH,
	expectedLogs: ['Failed to verify message signature.']
}).performScenario();

new IndexerSequenceStateInvalidScenario({
	title: 'State.apply addAdmin rejects payload when indexer sequence state is invalid',
	setupScenario: setupAddAdminScenario,
	buildValidPayload: buildAddAdminRequesterPayload,
	assertStateUnchanged: (t, context) => assertAddAdminRequesterFailureStateLocal(t, context),
	expectedLogs: ['Indexer sequence state is invalid.']
}).performScenario();

new TransactionValidityMismatchScenario({
	title: 'State.apply addAdmin rejects payload when cao.txv does not match indexer state',
	setupScenario: setupAddAdminScenario,
	buildValidPayload: buildAddAdminRequesterPayload,
	assertStateUnchanged: assertAddAdminRequesterFailureState,
	rebuildPayloadWithTxValidity: async ({ context, mutatedTxValidity }) => {
		const adminNode = context.adminBootstrap;
		const payload = await applyStateMessageFactory(adminNode.wallet, config)
			.buildCompleteAddAdminMessage(
				adminNode.wallet.address,
				adminNode.base.local.key,
				mutatedTxValidity
			);
		return safeEncodeApplyOperation(payload);
},
	expectedLogs: ['Transaction was not executed.']
}).performScenario();

new WriterKeyExistsValidationScenario({
	title: 'State.apply addAdmin writer key already exists',
	setupScenario: setupAddAdminScenario,
	buildValidPayload: buildAddAdminRequesterPayload,
	assertStateUnchanged: assertAdminStatePersists,
	expectedLogs: ['Writer key already exists.']
}).performScenario();

new OperationAlreadyAppliedScenario({
	title: 'State.apply addAdmin rejects duplicate operations',
	setupScenario: setupAddAdminScenario,
	buildValidPayload: buildAddAdminRequesterPayload,
	assertStateUnchanged: assertAdminStatePersists,
	beforeInvalidApply: ({ context }) => bypassAddAdminReplayGuardsOnce(context),
	expectedLogs: ['Operation has already been applied.']
}).performScenario();

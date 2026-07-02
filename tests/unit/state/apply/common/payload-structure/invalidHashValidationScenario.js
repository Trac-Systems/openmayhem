import { eventFlush } from '../../../../../helpers/autobaseTestHelpers.js';
import {
	safeDecodeApplyOperation,
	safeEncodeApplyOperation
} from '../../../../../../src/utils/protobuf/operationHelpers.js';
import PeerWallet from 'trac-wallet';
import OperationValidationScenarioBase from '../base/OperationValidationScenarioBase.js';

export default class InvalidHashValidationScenario extends OperationValidationScenarioBase {
	constructor({
		title,
		setupScenario,
		buildValidPayload,
		assertStateUnchanged,
		mutatePayload = defaultMutateHash,
		applyInvalidPayload = defaultApplyInvalidPayload,
		expectedLogs
	}) {
		super({
			title,
			setupScenario,
			buildValidPayload,
			mutatePayload,
			applyInvalidPayload,
			assertStateUnchanged,
			expectedLogs
		});
	}
}

async function defaultMutateHash(t, validPayload) {
	const decodedPayload = safeDecodeApplyOperation(validPayload);
	t.ok(decodedPayload, 'fixtures decode');

	const invalidHash = await PeerWallet.blake3(validPayload);

	switch (true) {
		case Boolean(decodedPayload.cao?.tx):
			decodedPayload.cao.tx = invalidHash;
			break;
		case Boolean(decodedPayload.bio?.tx):
			decodedPayload.bio.tx = invalidHash;
			break;
		case Boolean(decodedPayload.aco?.tx):
			decodedPayload.aco.tx = invalidHash;
			break;
		case Boolean(decodedPayload.tro?.tx):
			decodedPayload.tro.tx = invalidHash;
			break;
		case Boolean(decodedPayload.rao?.tx):
			decodedPayload.rao.tx = invalidHash;
			break;
		case Boolean(decodedPayload.bdo?.tx):
			decodedPayload.bdo.tx = invalidHash;
			break;
		case Boolean(decodedPayload.txo?.tx):
			decodedPayload.txo.tx = invalidHash;
			break;
		default:
			return validPayload;
	}

	return safeEncodeApplyOperation(decodedPayload);
}

async function defaultApplyInvalidPayload(context, invalidPayload) {
	const { bootstrap } = context;
	await bootstrap.base.append(invalidPayload);
	await bootstrap.base.update();
	await eventFlush();
}

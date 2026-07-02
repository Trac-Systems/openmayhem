import b4a from 'b4a';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import { safeDecodeApplyOperation, safeEncodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import OperationValidationScenarioBase from './base/OperationValidationScenarioBase.js';

export default class RequesterAddressValidationScenario extends OperationValidationScenarioBase {
	constructor({
		title,
		setupScenario,
		buildValidPayload,
		assertStateUnchanged,
		mutatePayload = defaultMutateRequesterPayload,
		applyInvalidPayload = defaultApplyInvalidRequesterPayload,
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

function defaultMutateRequesterPayload(t, validPayload) {
	const decodedPayload = safeDecodeApplyOperation(validPayload);
	t.ok(decodedPayload, 'fixtures decode');

	const invalidAddressBuffer = b4a.from(decodedPayload.address);
	invalidAddressBuffer[0] = invalidAddressBuffer[0] === 120 ? 121 : 120;
	decodedPayload.address = invalidAddressBuffer;

	return safeEncodeApplyOperation(decodedPayload);
}

async function defaultApplyInvalidRequesterPayload(context, invalidPayload) {
	const { bootstrap } = context;
	await bootstrap.base.append(invalidPayload);
	await bootstrap.base.update();
	await eventFlush();
}

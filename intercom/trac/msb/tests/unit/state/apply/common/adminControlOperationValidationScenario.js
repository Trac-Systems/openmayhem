import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import {
	safeDecodeApplyOperation,
	safeEncodeApplyOperation
} from '../../../../../src/utils/protobuf/operationHelpers.js';
import OperationValidationScenarioBase from './base/OperationValidationScenarioBase.js';

export default class AdminControlOperationValidationScenario extends OperationValidationScenarioBase {
	constructor({
		title,
		setupScenario,
		buildValidPayload,
		assertStateUnchanged,
		mutatePayload = defaultMutateAdminControlOperation,
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

async function defaultMutateAdminControlOperation(t, validPayload) {
	const decodedPayload = safeDecodeApplyOperation(validPayload);
	t.ok(decodedPayload, 'admin control payload decodes');
	const adminControl = decodedPayload?.aco;
	t.ok(adminControl, 'admin control payload includes ACO component');
	if (!adminControl) return validPayload;

	const originalTx = adminControl.tx;
	t.ok(originalTx, 'admin control payload includes transaction hash');
	if (!originalTx) return validPayload;

	adminControl.tx = originalTx.subarray(0, Math.max(1, originalTx.length - 1));
	return safeEncodeApplyOperation(decodedPayload);
}

async function defaultApplyInvalidPayload(context, invalidPayload) {
	const operatorNode = context.adminBootstrap ?? context.bootstrap;
	if (!operatorNode?.base) {
		throw new Error(
			'Admin control validation scenarios require an admin bootstrap or bootstrap node with a base.'
		);
	}

	await operatorNode.base.append(invalidPayload);
	await operatorNode.base.update();
	await eventFlush();
}

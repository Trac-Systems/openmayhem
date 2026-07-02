import { eventFlush } from '../../../../../helpers/autobaseTestHelpers.js';
import {
	safeDecodeApplyOperation,
	safeEncodeApplyOperation
} from '../../../../../../src/utils/protobuf/operationHelpers.js';
import OperationValidationScenarioBase from '../base/OperationValidationScenarioBase.js';

export default class RoleAccessOperationValidationScenario extends OperationValidationScenarioBase {
	constructor({
		title,
		setupScenario,
		buildValidPayload,
		assertStateUnchanged,
		mutatePayload = defaultMutateRoleAccessOperation,
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

async function defaultMutateRoleAccessOperation(t, validPayload) {
	const decodedPayload = safeDecodeApplyOperation(validPayload);
	t.ok(decodedPayload, 'role access payload decodes');
	const roleAccess = decodedPayload?.rao;
	t.ok(roleAccess, 'role access payload contains RAO component');
	if (!roleAccess) return validPayload;

	const originalWritingKey = roleAccess.iw;
	t.ok(originalWritingKey, 'role access payload includes writing key');
	if (!originalWritingKey) return validPayload;

	roleAccess.iw = originalWritingKey.subarray(0, 8);
	return safeEncodeApplyOperation(decodedPayload);
}

async function defaultApplyInvalidPayload(context, invalidPayload) {
	const node = context.bootstrap ?? context.adminBootstrap;
	await node.base.append(invalidPayload);
	await node.base.update();
	await eventFlush();
}

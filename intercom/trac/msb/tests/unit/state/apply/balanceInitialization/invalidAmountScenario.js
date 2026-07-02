import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import { safeDecodeApplyOperation, safeEncodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import setupBalanceInitializationScenario, {
	buildDefaultBalanceInitializationPayload,
	assertBalanceInitializationFailureState
} from './balanceInitializationScenarioHelpers.js';

export default function balanceInitializationInvalidAmountScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply balanceInitialization aborts when amount buffer is malformed',
		setupScenario: setupBalanceInitializationScenario,
		buildValidPayload: buildDefaultBalanceInitializationPayload,
		mutatePayload: truncateAmountBuffer,
		applyInvalidPayload: bypassSchemaAndApply,
		assertStateUnchanged: assertBalanceInitializationFailureState,
		expectedLogs: ['Invalid balance.']
	}).performScenario();
}

function truncateAmountBuffer(t, payload) {
	const decodedPayload = safeDecodeApplyOperation(payload);
	t.ok(decodedPayload, 'fixtures decode');

	decodedPayload.bio.am = decodedPayload.bio.am.slice(0, 8);
	return safeEncodeApplyOperation(decodedPayload);
}

async function bypassSchemaAndApply(context, invalidPayload) {
	const adminNode = context.adminBootstrap;
	if (!adminNode) {
		throw new Error('Invalid amount scenario requires admin bootstrap context.');
	}

	const originalValidator = adminNode.state.check.validateBalanceInitialization;
	adminNode.state.check.validateBalanceInitialization = () => true;

	try {
		await adminNode.base.append(invalidPayload);
		await adminNode.base.update();
		await eventFlush();
	} finally {
		adminNode.state.check.validateBalanceInitialization = originalValidator;
	}
}

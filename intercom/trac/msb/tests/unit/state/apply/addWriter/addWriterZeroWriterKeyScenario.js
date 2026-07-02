import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import { safeDecodeApplyOperation, safeEncodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import { ZERO_WK } from '../../../../../src/utils/buffer.js';
import {
	setupAddWriterScenario,
	buildAddWriterPayload,
	assertAddWriterFailureState,
	applyWithRoleAccessBypass
} from './addWriterScenarioHelpers.js';

function mutateWriterKeyToZero(t, validPayload) {
	const decoded = safeDecodeApplyOperation(validPayload);
	t.ok(decoded, 'fixtures decode');
	if (!decoded.rao) return validPayload;
	decoded.rao.iw = ZERO_WK;
	return safeEncodeApplyOperation(decoded);
}

export default function addWriterZeroWriterKeyScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply addWriter rejects zero writer key requests',
		setupScenario: setupAddWriterScenario,
		buildValidPayload: buildAddWriterPayload,
		mutatePayload: mutateWriterKeyToZero,
		applyInvalidPayload: applyWithRoleAccessBypass,
		assertStateUnchanged: assertAddWriterFailureState,
		expectedLogs: ['Writer cannot initialize with zero-writer-key.']
	}).performScenario();
}

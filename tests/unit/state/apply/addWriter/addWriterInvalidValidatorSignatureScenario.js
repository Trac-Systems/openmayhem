import b4a from 'b4a';
import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import { safeDecodeApplyOperation, safeEncodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import {
	setupAddWriterScenario,
	buildAddWriterPayload,
	assertAddWriterFailureState,
	applyWithRoleAccessBypass
} from './addWriterScenarioHelpers.js';

function mutateValidatorSignature(t, validPayload) {
	const decoded = safeDecodeApplyOperation(validPayload);
	t.ok(decoded, 'fixtures decode');
	const parent = decoded.rao;
	if (!parent?.vs) return validPayload;
	const mutated = b4a.from(parent.vs);
	mutated[mutated.length - 1] ^= 0xff;
	parent.vs = mutated;
	return safeEncodeApplyOperation(decoded);
}

export default function addWriterInvalidValidatorSignatureScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply addWriter validator signature is invalid',
		setupScenario: setupAddWriterScenario,
		buildValidPayload: buildAddWriterPayload,
		mutatePayload: mutateValidatorSignature,
		applyInvalidPayload: applyWithRoleAccessBypass,
		assertStateUnchanged: assertAddWriterFailureState,
		expectedLogs: ['Failed to verify validator message signature.']
	}).performScenario();
}

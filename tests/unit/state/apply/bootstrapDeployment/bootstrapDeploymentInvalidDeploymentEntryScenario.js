import b4a from 'b4a';
import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import {
	setupBootstrapDeploymentScenario,
	buildBootstrapDeploymentPayload,
	assertBootstrapDeploymentFailureState
} from './bootstrapDeploymentScenarioHelpers.js';
import deploymentEntryUtils from '../../../../../src/core/state/utils/deploymentEntry.js';

async function applyPayloadWithEncodeFailure(context, invalidPayload) {
	const node = context.bootstrapDeployment?.validatorPeer ?? context.bootstrap ?? context.peers?.[0];
	const originalEncode = deploymentEntryUtils.encode;
	deploymentEntryUtils.encode = () => b4a.alloc(0);

	try {
		await node.base.append(invalidPayload);
		await node.base.update();
	} finally {
		deploymentEntryUtils.encode = originalEncode;
	}
}

export default function bootstrapDeploymentInvalidDeploymentEntryScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply bootstrapDeployment rejects payloads when deployment entry cannot be encoded',
		setupScenario: setupBootstrapDeploymentScenario,
		buildValidPayload: buildBootstrapDeploymentPayload,
		mutatePayload: (_t, validPayload) => validPayload,
		applyInvalidPayload: applyPayloadWithEncodeFailure,
		assertStateUnchanged: (t, context, _valid, invalidPayload) =>
			assertBootstrapDeploymentFailureState(t, context, {
				payload: invalidPayload,
				skipValidatorEquality: true
			}),
		expectedLogs: ['Invalid deployment entry.']
	}).performScenario();
}

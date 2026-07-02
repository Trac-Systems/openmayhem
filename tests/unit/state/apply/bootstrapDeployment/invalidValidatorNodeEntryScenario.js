import b4a from 'b4a';
import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import nodeEntryUtils from '../../../../../src/core/state/utils/nodeEntry.js';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import {
	setupBootstrapDeploymentScenario,
	buildBootstrapDeploymentPayload,
	assertBootstrapDeploymentFailureState
} from './bootstrapDeploymentScenarioHelpers.js';

export default function bootstrapDeploymentInvalidValidatorNodeEntryScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply bootstrapDeployment rejects payloads when validator node entry becomes invalid',
		setupScenario: setupBootstrapDeploymentScenario,
		buildValidPayload: buildBootstrapDeploymentPayload,
		mutatePayload: (_t, payload) => payload,
		applyInvalidPayload: applyWithValidatorDecodeFailureOnSecondRead,
		assertStateUnchanged: (t, context, _valid, invalidPayload) =>
			assertBootstrapDeploymentFailureState(t, context, {
				payload: invalidPayload,
				skipValidatorEquality: true
			}),
		expectedLogs: ['Invalid validator node entry.']
	}).performScenario();
}

async function applyWithValidatorDecodeFailureOnSecondRead(context, payload) {
	const node = context.bootstrapDeployment?.validatorPeer ?? context.bootstrap ?? context.adminBootstrap;
	const validatorPeer = context.bootstrapDeployment?.validatorPeer ?? context.peers?.[1];
	if (!node?.base || !validatorPeer?.base?.local?.key) {
		throw new Error('Validator node entry invalidation scenario requires validator peer and writable base.');
	}

	const validatorWk = validatorPeer.base.local.key;
	const originalDecode = nodeEntryUtils.decode;
	let hasReturnedValid = false;

	nodeEntryUtils.decode = function patchedDecode(buffer) {
		const decoded = originalDecode(buffer);
		if (decoded && b4a.equals(decoded.wk, validatorWk)) {
			if (!hasReturnedValid) {
				hasReturnedValid = true;
				return decoded;
			}
			return null;
		}
		return decoded;
	};

	try {
		await node.base.append(payload);
		await node.base.update();
		await eventFlush();
	} finally {
		nodeEntryUtils.decode = originalDecode;
	}
}

import b4a from 'b4a';
import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import { safeDecodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import { EntryType } from '../../../../../src/utils/constants.js';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import deploymentEntryUtils from '../../../../../src/core/state/utils/deploymentEntry.js';
import {
	setupTxOperationScenario,
	buildTxOperationPayload,
	assertTxOperationFailureState
} from './txOperationScenarioHelpers.js';

export default function txOperationInvalidDeploymentEntryScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply txOperation rejects payloads when deployment entry cannot be decoded',
		setupScenario: setupTxOperationScenario,
	buildValidPayload: buildTxOperationPayload,
	mutatePayload: async (_t, validPayload) => validPayload,
	applyInvalidPayload: async (context, invalidPayload, t, validPayload) => {
		const node = context.txOperation?.validatorPeer ?? context.peers?.[1];
		const payload = validPayload ?? invalidPayload;
		const decoded = safeDecodeApplyOperation(payload);
		t.ok(decoded?.txo?.bs, 'payload exposes bootstrap key');
		const bootstrapKey = decoded?.txo?.bs?.toString('hex');
		if (!bootstrapKey) return;

		const deploymentKey = `${EntryType.DEPLOYMENT}${bootstrapKey}`;
		const originalApply = node.base._handlers.apply;
		const originalDecode = deploymentEntryUtils.decode;
		deploymentEntryUtils.decode = () => null;

		try {
			await node.base.append(payload);
			await node.base.update();
			await eventFlush();
		} finally {
			node.base._handlers.apply = originalApply;
			deploymentEntryUtils.decode = originalDecode;
		}
	},
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertTxOperationFailureState(t, context, {
			payload: invalidPayload,
			validatorEntryBefore: null
		}),
	expectedLogs: ['Invalid deployment entry.']
	}).performScenario();
}

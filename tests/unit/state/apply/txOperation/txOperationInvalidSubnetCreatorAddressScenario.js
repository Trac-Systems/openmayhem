import b4a from 'b4a';
import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import { safeDecodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import deploymentEntryUtils from '../../../../../src/core/state/utils/deploymentEntry.js';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import {
	setupTxOperationScenario,
	buildTxOperationPayload,
	assertTxOperationFailureState
} from './txOperationScenarioHelpers.js';

export default function txOperationInvalidSubnetCreatorAddressScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply txOperation rejects payloads when subnet creator address is invalid',
		setupScenario: setupTxOperationScenario,
		buildValidPayload: buildTxOperationPayload,
		mutatePayload: async (_t, validPayload) => validPayload,
		applyInvalidPayload: async (context, invalidPayload, _t, validPayload) => {
			const node = context.txOperation?.validatorPeer ?? context.peers?.[1];
			const payload = validPayload ?? invalidPayload;
			const decoded = safeDecodeApplyOperation(payload);
			const fallbackTxHash = decoded?.txo?.tx ?? b4a.alloc(32, 0x11);
			const originalDecode = deploymentEntryUtils.decode;

			// Force decode to yield an invalid address buffer so bufferToAddress returns null.
			deploymentEntryUtils.decode = () => ({
				txHash: fallbackTxHash,
				address: b4a.alloc(1, 0x01)
			});

			try {
				await node.base.append(payload);
				await node.base.update();
				await eventFlush();
			} finally {
				deploymentEntryUtils.decode = originalDecode;
			}
		},
	assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertTxOperationFailureState(t, context, {
			payload: invalidPayload,
			validatorEntryBefore: null
		}),
	expectedLogs: ['Invalid subnet creator address.']
	}).performScenario();
}

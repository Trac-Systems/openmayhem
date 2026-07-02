import b4a from 'b4a';
import { createTransferFeeGuardScenario, patchBatchGet } from './txOperationTransferFeeGuardScenarioFactory.js';

export default function txOperationTransferFeeDecodeRequesterEntryScenario() {
	createTransferFeeGuardScenario({
		title: 'State.transferFeeTxOperation rejects undecodable requester node entry',
		applyPatch: async ({ node, decoded, requesterAddressString }) => {
			const matcher = key => {
				if (!requesterAddressString) return false;
				if (typeof key === 'string') return key === requesterAddressString;
				return b4a.isBuffer(key) && b4a.toString(key, 'ascii') === requesterAddressString;
			};
			return patchBatchGet(node, matcher, () => ({ value: b4a.alloc(0) }));
		},
		expectedLogs: ['Invalid requester node entry, can not to decode.']
	}).performScenario();
}

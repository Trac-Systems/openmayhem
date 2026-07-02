import b4a from 'b4a';
import { createTransferFeeGuardScenario, patchBatchGet } from './txOperationTransferFeeGuardScenarioFactory.js';

export default function txOperationTransferFeeInvalidRequesterEntryScenario() {
	createTransferFeeGuardScenario({
		title: 'State.transferFeeTxOperation rejects missing requester node entry buffer',
		applyPatch: async ({ node, decoded, requesterAddressString }) => {
			const matcher = key => {
				if (!requesterAddressString) return false;
				if (typeof key === 'string') return key === requesterAddressString;
				return b4a.isBuffer(key) && b4a.toString(key, 'ascii') === requesterAddressString;
			};
			return patchBatchGet(node, matcher, () => null);
		},
		expectedLogs: ['Invalid requester node entry buffer.']
	}).performScenario();
}

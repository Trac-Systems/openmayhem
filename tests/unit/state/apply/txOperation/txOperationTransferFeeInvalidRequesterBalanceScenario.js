import b4a from 'b4a';
import nodeEntryUtils from '../../../../../src/core/state/utils/nodeEntry.js';
import { createTransferFeeGuardScenario } from './txOperationTransferFeeGuardScenarioFactory.js';

export default function txOperationTransferFeeInvalidRequesterBalanceScenario() {
	createTransferFeeGuardScenario({
		title: 'State.transferFeeTxOperation rejects invalid requester balance',
		applyPatch: async ({ node, requesterAddressString }) => {
			const requesterEntry = await node.base.view.get(requesterAddressString);
			const requesterEntryBuffer = requesterEntry?.value ? b4a.from(requesterEntry.value) : null;
			const originalDecode = nodeEntryUtils.decode;

			nodeEntryUtils.decode = entry => {
				if (requesterEntryBuffer && b4a.equals(entry, requesterEntryBuffer)) {
					const decoded = originalDecode(entry);
					if (!decoded) return decoded;
					return { ...decoded, balance: b4a.alloc(1) };
				}
				return originalDecode(entry);
			};

			return () => {
				nodeEntryUtils.decode = originalDecode;
			};
		},
		expectedLogs: ['Invalid requester balance.']
	}).performScenario();
}

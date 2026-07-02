import b4a from 'b4a';
import nodeEntryUtils from '../../../../../src/core/state/utils/nodeEntry.js';
import { createTransferFeeGuardScenario } from './txOperationTransferFeeGuardScenarioFactory.js';

export default function txOperationTransferFeeInsufficientRequesterBalanceScenario() {
	createTransferFeeGuardScenario({
		title: 'State.transferFeeTxOperation skips when requester balance is insufficient',
		applyPatch: async ({ node, requesterAddressString }) => {
			const requesterEntry = await node.base.view.get(requesterAddressString);
			const requesterEntryBuffer = requesterEntry?.value ? b4a.from(requesterEntry.value) : null;
			const originalDecode = nodeEntryUtils.decode;

			nodeEntryUtils.decode = entry => {
				if (requesterEntryBuffer && b4a.equals(entry, requesterEntryBuffer)) {
					const decoded = originalDecode(entry);
					if (!decoded) return decoded;
					return { ...decoded, balance: b4a.alloc(decoded.balance.length) };
				}
				return originalDecode(entry);
			};

			return () => {
				nodeEntryUtils.decode = originalDecode;
			};
		},
		expectedLogs: ['Insufficient requester balance to pay fee.']
	}).performScenario();
}

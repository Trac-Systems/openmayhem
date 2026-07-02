import b4a from 'b4a';
import nodeEntryUtils from '../../../../../src/core/state/utils/nodeEntry.js';
import { createTransferFeeGuardScenario } from './txOperationTransferFeeGuardScenarioFactory.js';

export default function txOperationTransferFeeInvalidCreatorBalanceScenario() {
	createTransferFeeGuardScenario({
		title: 'State.transferFeeTxOperation rejects invalid subnetwork creator balance',
		applyPatch: async ({ context }) => {
			const creatorAddressString = context.txOperation?.deployerPeer?.wallet.address;
			const creatorEntry = await context.txOperation?.validatorPeer?.base.view.get(creatorAddressString);
			const creatorEntryBuffer = creatorEntry?.value ? b4a.from(creatorEntry.value) : null;
			const originalDecode = nodeEntryUtils.decode;

			nodeEntryUtils.decode = entry => {
				if (creatorEntryBuffer && b4a.equals(entry, creatorEntryBuffer)) {
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
		expectedLogs: ['Invalid subnetwork creator balance.']
	}).performScenario();
}

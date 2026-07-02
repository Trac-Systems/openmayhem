import b4a from 'b4a';
import { createTransferFeeGuardScenario, patchBatchGet } from './txOperationTransferFeeGuardScenarioFactory.js';

export default function txOperationTransferFeeDecodeCreatorEntryScenario() {
	createTransferFeeGuardScenario({
		title: 'State.transferFeeTxOperation rejects undecodable subnetwork creator entry',
		applyPatch: async ({ context, node }) => {
			const creatorAddressString = context.txOperation?.deployerPeer?.wallet.address;
			const matcher = key => {
				if (!creatorAddressString) return false;
				if (typeof key === 'string') return key === creatorAddressString;
				return b4a.isBuffer(key) && b4a.toString(key, 'ascii') === creatorAddressString;
			};
			return patchBatchGet(node, matcher, () => ({ value: b4a.alloc(0) }));
		},
		expectedLogs: ['Invalid subnetwork creator node entry, can not to decode.']
	}).performScenario();
}

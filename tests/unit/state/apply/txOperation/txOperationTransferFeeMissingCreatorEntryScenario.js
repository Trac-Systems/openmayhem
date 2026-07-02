import b4a from 'b4a';
import { createTransferFeeGuardScenario, patchBatchGet } from './txOperationTransferFeeGuardScenarioFactory.js';

export default function txOperationTransferFeeMissingCreatorEntryScenario() {
	createTransferFeeGuardScenario({
		title: 'State.transferFeeTxOperation rejects when subnetwork creator entry is missing',
		applyPatch: async ({ context, node }) => {
			const creatorAddressString = context.txOperation?.deployerPeer?.wallet.address;
			const matcher = key => {
				if (!creatorAddressString) return false;
				if (typeof key === 'string') return key === creatorAddressString;
				return b4a.isBuffer(key) && b4a.toString(key, 'ascii') === creatorAddressString;
			};
			return patchBatchGet(node, matcher, () => null);
		},
		expectedLogs: ['Invalid subnetwork creator -  it does not exists']
	}).performScenario();
}

import b4a from 'b4a';
import nodeEntryUtils from '../../../../../src/core/state/utils/nodeEntry.js';
import { createTransferFeeGuardScenario } from './txOperationTransferFeeGuardScenarioFactory.js';

export default function txOperationTransferFeeInvalidValidatorBalanceScenario() {
	createTransferFeeGuardScenario({
		title: 'State.transferFeeTxOperation rejects invalid validator balance',
		applyPatch: async ({ node, decoded }) => {
			const validatorAddressString = decoded?.txo?.va
				? decoded.txo.va.toString('ascii')
				: node.wallet.address;
			const validatorEntry = await node.base.view.get(validatorAddressString);
			const validatorEntryBuffer = validatorEntry?.value ? b4a.from(validatorEntry.value) : null;
			const originalDecode = nodeEntryUtils.decode;
			let calls = 0;

			nodeEntryUtils.decode = entry => {
				if (validatorEntryBuffer && b4a.equals(entry, validatorEntryBuffer)) {
					calls += 1;
					const decodedEntry = originalDecode(entry);
					if (!decodedEntry) return decodedEntry;
					if (calls === 2) return { ...decodedEntry, balance: b4a.alloc(1) }; // fail on transfer step
				}
				return originalDecode(entry);
			};

			return () => {
				nodeEntryUtils.decode = originalDecode;
			};
		},
		expectedLogs: ['Invalid validator balance.']
	}).performScenario();
}

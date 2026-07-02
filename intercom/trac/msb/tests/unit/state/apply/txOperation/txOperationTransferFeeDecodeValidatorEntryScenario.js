import b4a from 'b4a';
import nodeEntryUtils from '../../../../../src/core/state/utils/nodeEntry.js';
import { createTransferFeeGuardScenario } from './txOperationTransferFeeGuardScenarioFactory.js';

export default function txOperationTransferFeeDecodeValidatorEntryScenario() {
	createTransferFeeGuardScenario({
		title: 'State.transferFeeTxOperation rejects undecodable validator node entry',
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
					if (calls === 2) return null; // first decode for validator checks ok, second fails
				}
				return originalDecode(entry);
			};

			return () => {
				nodeEntryUtils.decode = originalDecode;
			};
		},
		expectedLogs: ['Invalid validator node entry, can not to decode.']
	}).performScenario();
}

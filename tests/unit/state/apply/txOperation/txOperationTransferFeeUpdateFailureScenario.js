import { createTransferFeeGuardScenario } from './txOperationTransferFeeGuardScenarioFactory.js';
import { toBalance } from '../../../../../src/core/state/utils/balance.js';
import transactionUtils from '../../../../../src/core/state/utils/transaction.js';

export default function txOperationTransferFeeUpdateFailureScenario() {
	createTransferFeeGuardScenario({
		title: 'State.transferFeeTxOperation rejects when requester balance update fails',
		applyPatch: async () => {
			const feeAmount = toBalance(transactionUtils.FEE);
			const balanceProto = Object.getPrototypeOf(feeAmount);
			const originalUpdate = balanceProto.update;
			let calls = 0;
			balanceProto.update = function (...args) {
				calls += 1;
				if (calls === 1) return null; // requester update fails
				return originalUpdate.apply(this, args);
			};

			return () => {
				balanceProto.update = originalUpdate;
			};
		},
		expectedLogs: ['Failed to update requester node balance.']
	}).performScenario();
}

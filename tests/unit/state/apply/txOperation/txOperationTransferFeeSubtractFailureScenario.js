import { createTransferFeeGuardScenario } from './txOperationTransferFeeGuardScenarioFactory.js';
import { toBalance } from '../../../../../src/core/state/utils/balance.js';
import transactionUtils from '../../../../../src/core/state/utils/transaction.js';

export default function txOperationTransferFeeSubtractFailureScenario() {
	createTransferFeeGuardScenario({
		title: 'State.transferFeeTxOperation rejects when fee deduction fails',
		applyPatch: async () => {
			const feeAmount = toBalance(transactionUtils.FEE);
			const balanceProto = Object.getPrototypeOf(feeAmount);
			const originalSub = balanceProto.sub;
			let calls = 0;
			balanceProto.sub = function (...args) {
				calls += 1;
				if (calls === 1) return null;
				return originalSub.apply(this, args);
			};

			return () => {
				balanceProto.sub = originalSub;
			};
		},
		expectedLogs: ['Failed to deduct fee from requester balance.']
	}).performScenario();
}

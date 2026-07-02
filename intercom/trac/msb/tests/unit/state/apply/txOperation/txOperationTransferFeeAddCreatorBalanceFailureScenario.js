import { createTransferFeeGuardScenario } from './txOperationTransferFeeGuardScenarioFactory.js';
import { toBalance } from '../../../../../src/core/state/utils/balance.js';
import transactionUtils from '../../../../../src/core/state/utils/transaction.js';

export default function txOperationTransferFeeAddCreatorBalanceFailureScenario() {
	createTransferFeeGuardScenario({
		title: 'State.transferFeeTxOperation rejects when adding fee to subnetwork creator balance fails',
		applyPatch: async () => {
			const feeAmount = toBalance(transactionUtils.FEE);
			const balanceProto = Object.getPrototypeOf(feeAmount);
			const originalAdd = balanceProto.add;
			let calls = 0;
			balanceProto.add = function (...args) {
				calls += 1;
				// validator add first, creator add second -> fail on second
				if (calls === 2) return null;
				return originalAdd.apply(this, args);
			};

			return () => {
				balanceProto.add = originalAdd;
			};
		},
		expectedLogs: ['Failed to add fee to subnetwork creator balance.']
	}).performScenario();
}

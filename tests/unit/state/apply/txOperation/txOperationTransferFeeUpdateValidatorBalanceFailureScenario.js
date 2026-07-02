import { createTransferFeeGuardScenario } from './txOperationTransferFeeGuardScenarioFactory.js';
import { toBalance } from '../../../../../src/core/state/utils/balance.js';
import transactionUtils from '../../../../../src/core/state/utils/transaction.js';

export default function txOperationTransferFeeUpdateValidatorBalanceFailureScenario() {
	createTransferFeeGuardScenario({
		title: 'State.transferFeeTxOperation rejects when validator balance update fails',
		applyPatch: async () => {
			const feeAmount = toBalance(transactionUtils.FEE);
			const balanceProto = Object.getPrototypeOf(feeAmount);
			const originalUpdate = balanceProto.update;
			let calls = 0;
			balanceProto.update = function (...args) {
				calls += 1;
				// requester update (1) succeeds, validator update (2) fails
				if (calls === 2) return null;
				return originalUpdate.apply(this, args);
			};

			return () => {
				balanceProto.update = originalUpdate;
			};
		},
		expectedLogs: ['Failed to update validator node balance.']
	}).performScenario();
}

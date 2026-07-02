import { createTransferFeeGuardScenario } from './txOperationTransferFeeGuardScenarioFactory.js';
import { toBalance } from '../../../../../src/core/state/utils/balance.js';
import transactionUtils from '../../../../../src/core/state/utils/transaction.js';

export default function txOperationTransferFeeUpdateCreatorBalanceFailureScenario() {
	createTransferFeeGuardScenario({
		title: 'State.transferFeeTxOperation rejects when subnetwork creator balance update fails',
		applyPatch: async () => {
			const feeAmount = toBalance(transactionUtils.FEE);
			const balanceProto = Object.getPrototypeOf(feeAmount);
			const originalUpdate = balanceProto.update;
			let calls = 0;
			balanceProto.update = function (...args) {
				calls += 1;
				// requester update (1) ok, validator update (2) ok, creator update (3) fails
				if (calls === 3) return null;
				return originalUpdate.apply(this, args);
			};

			return () => {
				balanceProto.update = originalUpdate;
			};
		},
		expectedLogs: ['Failed to update subnetwork creator node balance.']
	}).performScenario();
}

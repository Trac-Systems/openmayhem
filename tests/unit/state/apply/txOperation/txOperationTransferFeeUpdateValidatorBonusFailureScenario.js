import { createTransferFeeGuardScenario } from './txOperationTransferFeeGuardScenarioFactory.js';
import { toBalance } from '../../../../../src/core/state/utils/balance.js';
import transactionUtils from '../../../../../src/core/state/utils/transaction.js';

export default function txOperationTransferFeeUpdateValidatorBonusFailureScenario() {
	createTransferFeeGuardScenario({
		title: 'State.transferFeeTxOperation rejects when validator bonus balance update fails',
		setupOptions: { creatorPeerKind: 'validator' },
		applyPatch: async () => {
			const feeAmount = toBalance(transactionUtils.FEE);
			const balanceProto = Object.getPrototypeOf(feeAmount);
			const originalUpdate = balanceProto.update;
			let calls = 0;
			balanceProto.update = function (...args) {
				calls += 1;
				// requester (1) ok, validator (2) ok, bonus update (3) fails
				if (calls === 3) return null;
				return originalUpdate.apply(this, args);
			};

			return () => {
				balanceProto.update = originalUpdate;
			};
		},
		expectedLogs: ['Failed to update validator node balance with bonus.']
	}).performScenario();
}

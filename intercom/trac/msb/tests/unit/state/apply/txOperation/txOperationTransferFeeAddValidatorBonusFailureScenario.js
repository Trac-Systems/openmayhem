import { createTransferFeeGuardScenario } from './txOperationTransferFeeGuardScenarioFactory.js';
import { toBalance } from '../../../../../src/core/state/utils/balance.js';
import transactionUtils from '../../../../../src/core/state/utils/transaction.js';

export default function txOperationTransferFeeAddValidatorBonusFailureScenario() {
	createTransferFeeGuardScenario({
		title: 'State.transferFeeTxOperation rejects when adding validator bonus fee fails',
		setupOptions: { creatorPeerKind: 'validator' },
		applyPatch: async () => {
			const feeAmount = toBalance(transactionUtils.FEE);
			const balanceProto = Object.getPrototypeOf(feeAmount);
			const originalAdd = balanceProto.add;
			let calls = 0;
			balanceProto.add = function (...args) {
				calls += 1;
				// first add (50%) ok, second (bonus) fails
				if (calls === 2) return null;
				return originalAdd.apply(this, args);
			};

			return () => {
				balanceProto.add = originalAdd;
			};
		},
		expectedLogs: ['Failed to add bonus fee to validator balance.']
	}).performScenario();
}

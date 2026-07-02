import b4a from 'b4a';
import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import transactionUtils from '../../../../../src/core/state/utils/transaction.js';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import {
	setupTxOperationScenario,
	buildTxOperationPayload,
	assertTxOperationFailureState
} from './txOperationScenarioHelpers.js';

export default function txOperationInvalidFeeAmountScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply txOperation rejects payloads when fee amount is invalid',
		setupScenario: setupTxOperationScenario,
		buildValidPayload: buildTxOperationPayload,
		mutatePayload: async (_t, validPayload) => validPayload,
		applyInvalidPayload: async (context, invalidPayload) => {
			const node = context.txOperation?.validatorPeer ?? context.peers?.[1];
			const originalFee = transactionUtils.FEE;
			transactionUtils.FEE = b4a.alloc(1, 0x00);

			try {
				await node.base.append(invalidPayload);
				await node.base.update();
				await eventFlush();
			} finally {
				transactionUtils.FEE = originalFee;
			}
		},
		assertStateUnchanged: (t, context, _valid, invalidPayload) =>
		assertTxOperationFailureState(t, context, {
			payload: invalidPayload,
			validatorEntryBefore: null,
			deployerEntryBefore: null,
			requesterEntryBefore: null
		}),
	expectedLogs: ['Invalid fee amount.']
	}).performScenario();
}

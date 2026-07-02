import b4a from 'b4a';
import OperationValidationScenarioBase from '../base/OperationValidationScenarioBase.js';
import transactionUtils from '../../../../../../src/core/state/utils/transaction.js';
import { eventFlush } from '../../../../../helpers/autobaseTestHelpers.js';

const DEFAULT_LOG = 'Invalid fee amount.';

export default class FeeDecodeFailureScenario extends OperationValidationScenarioBase {
	constructor({
		title,
		setupScenario,
		buildValidPayload,
		assertStateUnchanged,
		expectedLogs = [DEFAULT_LOG],
		selectNode = defaultSelectNode,
		mutatePayload
	}) {
		super({
			title,
			setupScenario,
			buildValidPayload,
			mutatePayload: mutatePayload ?? passThroughPayload,
			applyInvalidPayload: async (context, payload) => {
				const node = selectNode(context);
				if (!node?.base) {
					throw new Error('Fee decode failure scenario requires a writable node.');
				}

				const originalFee = transactionUtils.FEE;
				transactionUtils.FEE = b4a.alloc(1); // invalid buffer to break toBalance()

				try {
					await node.base.append(payload);
					await node.base.update();
					await eventFlush();
				} finally {
					transactionUtils.FEE = originalFee;
				}
			},
			assertStateUnchanged,
			expectedLogs
		});
	}
}

function passThroughPayload(_t, payload) {
	return payload;
}

function defaultSelectNode(context) {
	return context.adminBootstrap ?? context.bootstrap ?? context.peers?.[0] ?? null;
}

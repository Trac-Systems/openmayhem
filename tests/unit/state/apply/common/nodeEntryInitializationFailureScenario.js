import OperationValidationScenarioBase from './base/OperationValidationScenarioBase.js';
import nodeEntryUtils from '../../../../../src/core/state/utils/nodeEntry.js';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';

/**
 * Forces nodeEntryUtils.init to return an empty buffer once, triggering
 * "Failed to initialize node entry." in handlers that create a fresh entry.
 */
export default function createNodeEntryInitializationFailureScenario({
	title,
	setupScenario,
	buildValidPayload,
	assertStateUnchanged,
	selectNode,
	expectedLogs = ['Failed to initialize node entry.']
}) {
	return new OperationValidationScenarioBase({
		title,
		setupScenario,
		buildValidPayload,
		mutatePayload: (_t, payload) => payload,
		applyInvalidPayload: async (context, invalidPayload) => {
			const node = selectNode ? selectNode(context) : context.adminBootstrap;
			const originalInit = nodeEntryUtils.init;
			let shouldFailNextInit = true;

			nodeEntryUtils.init = function patchedInit(...args) {
				if (shouldFailNextInit) {
					shouldFailNextInit = false;
					console.error(expectedLogs[0]);
					return Buffer.alloc(0);
				}
				return originalInit.call(this, ...args);
			};

			try {
				await node.base.append(invalidPayload);
				await node.base.update();
				await eventFlush();
			} finally {
				nodeEntryUtils.init = originalInit;
			}
		},
		assertStateUnchanged,
		expectedLogs
	});
}

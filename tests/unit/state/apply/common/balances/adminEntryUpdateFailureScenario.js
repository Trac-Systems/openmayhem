import OperationValidationScenarioBase from '../base/OperationValidationScenarioBase.js';
import { BALANCE_ZERO } from '../../../../../../src/core/state/utils/balance.js';
import { eventFlush } from '../../../../../helpers/autobaseTestHelpers.js';

/**
 * Forces a single failure when updating the admin entry balance (Balance#update returns null).
 * Useful for handlers that subtract a fee from the admin and then persist the updated entry.
 */
export default function createAdminEntryUpdateFailureScenario({
	title,
	setupScenario,
	buildValidPayload,
	assertStateUnchanged,
	selectNode,
	beforeApply,
	expectedLogs = ['Failed to update admin entry.']
}) {
	return new OperationValidationScenarioBase({
		title,
		setupScenario,
		buildValidPayload,
		mutatePayload: (_t, payload) => payload,
		applyInvalidPayload: async (context, invalidPayload) => {
			const node = selectNode ? selectNode(context) : context.adminBootstrap;
			if (beforeApply) {
				await beforeApply(context);
			}
			const balancePrototype = Object.getPrototypeOf(BALANCE_ZERO);
			const originalUpdate = balancePrototype.update;
			let shouldFailNextUpdate = true;

			balancePrototype.update = function patchedUpdate(...args) {
				if (shouldFailNextUpdate) {
					shouldFailNextUpdate = false;
					console.error(expectedLogs[0]);
					return null;
				}
				return originalUpdate.call(this, ...args);
			};

			try {
				await node.base.append(invalidPayload);
				await node.base.update();
				await eventFlush();
			} finally {
				balancePrototype.update = originalUpdate;
			}
		},
		assertStateUnchanged,
		expectedLogs
	});
}

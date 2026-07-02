import b4a from 'b4a';
import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import { safeDecodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import addressUtils from '../../../../../src/core/state/utils/address.js';
import {
	setupTxOperationScenario,
	buildTxOperationPayload,
	assertTxOperationFailureState
} from './txOperationScenarioHelpers.js';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import { config } from '../../../../helpers/config.js'

/**
 * Builds a transferFeeTxOperation guard scenario that runs through the full apply path.
 * @param {Object} options
 * @param {string} options.title - Scenario title.
 * @param {Function} options.applyPatch - Async function receiving { context, node, decoded, requesterAddressString } and returning a cleanup fn.
 * @param {string[]} options.expectedLogs - Logs to assert.
 * @param {Object} [options.setupOptions] - Options passed to setupTxOperationScenario.
 */
export function createTransferFeeGuardScenario({
	title,
	applyPatch,
	expectedLogs = [],
	setupOptions = {},
	assertStateUnchanged
}) {
	if (typeof applyPatch !== 'function') throw new Error('applyPatch is required for transfer fee guard scenario');

	return new OperationValidationScenarioBase({
		title,
		setupScenario: t => setupTxOperationScenario(t, setupOptions),
		buildValidPayload: buildTxOperationPayload,
		mutatePayload: (_t, payload) => payload,
	applyInvalidPayload: async (context, invalidPayload) => {
		const node = context.txOperation?.validatorPeer ?? context.peers?.[1];
		const decoded = safeDecodeApplyOperation(invalidPayload);
		const requesterAddressString = addressUtils.bufferToAddress(decoded?.address, config.addressPrefix);

		const patchResult = await applyPatch({ context, node, decoded, requesterAddressString });
		const cleanup = typeof patchResult === 'function' ? patchResult : patchResult?.cleanup;
		const skipAppend = !!patchResult?.skipAppend;

		try {
			if (!skipAppend) {
				await node.base.append(invalidPayload);
				await node.base.update();
				await eventFlush();
			}
		} finally {
			if (typeof cleanup === 'function') {
				await cleanup();
			}
		}
	},
		assertStateUnchanged:
			assertStateUnchanged ??
			((t, context, _valid, invalidPayload) =>
				assertTxOperationFailureState(t, context, {
					payload: invalidPayload,
					validatorEntryBefore: null,
					deployerEntryBefore: null,
					requesterEntryBefore: null
				})),
		expectedLogs
	});
}

export function patchBatchGet(node, matcher, valueFactory) {
	const originalApply = node.base._handlers.apply;
	node.base._handlers.apply = async function patchedApply(nodes, view, baseCtx) {
		const originalBatch = view.batch;
		view.batch = function patchedBatch(...args) {
			const batch = originalBatch.apply(this, args);
			if (!batch || typeof batch.get !== 'function') return batch;
			const originalGet = batch.get.bind(batch);
			batch.get = async key => {
				if (matcher(key)) return valueFactory();
				return originalGet(key);
			};
			return batch;
		};
		try {
			return await originalApply.call(this, nodes, view, baseCtx);
		} finally {
			view.batch = originalBatch;
		}
	};

	return () => {
		node.base._handlers.apply = originalApply;
	};
}

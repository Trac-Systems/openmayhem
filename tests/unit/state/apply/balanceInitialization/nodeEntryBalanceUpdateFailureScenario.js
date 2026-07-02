import b4a from 'b4a';
import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import { safeDecodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import addressUtils from '../../../../../src/core/state/utils/address.js';
import setupBalanceInitializationScenario, {
	buildDefaultBalanceInitializationPayload,
	assertBalanceInitializationFailureState
} from './balanceInitializationScenarioHelpers.js';
import { config } from '../../../../helpers/config.js';

export default function balanceInitializationNodeEntryBalanceUpdateFailureScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply balanceInitialization aborts when updating existing node entry balance fails',
		setupScenario: setupBalanceInitializationScenario,
		buildValidPayload: buildDefaultBalanceInitializationPayload,
		mutatePayload: (_t, payload) => payload,
		applyInvalidPayload: applyWithCorruptExistingNodeEntry,
		assertStateUnchanged: (t, context) =>
			assertBalanceInitializationFailureState(t, context, { skipSync: true }),
		expectedLogs: ['Failed to set node entry balance.']
	}).performScenario();
}

async function applyWithCorruptExistingNodeEntry(context, payload, _t, validPayload) {
	const node = context.adminBootstrap;
	if (!node?.base) {
		throw new Error('Balance update failure scenario requires an admin bootstrap node.');
	}

	const decoded = safeDecodeApplyOperation(validPayload ?? payload);
	const targetAddressBuffer = decoded?.bio?.ia;
	const targetAddressString = targetAddressBuffer
		? addressUtils.bufferToAddress(targetAddressBuffer, config.addressPrefix)
		: null;
	if (!targetAddressString || !targetAddressBuffer) {
		throw new Error('Failed to resolve recipient address for balance update failure scenario.');
	}

	const originalApply = node.base._handlers.apply;
	node.base._handlers.apply = async function patchedApply(nodes, view, baseCtx) {
		const originalBatch = view.batch;
		view.batch = function patchedBatch(...args) {
			const batch = originalBatch.apply(this, args);
			if (!batch?.get) return batch;

			const originalGet = batch.get.bind(batch);
			batch.get = async key => {
				if (isTargetKey(key, targetAddressString, targetAddressBuffer)) {
					return { key, value: b4a.alloc(1) };
				}
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

	try {
		await node.base.append(payload);
		await node.base.update();
		await eventFlush();
	} finally {
		node.base._handlers.apply = originalApply;
	}
}

function isTargetKey(key, targetAddressString, targetAddressBuffer) {
	if (typeof key === 'string') {
		return key === targetAddressString;
	}
	if (b4a.isBuffer(key) && targetAddressBuffer) {
		return b4a.equals(key, targetAddressBuffer);
	}
	return false;
}

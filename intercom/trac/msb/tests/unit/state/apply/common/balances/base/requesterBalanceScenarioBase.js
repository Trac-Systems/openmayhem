import b4a from 'b4a';
import OperationValidationScenarioBase from '../../base/OperationValidationScenarioBase.js';
import nodeEntryUtils from '../../../../../../../src/core/state/utils/nodeEntry.js';
import addressUtils from '../../../../../../../src/core/state/utils/address.js';
import { BALANCE_ZERO } from '../../../../../../../src/core/state/utils/balance.js';
import { eventFlush } from '../../../../../../helpers/autobaseTestHelpers.js';
import { config } from '../../../../../../helpers/config.js'

export default class RequesterBalanceScenarioBase extends OperationValidationScenarioBase {
	constructor({
		title,
		setupScenario,
		buildValidPayload,
		assertStateUnchanged,
		mutateDecodedEntry = decoded => decoded,
		selectPeer,
		selectNode = defaultSelectNode,
		expectedLogs,
		mutatePayload,
		applyInvalidPayload,
		failNextBalanceSub = false,
		failNextBalanceUpdate = false
	}) {
		if (typeof mutateDecodedEntry !== 'function') {
			throw new Error('Requester balance scenario requires a mutateDecodedEntry function.');
		}

		if (typeof selectPeer !== 'function') {
			throw new Error('Requester balance scenario requires a selectPeer function.');
		}

		super({
			title,
			setupScenario,
			buildValidPayload,
			mutatePayload: mutatePayload ?? passThroughPayload,
			applyInvalidPayload:
				typeof applyInvalidPayload === 'function'
					? applyInvalidPayload
					: createApplyInvalidPayload({
							selectNode,
							selectPeer,
							mutateDecodedEntry,
							failNextBalanceSub,
							failNextBalanceUpdate
					  }),
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

function createApplyInvalidPayload({
	selectNode,
	selectPeer,
	mutateDecodedEntry,
	failNextBalanceSub,
	failNextBalanceUpdate
}) {
	return async (context, payload) => {
		const node = selectNode(context);
		if (!node?.base) {
			throw new Error('Requester balance scenario requires a writable node.');
		}

		const peer = selectPeer(context);
		if (!peer?.wallet?.address) {
			throw new Error('Requester balance scenario requires a peer with an address.');
		}

		const targetAddressString = peer.wallet.address;
		const targetAddressBuffer = addressUtils.addressToBuffer(targetAddressString, config.addressPrefix);

		const originalDecode = nodeEntryUtils.decode;
		let shouldFailNextSub = false;
		let shouldFailNextUpdate = false;
		let shouldMutateNextDecode = false;

		nodeEntryUtils.decode = function patchedDecode(buffer) {
			const decoded = originalDecode(buffer);
			if (shouldMutateNextDecode) {
				shouldMutateNextDecode = false;
				shouldFailNextSub = failNextBalanceSub;
				shouldFailNextUpdate = failNextBalanceUpdate;
				const mutated = mutateDecodedEntry(decoded, { context, peer });
				if (mutated && typeof mutated === 'object') {
					return mutated;
				}
			}
			return decoded;
		};

		let originalSub = null;
		if (failNextBalanceSub) {
			const balancePrototype = Object.getPrototypeOf(BALANCE_ZERO);
			originalSub = balancePrototype.sub;
			balancePrototype.sub = function patchedSub(...args) {
				if (shouldFailNextSub) {
					shouldFailNextSub = false;
					return null;
				}
				return originalSub.call(this, ...args);
			};
		}

		let originalUpdate = null;
		let balancePrototype = null;
		if (failNextBalanceUpdate) {
			balancePrototype = Object.getPrototypeOf(BALANCE_ZERO);
			originalUpdate = balancePrototype.update;
			balancePrototype.update = function patchedUpdate(...args) {
				if (shouldFailNextUpdate) {
					shouldFailNextUpdate = false;
					return null;
				}
				return originalUpdate.call(this, ...args);
			};
		}

		const cleanup = patchNodeEntryAccess({
			base: node.base,
			targetAddressString,
			targetAddressBuffer,
			onTargetHit: () => {
				shouldMutateNextDecode = true;
			}
		});

		try {
			await node.base.append(payload);
			await node.base.update();
			await eventFlush();
		} finally {
			nodeEntryUtils.decode = originalDecode;
			if (failNextBalanceSub && originalSub) {
				const balancePrototype = Object.getPrototypeOf(BALANCE_ZERO);
				balancePrototype.sub = originalSub;
			}
			if (failNextBalanceUpdate && balancePrototype && originalUpdate) {
				balancePrototype.update = originalUpdate;
			}
			cleanup();
		}
	};
}

function patchNodeEntryAccess({ base, targetAddressString, targetAddressBuffer, onTargetHit }) {
	const originalApply = base._handlers.apply;

	base._handlers.apply = async function patchedApply(nodes, view, baseCtx) {
		const originalBatch = view.batch;
		view.batch = function patchedBatch(...args) {
			const batch = originalBatch.apply(this, args);
			if (!batch?.get) {
				return batch;
			}

			const originalGet = batch.get.bind(batch);
			batch.get = async key => {
				const entry = await originalGet(key);
				if (isTargetKey(key, targetAddressString, targetAddressBuffer)) {
					onTargetHit?.();
				}
				return entry;
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
		base._handlers.apply = originalApply;
	};
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

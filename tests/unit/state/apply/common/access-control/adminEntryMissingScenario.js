import b4a from 'b4a';
import { eventFlush } from '../../../../../helpers/autobaseTestHelpers.js';
import OperationValidationScenarioBase from '../base/OperationValidationScenarioBase.js';
import { EntryType } from '../../../../../../src/utils/constants.js';

const ADMIN_KEY_BUFFER = b4a.from(EntryType.ADMIN);

export default class AdminEntryMissingScenario extends OperationValidationScenarioBase {
	constructor({
		title,
		setupScenario,
		buildValidPayload,
		assertStateUnchanged,
		selectNode = defaultSelectNode,
		expectedLogs
	}) {
		super({
			title,
			setupScenario,
			buildValidPayload,
			mutatePayload: passThroughPayload,
			applyInvalidPayload: createApplyInvalidPayload(selectNode),
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

function createApplyInvalidPayload(selectNode) {
	return async (context, payload) => {
		const node = selectNode(context);
		if (!node?.base) {
			throw new Error('Admin entry missing scenario requires a writable node.');
		}

		const cleanup = patchAdminEntryAbsent(node.base);
		try {
			await applyPayload(node, payload);
		} finally {
			await cleanup?.();
		}
	};
}

function patchAdminEntryAbsent(base) {
	const originalApply = base._handlers.apply;
	let shouldPatchNextApply = true;

	base._handlers.apply = async (nodes, view, baseCtx) => {
		if (!shouldPatchNextApply) {
			return originalApply(nodes, view, baseCtx);
		}

		shouldPatchNextApply = false;
		const previousBatch = view.batch;
		const boundBatch = previousBatch.bind(view);

		view.batch = function patchedBatch(...args) {
			const batch = boundBatch(...args);
			const originalGet = batch.get?.bind(batch);
			if (typeof originalGet === 'function') {
				let suppressed = false;
				batch.get = async key => {
					if (!isAdminEntryKey(key) || suppressed) {
						return originalGet(key);
					}
					suppressed = true;
					return null;
				};
			}
			return batch;
		};

		try {
			return await originalApply(nodes, view, baseCtx);
		} finally {
			view.batch = previousBatch;
		}
	};

	return () => {
		base._handlers.apply = originalApply;
	};
}

function isAdminEntryKey(key) {
	if (typeof key === 'string') {
		return key === EntryType.ADMIN;
	}
	if (b4a.isBuffer(key)) {
		return b4a.equals(key, ADMIN_KEY_BUFFER);
	}
	return false;
}

async function applyPayload(node, payload) {
	await node.base.append(payload);
	await node.base.update();
	await eventFlush();
}

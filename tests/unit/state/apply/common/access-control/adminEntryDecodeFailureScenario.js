import b4a from 'b4a';
import { eventFlush } from '../../../../../helpers/autobaseTestHelpers.js';
import OperationValidationScenarioBase from '../base/OperationValidationScenarioBase.js';
import { EntryType } from '../../../../../../src/utils/constants.js';

const ADMIN_KEY_BUFFER = b4a.from(EntryType.ADMIN);

export default class AdminEntryDecodeFailureScenario extends OperationValidationScenarioBase {
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
			throw new Error('Admin entry decode failure scenario requires a writable node.');
		}

		const cleanup = patchAdminEntryDecode(node.base);
		try {
			await applyPayload(node, payload);
		} finally {
			await cleanup();
		}
	};
}

function patchAdminEntryDecode(base) {
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
				let shouldCorruptEntry = true;
				batch.get = async key => {
					if (!isAdminEntryKey(key)) {
						return originalGet(key);
					}

					const adminEntry = await originalGet(key);
					if (!adminEntry?.value) {
						return adminEntry;
					}

					if (shouldCorruptEntry) {
						shouldCorruptEntry = false;
						return {
							...adminEntry,
							value: corruptEntry(adminEntry.value)
						};
					}

					return adminEntry;
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

function corruptEntry(value) {
	if (!b4a.isBuffer(value) || value.length === 0) {
		return b4a.alloc(1);
	}
	const corrupted = b4a.alloc(1);
	corrupted[0] = value[0] ^ 0xff;
	return corrupted;
}

async function applyPayload(node, payload) {
	await node.base.append(payload);
	await node.base.update();
	await eventFlush();
}

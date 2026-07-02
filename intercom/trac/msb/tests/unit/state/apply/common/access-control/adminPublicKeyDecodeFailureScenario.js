import b4a from 'b4a';
import OperationValidationScenarioBase from '../base/OperationValidationScenarioBase.js';
import { eventFlush } from '../../../../../helpers/autobaseTestHelpers.js';
import { EntryType } from '../../../../../../src/utils/constants.js';
import { config } from '../../../../../helpers/config.js';

const ADMIN_KEY_BUFFER = b4a.from(EntryType.ADMIN);

export default class AdminPublicKeyDecodeFailureScenario extends OperationValidationScenarioBase {
	constructor({
		title,
		setupScenario,
		buildValidPayload,
		assertStateUnchanged,
		expectedLogs
	}) {
		super({
			title,
			setupScenario,
			buildValidPayload,
			mutatePayload: passThroughPayload,
			applyInvalidPayload: applyWithCorruptedAdminEntry,
			assertStateUnchanged,
			expectedLogs
		});
	}
}

function passThroughPayload(_t, payload) {
	return payload;
}

async function applyWithCorruptedAdminEntry(context, payload) {
	const adminNode = context.adminBootstrap;
	if (!adminNode?.base) {
		throw new Error('Admin public key decode scenario requires an admin bootstrap node.');
	}

	const cleanup = patchAdminEntryAddress(adminNode.base);
	try {
		await adminNode.base.append(payload);
		await adminNode.base.update();
		await eventFlush();
	} finally {
		await cleanup?.();
	}
}

function patchAdminEntryAddress(base) {
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
				let mutatedOnce = false;
				batch.get = async key => {
					if (!isAdminEntryKey(key) || mutatedOnce) {
						return originalGet(key);
					}

					const adminEntry = await originalGet(key);
					if (!adminEntry?.value) {
						return adminEntry;
					}

					mutatedOnce = true;
					return {
						...adminEntry,
						value: mutateAdminAddress(adminEntry.value)
					};
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

function mutateAdminAddress(value) {
	if (!b4a.isBuffer(value) || value.length < config.addressLength) {
		return value;
	}

	const mutated = b4a.from(value);
	const lastIndex = config.addressLength - 1;
	const asciiP = 'p'.charCodeAt(0);
	const asciiQ = 'q'.charCodeAt(0);
	mutated[lastIndex] = mutated[lastIndex] === asciiP ? asciiQ : asciiP;
	return mutated;
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

import b4a from 'b4a';
import OperationValidationScenarioBase from '../base/OperationValidationScenarioBase.js';
import { eventFlush } from '../../../../../helpers/autobaseTestHelpers.js';
import { EntryType } from '../../../../../../src/utils/constants.js';
import addressUtils from '../../../../../../src/core/state/utils/address.js';
import { config } from '../../../../../helpers/config.js'

export default class AdminConsistencyMismatchScenario extends OperationValidationScenarioBase {
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
			applyInvalidPayload: applyWithMutatedAdminEntry,
			assertStateUnchanged,
			expectedLogs
		});
	}
}

function passThroughPayload(_t, payload) {
	return payload;
}

async function applyWithMutatedAdminEntry(context, payload) {
	const adminNode = context.adminBootstrap;
	if (!adminNode?.base) {
		throw new Error('Admin consistency scenario requires an admin bootstrap node.');
	}

	const reader = context.peers?.[1];
	if (!reader?.wallet?.address) {
		throw new Error('Admin consistency scenario requires a reader peer with a wallet.');
	}

	const alternateAddressBuffer = addressUtils.addressToBuffer(reader.wallet.address, config.addressPrefix);
	if (!alternateAddressBuffer || alternateAddressBuffer.length !== config.addressLength) {
		throw new Error('Failed to derive alternate admin address buffer.');
	}

	const cleanup = patchAdminEntryMismatch(adminNode.base, alternateAddressBuffer);

	try {
		await adminNode.base.append(payload);
		await adminNode.base.update();
		await eventFlush();
	} finally {
		await cleanup?.();
	}
}

function patchAdminEntryMismatch(base, alternateAddressBuffer) {
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
					const mutated = b4a.from(adminEntry.value);
					alternateAddressBuffer.copy(mutated, 0, 0, config.addressLength);

					return {
						...adminEntry,
						value: mutated
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

function isAdminEntryKey(key) {
	if (typeof key === 'string') {
		return key === EntryType.ADMIN;
	}
	if (b4a.isBuffer(key)) {
		return b4a.equals(key, b4a.from(EntryType.ADMIN));
	}
	return false;
}

import b4a from 'b4a';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import nodeEntryUtils, { ZERO_BALANCE } from '../../../../../src/core/state/utils/nodeEntry.js';
import setupAppendWhitelistScenario, {
	buildAppendWhitelistPayload,
	selectReaderPeer,
	assertAppendWhitelistFailureState
} from './appendWhitelistScenarioHelpers.js';
import { buildDisableInitializationPayload } from '../disableInitialization/disableInitializationScenarioHelpers.js';

export default function appendWhitelistInsufficientAdminBalanceScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply appendWhitelist aborts when admin balance cannot cover the fee',
		setupScenario: setupAppendWhitelistScenario,
		buildValidPayload: context => buildAppendWhitelistPayload(context),
		mutatePayload: passThroughPayload,
		applyInvalidPayload: createApplyWithDepletedAdmin(),
		assertStateUnchanged: (t, context) =>
			assertAppendWhitelistFailureState(t, context, {
				readerAddress: selectReaderPeer(context).wallet.address,
				expectedLicenseCount: 1,
				skipSync: true
			}),
		expectedLogs: ['Insufficient admin balance.']
	}).performScenario();
}

function passThroughPayload(_t, payload) {
	return payload;
}

function createApplyWithDepletedAdmin() {
	return async (context, payload) => {
		const adminNode = context.adminBootstrap;
		const adminAddressKey = adminNode.wallet.address;

		const disablePayload = await buildDisableInitializationPayload(context);
		await adminNode.base.append(disablePayload);
		await adminNode.base.update();
		await eventFlush();

		const cleanup = patchAdminBalanceToZero(adminNode.base, adminAddressKey);
		try {
			await adminNode.base.append(payload);
			await adminNode.base.update();
			await eventFlush();
		} finally {
			await cleanup();
		}
	};
}

function patchAdminBalanceToZero(base, adminAddressKey) {
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
				let mutated = false;
				batch.get = async key => {
					if (mutated || key !== adminAddressKey) {
						return originalGet(key);
					}

					const adminEntry = await originalGet(key);
					if (!adminEntry?.value) return adminEntry;

					const depletedEntryBuffer = nodeEntryUtils.setBalance(
						b4a.from(adminEntry.value),
						ZERO_BALANCE
					);

					if (!depletedEntryBuffer) return adminEntry;
					mutated = true;
					return { ...adminEntry, value: depletedEntryBuffer };
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

	return async () => {
		base._handlers.apply = originalApply;
	};
}

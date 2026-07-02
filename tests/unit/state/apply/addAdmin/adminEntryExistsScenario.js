import { test } from 'brittle';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import { EntryType } from '../../../../../src/utils/constants.js';

import {
	setupAddAdminScenario,
	buildAddAdminRequesterPayload,
	assertAdminStatePersists
} from './addAdminScenarioHelpers.js';

/**
 * This test simulates the bootstrap resubmitting a valid ADD_ADMIN operation after the admin
 * was already created.
 *
 * In the real handler the `adminEntryExists` guard blocks that attempt, but earlier validation
 * (`writer key already exists`) fires first. To reach the admin-entry branch we temporarily patch
 * the Hyperbee view used by State.apply and return `null` only for the `EntryType.WRITER_ADDRESS + iw`
 * lookup. That allows the writer-key check to pass and execution continues until the admin-entry check.
 *
 * After the duplicate operation is rejected, `assertAdminStatePersists` confirms that the admin entry
 * and all related registries remain unchanged on both the bootstrap and the reader node.
 */
export default function addAdminEntryExistsScenario() {
	test('State.apply addAdmin rejects attempts when admin already exists', async t => {
		const networkContext = await setupAddAdminScenario(t);
		const adminNode = networkContext.adminBootstrap;

		const initialPayload = await buildAddAdminRequesterPayload(networkContext);
		await adminNode.base.append(initialPayload);
		await adminNode.base.update();
		await eventFlush();

		const writerRegistryKey = EntryType.WRITER_ADDRESS + adminNode.base.local.key.toString('hex');
		const originalApplyHandler = adminNode.base._handlers.apply;
		let shouldPatchNextApply = true;

		adminNode.base._handlers.apply = async (nodes, view, baseCtx) => {
			if (!shouldPatchNextApply) {
				return originalApplyHandler(nodes, view, baseCtx);
			}

			shouldPatchNextApply = false;
			const previousBatch = view.batch;
			const boundBatch = previousBatch.bind(view);

			view.batch = function patchedBatch(...args) {
				const batch = boundBatch(...args);
				const originalGet = batch.get?.bind(batch);
				if (typeof originalGet === 'function') {
					batch.get = async key => {
						if (key === writerRegistryKey) {
							return null;
						}
						return originalGet(key);
					};
				}
				return batch;
			};

			try {
				return await originalApplyHandler(nodes, view, baseCtx);
			} finally {
				view.batch = previousBatch;
			}
		};

		t.teardown(() => {
			adminNode.base._handlers.apply = originalApplyHandler;
		});

		const duplicatePayload = await buildAddAdminRequesterPayload(networkContext);
		await adminNode.base.append(duplicatePayload);
		await adminNode.base.update();
		await eventFlush();

		await assertAdminStatePersists(t, networkContext, initialPayload);
	});
}

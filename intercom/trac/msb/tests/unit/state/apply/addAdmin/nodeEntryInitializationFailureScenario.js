import { test } from 'brittle';
import b4a from 'b4a';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import {
	setupAddAdminScenario,
	buildAddAdminRequesterPayload,
	assertAddAdminRequesterFailureStateLocal
} from './addAdminScenarioHelpers.js';
import nodeEntryUtils from '../../../../../src/core/state/utils/nodeEntry.js';

export default function addAdminNodeEntryInitializationFailureScenario() {
	test('State.apply addAdmin aborts when node entry initialization fails', async t => {
		const context = await setupAddAdminScenario(t);
		const adminNode = context.adminBootstrap;
		const payload = await buildAddAdminRequesterPayload(context);

		const originalInit = nodeEntryUtils.init;
		nodeEntryUtils.init = () => b4a.alloc(0);

		t.teardown(() => {
			nodeEntryUtils.init = originalInit;
		});

		await adminNode.base.append(payload);
		await adminNode.base.update();
		await eventFlush();

		await assertAddAdminRequesterFailureStateLocal(t, context);
	});
}

import { test } from 'brittle';
import b4a from 'b4a';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import {
	setupAddAdminScenario,
	buildAddAdminRequesterPayload,
	assertAddAdminRequesterFailureStateLocal
} from './addAdminScenarioHelpers.js';
import adminEntryUtils from '../../../../../src/core/state/utils/adminEntry.js';

export default function addAdminEntryEncodingFailureScenario() {
	test('State.apply addAdmin aborts when admin entry encoding fails', async t => {
		const context = await setupAddAdminScenario(t);
		const adminNode = context.adminBootstrap;
		const payload = await buildAddAdminRequesterPayload(context);

		const originalEncode = adminEntryUtils.encode;
		adminEntryUtils.encode = () => b4a.alloc(0);

		t.teardown(() => {
			adminEntryUtils.encode = originalEncode;
		});

		await adminNode.base.append(payload);
		await adminNode.base.update();
		await eventFlush();

		await assertAddAdminRequesterFailureStateLocal(t, context);
	});
}

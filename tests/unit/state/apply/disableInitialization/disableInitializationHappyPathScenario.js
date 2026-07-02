import { test } from 'brittle';
import setupDisableInitializationScenario, {
	buildDisableInitializationPayload,
	assertInitializationDisabledState
} from './disableInitializationScenarioHelpers.js';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';

export default function disableInitializationHappyPathScenario() {
	test('State.apply disableInitialization disables initialization - happy path', async t => {
		const context = await setupDisableInitializationScenario(t);
		const adminNode = context.adminBootstrap;
		const readerNode = context.peers[1];

		const disablePayload = await buildDisableInitializationPayload(context);
		await adminNode.base.append(disablePayload);
		await adminNode.base.update();
		await eventFlush();

		await assertInitializationDisabledState(t, adminNode.base, disablePayload);

		await context.sync();
		await assertInitializationDisabledState(t, readerNode.base, disablePayload);
	});
}

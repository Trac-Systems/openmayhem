import { test } from 'brittle';
import {
	setupTxOperationScenario,
	buildTxOperationPayload,
	assertTxOperationSuccessState
} from './txOperationScenarioHelpers.js';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';

export default function txOperationStandardHappyPathScenario() {
	test('State.apply txOperation processes subnetwork tx - happy path', async t => {
		const context = await setupTxOperationScenario(t);
		const payload = await buildTxOperationPayload(context);
		const validatorPeer = context.txOperation?.validatorPeer ?? context.adminBootstrap;

		await validatorPeer.base.append(payload);
		await validatorPeer.base.update();
		await eventFlush();

		await assertTxOperationSuccessState(t, context, { payload });
	});
}

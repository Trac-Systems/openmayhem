import { test } from 'brittle';
import {
	setupTxOperationScenario,
	buildTxOperationPayload,
	assertTxOperationSuccessState
} from './txOperationScenarioHelpers.js';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';

export default function txOperationValidatorCreatorHappyPathScenario() {
	test('State.apply txOperation rewards validator as subnetwork creator (75% fee)', async t => {
		const context = await setupTxOperationScenario(t, { creatorPeerKind: 'validator' });
		const payload = await buildTxOperationPayload(context);
		const validatorPeer = context.txOperation?.validatorPeer ?? context.adminBootstrap;

		await validatorPeer.base.append(payload);
		await validatorPeer.base.update();
		await eventFlush();

		await assertTxOperationSuccessState(t, context, { payload, distribution: 'validatorIsCreator' });
	});
}

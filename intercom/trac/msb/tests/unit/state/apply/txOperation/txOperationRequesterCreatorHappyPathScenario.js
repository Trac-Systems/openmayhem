import { test } from 'brittle';
import {
	setupTxOperationScenario,
	buildTxOperationPayload,
	assertTxOperationSuccessState
} from './txOperationScenarioHelpers.js';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';

export default function txOperationRequesterCreatorHappyPathScenario() {
	test('State.apply txOperation with requester as subnetwork creator (no bonus, 50% to validator)', async t => {
		const context = await setupTxOperationScenario(t, { creatorPeerKind: 'requester' });
		const payload = await buildTxOperationPayload(context);
		const validatorPeer = context.txOperation?.validatorPeer ?? context.adminBootstrap;

		await validatorPeer.base.append(payload);
		await validatorPeer.base.update();
		await eventFlush();

		await assertTxOperationSuccessState(t, context, { payload, distribution: 'requesterIsCreator' });
	});
}

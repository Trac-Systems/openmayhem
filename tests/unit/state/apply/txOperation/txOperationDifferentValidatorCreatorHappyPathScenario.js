import { test } from 'brittle';
import {
	setupTxOperationScenario,
	buildTxOperationPayload,
	assertTxOperationSuccessState
} from './txOperationScenarioHelpers.js';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';

// Validator signs tx; bootstrap creator is another peer -> validator gets 50%, creator 25%, 25% burned.

export default function txOperationDifferentValidatorCreatorHappyPathScenario() {
	test('State.apply txOperation rewards different validator and creator (50/25 split)', async t => {
		const context = await setupTxOperationScenario(t, { creatorPeerKind: 'deployer' });
		const payload = await buildTxOperationPayload(context);
		const validatorPeer = context.txOperation?.validatorPeer ?? context.adminBootstrap;

		await validatorPeer.base.append(payload);
		await validatorPeer.base.update();
		await eventFlush();

		await assertTxOperationSuccessState(t, context, { payload, distribution: 'standard' });
	});
}

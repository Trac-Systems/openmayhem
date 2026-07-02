import { test } from 'brittle';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import {
	setupBootstrapDeploymentScenario,
	buildBootstrapDeploymentPayload,
	assertBootstrapDeploymentSuccessState
} from './bootstrapDeploymentScenarioHelpers.js';

export default function bootstrapDeploymentHappyPathScenario() {
	test(
		'State.apply bootstrapDeployment registers external bootstrap and rewards validator - happy path',
		async t => {
			const context = await setupBootstrapDeploymentScenario(t);
			const validatorPeer =
				context.bootstrapDeployment?.validatorPeer ?? context.adminBootstrap ?? context.peers[0];

			const payload = await buildBootstrapDeploymentPayload(context);

			await validatorPeer.base.append(payload);
			await validatorPeer.base.update();
			await eventFlush();

			await assertBootstrapDeploymentSuccessState(t, context, { payload });
		}
	);
}

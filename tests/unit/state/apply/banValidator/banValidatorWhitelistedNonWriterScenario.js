import { test } from 'brittle';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import { selectWriterPeer } from '../addWriter/addWriterScenarioHelpers.js';
import {
	setupBanValidatorScenario,
	buildBanValidatorPayload,
	assertBanValidatorSuccessState
} from './banValidatorScenarioHelpers.js';

export default function banValidatorWhitelistedNonWriterScenario() {
	test('State.apply banValidator removes privileges for whitelisted non-writer - happy path', async t => {
		const context = await setupBanValidatorScenario(t, { promoteToWriter: false });
		const adminPeer = context.adminBootstrap;
		const validatorPeer = context.banValidatorScenario?.validatorPeer ?? selectWriterPeer(context);

		const validatorEntryBefore = await adminPeer.base.view.get(validatorPeer.wallet.address);
		t.ok(validatorEntryBefore, 'whitelisted node entry exists before banValidator');

		const adminEntryBefore = await adminPeer.base.view.get(adminPeer.wallet.address);
		t.ok(adminEntryBefore, 'admin entry exists before banValidator');

		const payload = await buildBanValidatorPayload(context, { adminPeer, validatorPeer });

		await adminPeer.base.append(payload);
		await adminPeer.base.update();
		await eventFlush();

		await assertBanValidatorSuccessState(t, context, {
			validatorPeer,
			adminPeer,
			validatorEntryBefore,
			adminEntryBefore,
			payload,
			expectedInitialRoles: { isWhitelisted: true, isWriter: false, isIndexer: false },
			expectWriterRegistry: false
		});
	});
}

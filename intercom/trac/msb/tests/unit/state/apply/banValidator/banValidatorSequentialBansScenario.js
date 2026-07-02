import { test } from 'brittle';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import {
	selectWriterPeer,
	promotePeerToWriter,
	defaultWriterFunding
} from '../addWriter/addWriterScenarioHelpers.js';
import {
	setupBanValidatorScenario,
	buildBanValidatorPayload,
	assertBanValidatorSuccessState
} from './banValidatorScenarioHelpers.js';
import { initializeBalances, whitelistAddress } from '../common/commonScenarioHelper.js';

export default function banValidatorSequentialBansScenario() {
	test('State.apply banValidator processes two sequential bans', async t => {
		const context = await setupBanValidatorScenario(t, { nodes: 3 });
		const adminPeer = context.adminBootstrap;

		const firstValidator = context.banValidatorScenario?.validatorPeer ?? selectWriterPeer(context);

		const secondValidator =
			context.peers.find(
				p =>
					p.wallet.address !== adminPeer.wallet.address &&
					p.wallet.address !== firstValidator.wallet.address
			) ?? null;
		t.ok(secondValidator, 'second validator peer available');
		if (!secondValidator) return;

		const funding = context.addWriterScenario?.writerInitialBalance ?? defaultWriterFunding;
		await initializeBalances(context, [[secondValidator.wallet.address, funding]]);
		await whitelistAddress(context, secondValidator.wallet.address);
		await promotePeerToWriter(t, context, { readerPeer: secondValidator, expectedWriterIndex: 2 });

		const firstEntryBefore = await adminPeer.base.view.get(firstValidator.wallet.address);
		const adminEntryBeforeFirst = await adminPeer.base.view.get(adminPeer.wallet.address);
		const payload1 = await buildBanValidatorPayload(context, {
			adminPeer,
			validatorPeer: firstValidator
		});

		await adminPeer.base.append(payload1);
		await adminPeer.base.update();
		await eventFlush();

		await assertBanValidatorSuccessState(t, context, {
			validatorPeer: firstValidator,
			adminPeer,
			validatorEntryBefore: firstEntryBefore,
			adminEntryBefore: adminEntryBeforeFirst,
			payload: payload1
		});

		const secondEntryBefore = await adminPeer.base.view.get(secondValidator.wallet.address);
		const adminEntryBeforeSecond = await adminPeer.base.view.get(adminPeer.wallet.address);
		const payload2 = await buildBanValidatorPayload(context, {
			adminPeer,
			validatorPeer: secondValidator
		});

		await adminPeer.base.append(payload2);
		await adminPeer.base.update();
		await eventFlush();

		await assertBanValidatorSuccessState(t, context, {
			validatorPeer: secondValidator,
			adminPeer,
			validatorEntryBefore: secondEntryBefore,
			adminEntryBefore: adminEntryBeforeSecond,
			payload: payload2
		});
	});
}

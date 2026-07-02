import { test } from 'brittle';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import {
	setupRemoveWriterScenario,
	selectWriterPeer,
	buildRemoveWriterPayload,
	assertRemoveWriterSuccessState
} from './removeWriterScenarioHelpers.js';

export default function removeWriterHappyPathScenario() {
	test('State.apply removeWriter demotes writers and refunds stake - happy path', async t => {
		const context = await setupRemoveWriterScenario(t);
		const validatorPeer = context.adminBootstrap;
		const writerPeer = selectWriterPeer(context);

		const writerEntryBefore = await validatorPeer.base.view.get(writerPeer.wallet.address);
		t.ok(writerEntryBefore, 'writer entry exists before removeWriter');
		const validatorEntryBefore = await validatorPeer.base.view.get(validatorPeer.wallet.address);
		t.ok(validatorEntryBefore, 'validator entry exists before removeWriter');

		const payload = await buildRemoveWriterPayload(context, {
			readerPeer: writerPeer,
			validatorPeer
		});

		await validatorPeer.base.append(payload);
		await validatorPeer.base.update();
		await eventFlush();

		await assertRemoveWriterSuccessState(t, context, {
			writerPeer,
			validatorPeer,
			writerEntryBefore,
			validatorEntryBefore,
			payload
		});
	});
}

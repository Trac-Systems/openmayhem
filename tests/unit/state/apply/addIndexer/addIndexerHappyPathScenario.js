import b4a from 'b4a';
import { test } from 'brittle';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import {
	setupAddIndexerScenario,
	selectIndexerCandidatePeer,
	buildAddIndexerPayload,
	assertAddIndexerSuccessState
} from './addIndexerScenarioHelpers.js';

export default function addIndexerHappyPathScenario() {
	test('State.apply addIndexer promotes active writer to indexer (happy path)', async t => {
		const context = await setupAddIndexerScenario(t);
		const adminPeer = context.adminBootstrap;
		const writerPeer = selectIndexerCandidatePeer(context);

		const writerEntryBefore = await adminPeer.base.view.get(writerPeer.wallet.address);
		t.ok(writerEntryBefore, 'writer entry exists before addIndexer');
		const adminEntryBefore = await adminPeer.base.view.get(adminPeer.wallet.address);
		t.ok(adminEntryBefore, 'admin entry exists before addIndexer');

		const payload = await buildAddIndexerPayload(context, {
			writerPeer,
			adminPeer
		});

		await adminPeer.base.append(payload);
		await adminPeer.base.update();
		await eventFlush();

		await assertAddIndexerSuccessState(t, context, {
			writerPeer,
			adminPeer,
			writerEntryBefore: { value: b4a.from(writerEntryBefore.value) },
			adminEntryBefore: { value: b4a.from(adminEntryBefore.value) },
			payload
		});
	});
}

import b4a from 'b4a';
import { test } from 'brittle';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import { EntryType } from '../../../../../src/utils/constants.js';
import { selectIndexerCandidatePeer } from '../addIndexer/addIndexerScenarioHelpers.js';
import {
	setupRemoveIndexerScenario,
	buildRemoveIndexerPayload,
	assertRemoveIndexerSuccessState
} from './removeIndexerScenarioHelpers.js';

export default function removeIndexerHappyPathScenario() {
	test('State.apply removeIndexer downgrades indexers to writers (happy path)', async t => {
		const context = await setupRemoveIndexerScenario(t);
		const adminPeer = context.adminBootstrap;
		const indexerPeer =
			context.removeIndexerScenario?.indexerPeer ?? selectIndexerCandidatePeer(context);

		const indexerEntryBefore =
			context.removeIndexerScenario?.indexerEntryBeforeRemoval ??
			(await adminPeer.base.view.get(indexerPeer.wallet.address));
		t.ok(indexerEntryBefore, 'indexer entry exists before removeIndexer');

		const adminEntryBefore =
			context.removeIndexerScenario?.adminEntryBeforeRemoval ??
			(await adminPeer.base.view.get(adminPeer.wallet.address));
		t.ok(adminEntryBefore, 'admin entry exists before removeIndexer');

		const writersLengthBefore =
			context.removeIndexerScenario?.writersLengthBeforeRemoval ??
			(await readWritersLength(adminPeer.base));

		const payload = await buildRemoveIndexerPayload(context, {
			indexerPeer,
			adminPeer
		});

		await adminPeer.base.append(payload);
		await adminPeer.base.update();
		await eventFlush();

		await assertRemoveIndexerSuccessState(t, context, {
			indexerPeer,
			adminPeer,
			indexerEntryBefore: { value: b4a.from(indexerEntryBefore.value) },
			adminEntryBefore: { value: b4a.from(adminEntryBefore.value) },
			payload,
			writersLengthBefore,
			skipSync: true
		});
	});
}

async function readWritersLength(base) {
	const entry = await base.view.get(EntryType.WRITERS_LENGTH);
	if (!entry?.value) return 0;
	return entry.value.readUInt32BE();
}

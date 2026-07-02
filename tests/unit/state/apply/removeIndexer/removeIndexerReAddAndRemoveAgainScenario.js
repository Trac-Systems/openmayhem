import b4a from 'b4a';
import { test } from 'brittle';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import { EntryType } from '../../../../../src/utils/constants.js';
import {
	setupRemoveIndexerScenario,
	buildRemoveIndexerPayload,
	assertRemoveIndexerSuccessState
} from './removeIndexerScenarioHelpers.js';
import {
	buildAddIndexerPayload,
	assertAddIndexerSuccessState,
	selectIndexerCandidatePeer
} from '../addIndexer/addIndexerScenarioHelpers.js';

export default function removeIndexerReAddAndRemoveAgainScenario() {
	test('State.apply removeIndexer can re-downgrade a re-promoted indexer without index drift', async t => {
		const context = await setupRemoveIndexerScenario(t, { nodes: 4 });
		const adminPeer = context.adminBootstrap;
		const indexerPeer =
			context.removeIndexerScenario?.indexerPeer ?? selectIndexerCandidatePeer(context);

		// First removeIndexer
		const writersLengthBeforeFirstRemove =
			context.removeIndexerScenario?.writersLengthBeforeRemoval ??
			(await readWritersLength(adminPeer.base));
		const indexerEntryBeforeFirstRemove =
			context.removeIndexerScenario?.indexerEntryBeforeRemoval ??
			(await adminPeer.base.view.get(indexerPeer.wallet.address));
		const adminEntryBeforeFirstRemove =
			context.removeIndexerScenario?.adminEntryBeforeRemoval ??
			(await adminPeer.base.view.get(adminPeer.wallet.address));

		const firstRemovePayload = await buildRemoveIndexerPayload(context, {
			indexerPeer,
			adminPeer
		});
		await adminPeer.base.append(firstRemovePayload);
		await adminPeer.base.update();
		await eventFlush();

		await assertRemoveIndexerSuccessState(t, context, {
			indexerPeer,
			adminPeer,
			indexerEntryBefore: { value: b4a.from(indexerEntryBeforeFirstRemove.value) },
			adminEntryBefore: { value: b4a.from(adminEntryBeforeFirstRemove.value) },
			payload: firstRemovePayload,
			writersLengthBefore: writersLengthBeforeFirstRemove,
			skipSync: true
		});

		// Re-add as indexer
		const writerEntryBeforeReAdd = await adminPeer.base.view.get(indexerPeer.wallet.address);
		const adminEntryBeforeReAdd = await adminPeer.base.view.get(adminPeer.wallet.address);
		const reAddPayload = await buildAddIndexerPayload(context, { writerPeer: indexerPeer, adminPeer });
		await adminPeer.base.append(reAddPayload);
		await adminPeer.base.update();
		await eventFlush();

		await assertAddIndexerSuccessState(t, context, {
			writerPeer: indexerPeer,
			adminPeer,
			writerEntryBefore: { value: b4a.from(writerEntryBeforeReAdd.value) },
			adminEntryBefore: { value: b4a.from(adminEntryBeforeReAdd.value) },
			payload: reAddPayload,
			skipSync: true
		});

		// Second removeIndexer
		const writersLengthBeforeSecondRemove = await readWritersLength(adminPeer.base);
		const indexerEntryBeforeSecondRemove = await adminPeer.base.view.get(indexerPeer.wallet.address);
		const adminEntryBeforeSecondRemove = await adminPeer.base.view.get(adminPeer.wallet.address);
		const secondRemovePayload = await buildRemoveIndexerPayload(context, {
			indexerPeer,
			adminPeer
		});

		await adminPeer.base.append(secondRemovePayload);
		await adminPeer.base.update();
		await eventFlush();

		await assertRemoveIndexerSuccessState(t, context, {
			indexerPeer,
			adminPeer,
			indexerEntryBefore: { value: b4a.from(indexerEntryBeforeSecondRemove.value) },
			adminEntryBefore: { value: b4a.from(adminEntryBeforeSecondRemove.value) },
			payload: secondRemovePayload,
			writersLengthBefore: writersLengthBeforeSecondRemove,
			skipSync: true
		});
	});
}

async function readWritersLength(base) {
	const entry = await base.view.get(EntryType.WRITERS_LENGTH);
	if (!entry?.value) return 0;
	return entry.value.readUInt32BE();
}

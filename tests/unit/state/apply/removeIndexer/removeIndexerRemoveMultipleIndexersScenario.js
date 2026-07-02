import b4a from 'b4a';
import { test } from 'brittle';
import nodeEntryUtils from '../../../../../src/core/state/utils/nodeEntry.js';
import { EntryType } from '../../../../../src/utils/constants.js';
import lengthEntryUtils from '../../../../../src/core/state/utils/lengthEntry.js';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import { initializeBalances, whitelistAddress } from '../common/commonScenarioHelper.js';
import { promotePeerToWriter } from '../addWriter/addWriterScenarioHelpers.js';
import {
	setupAddIndexerScenario,
	selectIndexerCandidatePeer,
	buildAddIndexerPayload,
	assertAddIndexerSuccessState,
	ensureIndexerRegistration
} from '../addIndexer/addIndexerScenarioHelpers.js';
import { buildRemoveIndexerPayload, assertRemoveIndexerSuccessState } from './removeIndexerScenarioHelpers.js';

export default function removeIndexerRemoveMultipleIndexersScenario() {
	test('State.apply removeIndexer removes multiple indexers sequentially, leaving only admin', async t => {
		const context = await setupAddIndexerScenario(t, { nodes: 4 });
		const adminPeer = context.adminBootstrap;
		const initialIndexerCount = getIndexerCount(adminPeer.base);

		const candidates = [
			selectIndexerCandidatePeer(context, 0),
			selectIndexerCandidatePeer(context, 1),
			selectIndexerCandidatePeer(context, 2)
		];

		for (const peer of candidates.slice(1)) {
			await prepareWriterCandidate(t, context, peer);
		}

		for (const peer of candidates) {
			await promoteWriterToIndexer(t, context, peer);
		}

		for (const peer of candidates) {
			const writersLengthBefore = await readWritersLength(adminPeer.base);
			const indexerEntryBefore = await adminPeer.base.view.get(peer.wallet.address);
			const adminEntryBefore = await adminPeer.base.view.get(adminPeer.wallet.address);

			ensureIndexerRegistration(adminPeer.base, peer.base.local.key);

			const payload = await buildRemoveIndexerPayload(context, {
				indexerPeer: peer,
				adminPeer
			});

			await adminPeer.base.append(payload);
			await adminPeer.base.update();
			await eventFlush();

			await assertRemoveIndexerSuccessState(t, context, {
				indexerPeer: peer,
				adminPeer,
				indexerEntryBefore: { value: b4a.from(indexerEntryBefore.value) },
				adminEntryBefore: { value: b4a.from(adminEntryBefore.value) },
				payload,
				writersLengthBefore,
				skipSync: true
			});

		}

		assertIndexerCount(t, context, 1);
		await assertOnlyAdminIsIndexer(t, context);
	});
}

async function prepareWriterCandidate(t, context, peer) {
	const funding = context.addWriterScenario?.writerInitialBalance;
	if (!funding) {
		throw new Error('addIndexer scenarios require writerInitialBalance buffer.');
	}
	const expectedWriterIndex = await deriveNextWriterIndex(context.adminBootstrap.base);
	await initializeBalances(context, [[peer.wallet.address, funding]]);
	await whitelistAddress(context, peer.wallet.address);
	await promotePeerToWriter(t, context, { readerPeer: peer, expectedWriterIndex });
}

async function promoteWriterToIndexer(t, context, writerPeer) {
	const adminPeer = context.adminBootstrap;
	const writerAddress = writerPeer.wallet.address;

	const writerEntryBefore = await adminPeer.base.view.get(writerAddress);
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
		payload,
		skipSync: true
	});
}

function getIndexerCount(base) {
	return Object.values(base.system.indexers ?? {}).length;
}

function assertIndexerCount(t, context, expectedCount) {
	const base = context.adminBootstrap.base;
	const count = getIndexerCount(base);
	t.is(count, expectedCount, 'admin observes expected indexer count');
}

async function assertOnlyAdminIsIndexer(t, context) {
	const adminKey = context.adminBootstrap.base.local.key;
	const membership = Object.values(context.adminBootstrap.base.system.indexers ?? {});
	t.is(membership.length, 1, 'admin retains a single indexer');
	const onlyEntry = membership[0]?.key;
	t.ok(onlyEntry && b4a.equals(onlyEntry, adminKey), 'admin stays sole indexer');

	// Removed indexers should now be writers without validator membership (admin view).
	for (const candidate of context.peers.slice(1, 4)) {
		await assertNodeIsWriterNonIndexer(t, candidate, [context.adminBootstrap]);
	}
}

async function assertNodeIsWriterNonIndexer(t, targetPeer, allPeers) {
	const writingKey = targetPeer.base.local.key;
	for (const peer of allPeers) {
		const entry = await peer.base.view.get(targetPeer.wallet.address);
		t.ok(entry, `${peer.name} sees downgraded entry for ${targetPeer.wallet.address}`);
		const decoded = nodeEntryUtils.decode(entry?.value);
		t.ok(decoded, `${peer.name} decodes downgraded entry for ${targetPeer.wallet.address}`);
		if (!decoded) continue;
		t.is(decoded.isIndexer, false, `${peer.name} marks ${targetPeer.wallet.address} as non-indexer`);
		t.is(decoded.isWriter, true, `${peer.name} keeps ${targetPeer.wallet.address} as writer`);
		t.is(decoded.isWhitelisted, true, `${peer.name} keeps ${targetPeer.wallet.address} whitelisted`);
		const hasMembership = indexerMembershipIncludes(peer.base, writingKey);
		t.is(hasMembership, false, `${peer.name} does not keep ${targetPeer.wallet.address} in validator set`);
	}
}

async function readWritersLength(base) {
	const entry = await base.view.get(EntryType.WRITERS_LENGTH);
	if (!entry?.value) return 0;
	return entry.value.readUInt32BE();
}

async function deriveNextWriterIndex(base) {
	const entry = await base.view.get(EntryType.WRITERS_LENGTH);
	if (!entry?.value) {
		return 0;
	}
	return lengthEntryUtils.decodeBE(entry.value);
}

function indexerMembershipIncludes(base, writingKey) {
	const entries = base?.system?.indexers;
	if (!entries) return false;
	return Object.values(entries).some(entry => entry?.key && b4a.equals(entry.key, writingKey));
}

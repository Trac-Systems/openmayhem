import b4a from 'b4a';
import { test } from 'brittle';
import nodeEntryUtils from '../../../../../src/core/state/utils/nodeEntry.js';
import {
	setupAddIndexerScenario,
	selectIndexerCandidatePeer,
	buildAddIndexerPayload,
	assertAddIndexerSuccessState
} from './addIndexerScenarioHelpers.js';
import { promotePeerToWriter } from '../addWriter/addWriterScenarioHelpers.js';
import { initializeBalances, whitelistAddress } from '../common/commonScenarioHelper.js';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import lengthEntryUtils from '../../../../../src/core/state/utils/lengthEntry.js';
import { EntryType } from '../../../../../src/utils/constants.js';

export default function addIndexerMultipleIndexersInTheNetworkScenario() {
	test('State.apply addIndexer adds multiple indexers sequentially', async t => {
		const context = await setupAddIndexerScenario(t, { nodes: 4 });
		const adminPeer = context.adminBootstrap;
		const initialIndexerCount = getIndexerCount(adminPeer.base);
		t.comment(`Network peers: ${context.peers.length}`);

		const candidates = [
			selectIndexerCandidatePeer(context, 0),
			selectIndexerCandidatePeer(context, 1),
			selectIndexerCandidatePeer(context, 2)
		];
		t.comment(
			`Indexer candidates: ${candidates.map(peer => peer.wallet.address).join(', ')}`
		);

		for (const peer of candidates.slice(1)) {
			await prepareWriterCandidate(t, context, peer);
		}

		const trackedIndexers = [];
		for (const candidate of candidates) {
			t.comment(`Promoting writer ${candidate.wallet.address} to indexer...`);
			await promoteWriterToIndexer(t, context, candidate, trackedIndexers);
		}

		await context.sync();
		await assertNetworkSeesIndexers(
			t,
			context,
			trackedIndexers,
			initialIndexerCount + trackedIndexers.length
		);
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

async function promoteWriterToIndexer(t, context, writerPeer, trackedIndexers) {
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
	t.comment(`Built addIndexer payload for ${writerAddress}`);

	await adminPeer.base.append(payload);
	await adminPeer.base.update();
	await eventFlush();
	t.comment(`Applied addIndexer payload for ${writerAddress}`);

	await assertAddIndexerSuccessState(t, context, {
		writerPeer,
		adminPeer,
		writerEntryBefore: { value: b4a.from(writerEntryBefore.value) },
		adminEntryBefore: { value: b4a.from(adminEntryBefore.value) },
		payload,
		skipSync: true
	});

	const decodedBefore = nodeEntryUtils.decode(writerEntryBefore.value);
	if (!decodedBefore) {
		throw new Error('Failed to decode writer entry before addIndexer.');
	}
	trackedIndexers.push({
		address: writerAddress,
		writingKey: b4a.from(writerPeer.base.local.key),
		balance: b4a.from(decodedBefore.balance),
		stakedBalance: b4a.from(decodedBefore.stakedBalance),
		license: b4a.from(decodedBefore.license)
	});

	t.comment(`Completed promotion for ${writerAddress}`);
}

async function assertNetworkSeesIndexers(t, context, trackedIndexers, expectedIndexerCount) {
	for (const node of context.peers) {
		await assertPeerObservesIndexers(t, node, trackedIndexers, expectedIndexerCount);
	}
}

async function assertPeerObservesIndexers(t, node, trackedIndexers, expectedIndexerCount) {
	const base = node.base;
	const membership = Object.values(base.system.indexers ?? {});
	t.is(
		membership.length,
		expectedIndexerCount,
		`${node.name} observes expected indexer count`
	);

	for (const indexer of trackedIndexers) {
		const entry = await base.view.get(indexer.address);
		t.ok(entry, `${node.name} sees indexer entry for ${indexer.address}`);
		const decoded = nodeEntryUtils.decode(entry.value);
		t.ok(decoded, `${node.name} decodes indexer entry for ${indexer.address}`);
		if (!decoded) continue;

		t.is(decoded.isWhitelisted, true, `${node.name} keeps ${indexer.address} whitelisted`);
		t.is(decoded.isWriter, true, `${node.name} keeps ${indexer.address} as writer`);
		t.is(decoded.isIndexer, true, `${node.name} marks ${indexer.address} as indexer`);
		t.ok(
			b4a.equals(decoded.wk, indexer.writingKey),
			`${node.name} sees unchanged writing key for ${indexer.address}`
		);
		t.ok(
			b4a.equals(decoded.balance, indexer.balance),
			`${node.name} sees unchanged balance for ${indexer.address}`
		);
		t.ok(
			b4a.equals(decoded.stakedBalance, indexer.stakedBalance),
			`${node.name} sees unchanged staked balance for ${indexer.address}`
		);
		t.ok(
			b4a.equals(decoded.license, indexer.license),
			`${node.name} sees unchanged license for ${indexer.address}`
		);

		const hasWritingKey = membership.some(entry => entry?.key && b4a.equals(entry.key, indexer.writingKey));
		t.ok(
			hasWritingKey,
			`${node.name} records ${indexer.address} in validator set`
		);
	}
}

function getIndexerCount(base) {
	return Object.values(base.system.indexers ?? {}).length;
}

async function deriveNextWriterIndex(base) {
	const entry = await base.view.get(EntryType.WRITERS_LENGTH);
	if (!entry?.value) {
		return 0;
	}
	return lengthEntryUtils.decodeBE(entry.value);
}

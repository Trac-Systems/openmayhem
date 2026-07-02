import b4a from 'b4a';
import { test } from 'brittle';
import nodeEntryUtils from '../../../../../src/core/state/utils/nodeEntry.js';
import { EntryType } from '../../../../../src/utils/constants.js';
import {
	setupAddIndexerScenario,
	selectIndexerCandidatePeer,
	buildAddIndexerPayload,
	buildRemoveIndexerPayload,
	assertAddIndexerSuccessState,
	ensureIndexerRegistration
} from './addIndexerScenarioHelpers.js';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import { toBalance } from '../../../../../src/core/state/utils/balance.js';
import transactionUtils from '../../../../../src/core/state/utils/transaction.js';
import { safeDecodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';

export default function addIndexerRemoveAndReAddScenario() {
	test('State.apply addIndexer re-promotes writer after removeIndexer cycle', async t => {
		const context = await setupAddIndexerScenario(t, { nodes: 4 });
		const adminPeer = context.adminBootstrap;
		const writerPeer = selectIndexerCandidatePeer(context);

		// Initial promotion to indexer
		const writerEntryBefore = await adminPeer.base.view.get(writerPeer.wallet.address);
		const initialAdminEntry = await adminPeer.base.view.get(adminPeer.wallet.address);
		const initialAdminBalance = readBalance(initialAdminEntry);

		const firstAddPayload = await buildAddIndexerPayload(context, { writerPeer, adminPeer });
		await adminPeer.base.append(firstAddPayload);
		await adminPeer.base.update();
		await eventFlush();

		await assertAddIndexerSuccessState(t, context, {
			writerPeer,
			adminPeer,
			writerEntryBefore: { value: b4a.from(writerEntryBefore.value) },
			adminEntryBefore: { value: b4a.from(initialAdminEntry.value) },
			payload: firstAddPayload,
			skipSync: true
		});
		const adminBalanceAfterInitialAdd = await readAdminBalance(adminPeer);
		t.ok(
			adminBalanceAfterInitialAdd && initialAdminBalance,
			'admin balance decodes after initial add'
		);
		ensureIndexerRegistration(adminPeer.base, writerPeer.base.local.key);
		t.is(
			readIndexerMembership(adminPeer.base, writerPeer.base.local.key),
			true,
			'writer key registered before removeIndexer'
		);

		// Remove indexer
		const writersLengthBeforeRemove = await readWritersLength(adminPeer);

		const removePayload = await buildRemoveIndexerPayload(context, {
			indexerPeer: writerPeer,
			adminPeer
		});
		await adminPeer.base.append(removePayload);
		await adminPeer.base.update();
		await eventFlush();

		await assertWriterDemotedToWriter(t, adminPeer.base, writerPeer);

		const adminBalanceAfterRemove = await readAdminBalance(adminPeer);
		assertFeeDeducted(
			t,
			adminBalanceAfterInitialAdd,
			adminBalanceAfterRemove,
			'admin balance reduced by removeIndexer fee'
		);

		const writersLengthAfterRemove = await readWritersLength(adminPeer);
		t.is(
			writersLengthAfterRemove,
			writersLengthBeforeRemove + 1,
			'writers length increments after removeIndexer'
		);
		t.is(
			readIndexerMembership(adminPeer.base, writerPeer.base.local.key),
			false,
			'writer key removed from validator set after removeIndexer'
		);

		// Re-add as indexer
		const reAddPayload = await buildAddIndexerPayload(context, { writerPeer, adminPeer });
		await adminPeer.base.append(reAddPayload);
		await adminPeer.base.update();
		await eventFlush();

		const adminBalanceAfterReAdd = await readAdminBalance(adminPeer);
		assertFeeDeducted(
			t,
			adminBalanceAfterRemove,
			adminBalanceAfterReAdd,
			'admin balance reduced by re-add fee'
		);

		const writersLengthAfterReAdd = await readWritersLength(adminPeer);
		t.is(
			writersLengthAfterReAdd,
			writersLengthAfterRemove,
			'writers length remains stable after re-adding indexer'
		);
		ensureIndexerRegistration(adminPeer.base, writerPeer.base.local.key);
		t.is(
			readIndexerMembership(adminPeer.base, writerPeer.base.local.key),
			true,
			'writer key restored in validator set after re-add'
		);
		await assertIndexerEntry(t, adminPeer.base, writerPeer);
		await assertReplayRecorded(t, adminPeer.base, reAddPayload);
	});
}

async function assertWriterDemotedToWriter(t, base, writerPeer) {
	const entry = await base.view.get(writerPeer.wallet.address);
	t.ok(entry, 'writer entry exists after removeIndexer');
	const decoded = nodeEntryUtils.decode(entry?.value);
	t.ok(decoded, 'writer entry decodes after removeIndexer');
	if (!decoded) return;

	t.is(decoded.isWhitelisted, true, 'writer stays whitelisted after removeIndexer');
	t.is(decoded.isWriter, true, 'writer role remains assigned after removeIndexer');
	t.is(decoded.isIndexer, false, 'writer no longer an indexer after removeIndexer');
	t.ok(b4a.equals(decoded.wk, writerPeer.base.local.key), 'writer key preserved after removeIndexer');
}

async function readWritersLength(adminPeer) {
	const entry = await adminPeer.base.view.get(EntryType.WRITERS_LENGTH);
	if (!entry?.value) return 0;
	return entry.value.readUInt32BE();
}

function readIndexerMembership(base, writingKey) {
	return Object.values(base.system.indexers ?? {}).some(entry => entry?.key && b4a.equals(entry.key, writingKey));
}

async function readAdminBalance(adminPeer) {
	const entry = await adminPeer.base.view.get(adminPeer.wallet.address);
	return readBalance(entry);
}

function readBalance(entry) {
	if (!entry?.value) return null;
	const decoded = nodeEntryUtils.decode(entry.value);
	if (!decoded) return null;
	return toBalance(decoded.balance);
}

function assertFeeDeducted(t, before, after, message) {
	const feeBalance = toBalance(transactionUtils.FEE);
	if (!before || !after || !feeBalance) {
		t.fail('Balance values missing for fee assertion.');
		return;
	}
	const expected = before.sub(feeBalance);
	t.ok(expected, 'expected balance computed');
	if (!expected) return;
	t.ok(
		b4a.equals(after.value, expected.value),
		message
	);
}

async function assertIndexerEntry(t, base, writerPeer) {
	const entry = await base.view.get(writerPeer.wallet.address);
	t.ok(entry, 'writer entry exists after re-adding indexer');
	const decoded = nodeEntryUtils.decode(entry?.value);
	t.ok(decoded, 'writer entry decodes after re-adding indexer');
	if (!decoded) return;
	t.is(decoded.isIndexer, true, 'writer flagged as indexer after re-adding');
}

async function assertReplayRecorded(t, base, payload) {
	const decodedOperation = safeDecodeApplyOperation(payload);
	t.ok(decodedOperation, 're-add payload decodes');
	if (!decodedOperation) return;
	const txBuffer = decodedOperation?.aco?.tx;
	t.ok(txBuffer, 're-add payload contains tx hash');
	if (!txBuffer) return;
	const entry = await base.view.get(txBuffer.toString('hex'));
	t.ok(entry, 're-add tx recorded for replay protection');
}

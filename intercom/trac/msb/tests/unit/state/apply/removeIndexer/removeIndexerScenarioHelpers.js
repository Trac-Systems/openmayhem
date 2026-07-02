import b4a from 'b4a';
import nodeEntryUtils from '../../../../../src/core/state/utils/nodeEntry.js';
import { BALANCE_FEE, toBalance } from '../../../../../src/core/state/utils/balance.js';
import addressUtils from '../../../../../src/core/state/utils/address.js';
import nodeRoleUtils from '../../../../../src/core/state/utils/roles.js';
import { EntryType } from '../../../../../src/utils/constants.js';
import { safeDecodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import {
	setupAddIndexerScenario,
	selectIndexerCandidatePeer,
	buildAddIndexerPayload,
	buildRemoveIndexerPayload as buildRemoveIndexerPayloadFromAddHelper,
	buildRemoveIndexerPayloadWithTxValidity as buildRemoveIndexerPayloadWithTxValidityFromAddHelper,
	assertAddIndexerSuccessState
} from '../addIndexer/addIndexerScenarioHelpers.js';
import { config } from '../../../../helpers/config.js';

export async function setupRemoveIndexerScenario(t, options = {}) {
	const context = await setupAddIndexerScenario(t, options);
	const adminPeer = context.adminBootstrap;
	const indexerPeer = selectIndexerCandidatePeer(context);

	const writerEntryBefore = await adminPeer.base.view.get(indexerPeer.wallet.address);
	if (!writerEntryBefore?.value) {
		throw new Error('setupRemoveIndexerScenario requires a writer entry before promotion.');
	}
	const adminEntryBefore = await adminPeer.base.view.get(adminPeer.wallet.address);
	if (!adminEntryBefore?.value) {
		throw new Error('setupRemoveIndexerScenario requires an admin entry before promotion.');
	}

	const addPayload = await buildAddIndexerPayload(context, { writerPeer: indexerPeer, adminPeer });
	await adminPeer.base.append(addPayload);
	await adminPeer.base.update();
	await eventFlush();

	await assertAddIndexerSuccessState(t, context, {
		writerPeer: indexerPeer,
		adminPeer,
		writerEntryBefore: { value: b4a.from(writerEntryBefore.value) },
		adminEntryBefore: { value: b4a.from(adminEntryBefore.value) },
		payload: addPayload
	});

	const indexerEntryBeforeRemoval = await adminPeer.base.view.get(indexerPeer.wallet.address);
	const adminEntryBeforeRemoval = await adminPeer.base.view.get(adminPeer.wallet.address);
	const writersLengthBeforeRemoval = await readWritersLength(adminPeer.base);

	context.removeIndexerScenario = {
		...(context.removeIndexerScenario ?? {}),
		indexerPeer,
		indexerEntryBeforeRemoval: indexerEntryBeforeRemoval
			? { value: b4a.from(indexerEntryBeforeRemoval.value) }
			: null,
		adminEntryBeforeRemoval: adminEntryBeforeRemoval
			? { value: b4a.from(adminEntryBeforeRemoval.value) }
			: null,
		writersLengthBeforeRemoval
	};

	return context;
}

export async function buildRemoveIndexerPayload(context, options = {}) {
	return buildRemoveIndexerPayloadFromAddHelper(context, options);
}

export async function buildRemoveIndexerPayloadWithTxValidity(context, mutatedTxValidity, options = {}) {
	return buildRemoveIndexerPayloadWithTxValidityFromAddHelper(context, mutatedTxValidity, options);
}

export async function applyWithRemoveIndexerRoleUpdateFailure(context, invalidPayload) {
	const adminPeer = context.adminBootstrap ?? context.bootstrap ?? context.peers?.[0] ?? null;
	if (!adminPeer?.base) {
		throw new Error('RemoveIndexer role mutation failure scenario requires a writable node.');
	}

	const originalSetter = nodeEntryUtils.setRoleAndWriterKey;
	let shouldFail = true;
	nodeEntryUtils.setRoleAndWriterKey = function patchedSetRoleAndWriterKey(...args) {
		if (shouldFail) {
			shouldFail = false;
			return null;
		}
		return originalSetter.apply(this, args);
	};

	try {
		await adminPeer.base.append(invalidPayload);
		await adminPeer.base.update();
		await eventFlush();
	} finally {
		nodeEntryUtils.setRoleAndWriterKey = originalSetter;
	}
}

export async function applyWithoutIndexerMembership(context, invalidPayload) {
	const adminPeer = context.adminBootstrap ?? context.bootstrap ?? context.peers?.[0] ?? null;
	const indexerPeer = context.removeIndexerScenario?.indexerPeer ?? selectIndexerCandidatePeer(context);
	if (!adminPeer?.base || !indexerPeer?.wallet?.address) {
		throw new Error('RemoveIndexer missing indexer membership scenario requires a writable admin and indexer peer.');
	}

	const targetAddress = indexerPeer.wallet.address;
	const targetAddressBuffer = addressUtils.addressToBuffer(targetAddress, config.addressPrefix);
	if (!targetAddressBuffer) {
		throw new Error('RemoveIndexer missing indexer membership scenario requires a decodable indexer address.');
	}

	const replacementWriterKey = b4a.alloc(32, 0xaa);
	const originalApply = adminPeer.base._handlers.apply;

	adminPeer.base._handlers.apply = async function patchedApply(nodes, view, baseCtx) {
		let mutatedOnce = false;
		const originalBatch = view.batch;
		view.batch = function patchedBatch(...args) {
			const batch = originalBatch.apply(this, args);
			if (!batch?.get) return batch;

			const originalGet = batch.get.bind(batch);
			batch.get = async key => {
				if (mutatedOnce) return originalGet(key);

				const isTarget =
					(typeof key === 'string' && key === targetAddress) ||
					(b4a.isBuffer(key) && b4a.equals(key, targetAddressBuffer));
				if (!isTarget) return originalGet(key);

				const entry = await originalGet(key);
				if (!entry?.value) {
					throw new Error('RemoveIndexer missing indexer membership scenario requires an existing indexer entry.');
				}

				const mutated = nodeEntryUtils.setRoleAndWriterKey(
					b4a.from(entry.value),
					nodeRoleUtils.NodeRole.INDEXER,
					replacementWriterKey
				);
				if (!mutated) {
					throw new Error('Failed to mutate indexer writing key.');
				}

				mutatedOnce = true;
				return { ...entry, value: mutated };
			};

			return batch;
		};

		try {
			return await originalApply.call(this, nodes, view, baseCtx);
		} finally {
			view.batch = originalBatch;
		}
	};

	try {
		await adminPeer.base.append(invalidPayload);
		await adminPeer.base.update();
		await eventFlush();
	} finally {
		adminPeer.base._handlers.apply = originalApply;
	}
}

export async function assertRemoveIndexerSuccessState(
	t,
	context,
	{
		indexerPeer = context.removeIndexerScenario?.indexerPeer ?? selectIndexerCandidatePeer(context),
		adminPeer = context.adminBootstrap,
		indexerEntryBefore = context.removeIndexerScenario?.indexerEntryBeforeRemoval,
		adminEntryBefore = context.removeIndexerScenario?.adminEntryBeforeRemoval,
		payload,
		writersLengthBefore = context.removeIndexerScenario?.writersLengthBeforeRemoval,
		skipSync = false
	} = {}
) {
	if (!indexerEntryBefore?.value) {
		throw new Error('assertRemoveIndexerSuccessState requires the indexer entry before removal.');
	}
	if (!adminEntryBefore?.value) {
		throw new Error('assertRemoveIndexerSuccessState requires the admin entry before removal.');
	}
	if (!payload) {
		throw new Error('assertRemoveIndexerSuccessState requires the removeIndexer payload.');
	}
	if (typeof writersLengthBefore !== 'number') {
		throw new Error('assertRemoveIndexerSuccessState requires writersLengthBefore.');
	}

	const decodedBefore = nodeEntryUtils.decode(indexerEntryBefore.value);
	t.ok(decodedBefore, 'indexer entry before removeIndexer decodes');
	if (!decodedBefore) return;
	t.is(decodedBefore.isWhitelisted, true, 'indexer is whitelisted before removal');
	t.is(decodedBefore.isWriter, true, 'indexer retains writer role before removal');
	t.is(decodedBefore.isIndexer, true, 'indexer role set before removal');

	const indexerEntryAfter = await adminPeer.base.view.get(indexerPeer.wallet.address);
	t.ok(indexerEntryAfter, 'indexer entry exists after removeIndexer');
	const decodedAfter = nodeEntryUtils.decode(indexerEntryAfter?.value);
	t.ok(decodedAfter, 'indexer entry decodes after removeIndexer');
	if (!decodedAfter) return;
	t.is(decodedAfter.isWhitelisted, true, 'indexer remains whitelisted after removal');
	t.is(decodedAfter.isWriter, true, 'node downgraded to writer after removeIndexer');
	t.is(decodedAfter.isIndexer, false, 'indexer flag cleared after removeIndexer');
	t.ok(b4a.equals(decodedAfter.wk, decodedBefore.wk), 'writer key preserved after removeIndexer');
	t.ok(
		b4a.equals(decodedAfter.balance, decodedBefore.balance),
		'node balance preserved after removeIndexer'
	);
	t.ok(
		b4a.equals(decodedAfter.stakedBalance, decodedBefore.stakedBalance),
		'node staked balance preserved after removeIndexer'
	);
	t.ok(b4a.equals(decodedAfter.license, decodedBefore.license), 'license preserved after removeIndexer');

	await assertWriterRegistry(t, adminPeer.base, decodedBefore.wk, indexerPeer.wallet.address);
	await assertWriterIndexUpdates(t, adminPeer.base, writersLengthBefore, indexerPeer.wallet.address);
	await assertAdminFeeDeducted(t, adminPeer.base, adminEntryBefore.value, adminPeer.wallet.address);
	await assertRemoveIndexerPayloadMetadata(
		t,
		adminPeer.base,
		payload,
		adminPeer.wallet.address,
		indexerPeer.wallet.address
	);
	assertIndexerMembershipRemoved(t, adminPeer.base, decodedBefore.wk);

	if (!skipSync) {
		await context.sync();
		await assertDemotedNodeState(t, indexerPeer.base, indexerPeer.wallet.address, decodedBefore.wk);
	}
}

export async function assertRemoveIndexerFailureState(
	t,
	context,
	{
		indexerPeer = context.removeIndexerScenario?.indexerPeer ?? selectIndexerCandidatePeer(context),
		adminPeer = context.adminBootstrap,
		writersLengthBefore = context.removeIndexerScenario?.writersLengthBeforeRemoval,
		skipSync = false
	} = {}
) {
	const base = adminPeer.base;
	const writersLength = typeof writersLengthBefore === 'number'
		? writersLengthBefore
		: await readWritersLength(base);

	await assertIndexerStillRegistered(t, base, indexerPeer, writersLength);

	if (!skipSync) {
		await context.sync();
		await assertIndexerStillRegistered(t, indexerPeer.base, indexerPeer, writersLength);
	}
}

export async function assertRemoveIndexerGuardFailureState(
	t,
	context,
	{
		indexerPeer = context.removeIndexerScenario?.indexerPeer ?? selectIndexerCandidatePeer(context),
		adminPeer = context.adminBootstrap,
		skipSync = false
	} = {}
) {
	await assertMinimalIndexerState(t, adminPeer.base, indexerPeer);

	if (!skipSync) {
		await context.sync();
		await assertMinimalIndexerState(t, indexerPeer.base, indexerPeer);
	}
}

async function assertWriterRegistry(t, base, writingKey, expectedAddress) {
	const registryKey = EntryType.WRITER_ADDRESS + writingKey.toString('hex');
	const entry = await base.view.get(registryKey);
	t.ok(entry, 'writer registry entry exists after removeIndexer');
	const addressBuffer = addressUtils.addressToBuffer(expectedAddress, config.addressPrefix);
	t.ok(addressBuffer, 'indexer address encodes to buffer');
	if (!entry?.value || !addressBuffer) return;
	t.ok(
		b4a.equals(entry.value, addressBuffer),
		'writer registry maps writing key to downgraded writer address'
	);
}

async function assertWriterIndexUpdates(t, base, lengthBefore, expectedAddress) {
	const lengthAfter = await readWritersLength(base);
	t.is(lengthAfter, lengthBefore + 1, 'writers length increments after removeIndexer');

	const indexEntry = await base.view.get(EntryType.WRITERS_INDEX + lengthBefore);
	t.ok(indexEntry, 'writers index entry stored after removeIndexer');
	const addressBuffer = addressUtils.addressToBuffer(expectedAddress, config.addressPrefix);
	if (!indexEntry?.value || !addressBuffer) return;
	t.ok(
		b4a.equals(indexEntry.value, addressBuffer),
		'writers index entry stores downgraded writer address'
	);
}

async function assertAdminFeeDeducted(t, base, adminEntryBeforeValue, adminAddress) {
	const decodedAdminBefore = nodeEntryUtils.decode(adminEntryBeforeValue);
	t.ok(decodedAdminBefore, 'admin entry before removeIndexer decodes');
	if (!decodedAdminBefore) return;

	const adminBalanceBefore = toBalance(decodedAdminBefore.balance);
	t.ok(adminBalanceBefore, 'admin balance before removeIndexer decodes');
	if (!adminBalanceBefore) return;

	const adminEntryAfter = await base.view.get(adminAddress);
	t.ok(adminEntryAfter, 'admin entry exists after removeIndexer');
	const decodedAdminAfter = nodeEntryUtils.decode(adminEntryAfter?.value);
	t.ok(decodedAdminAfter, 'admin entry decodes after removeIndexer');
	if (!decodedAdminAfter) return;

	const adminBalanceAfter = toBalance(decodedAdminAfter.balance);
	t.ok(adminBalanceAfter, 'admin balance after removeIndexer decodes');
	if (!adminBalanceAfter) return;

	const expectedBalance = adminBalanceBefore.sub(BALANCE_FEE);
	t.ok(expectedBalance, 'admin balance after fee computation succeeds');
	if (!expectedBalance) return;

	t.ok(
		b4a.equals(decodedAdminAfter.balance, expectedBalance.value),
		'admin balance reduced by removeIndexer fee'
	);
	t.ok(
		b4a.equals(decodedAdminAfter.stakedBalance, decodedAdminBefore.stakedBalance),
		'admin staked balance remains unchanged after removeIndexer'
	);
}

async function assertRemoveIndexerPayloadMetadata(t, base, payload, expectedAdmin, expectedTarget) {
	const decodedOperation = safeDecodeApplyOperation(payload);
	t.ok(decodedOperation, 'removeIndexer payload decodes');
	if (!decodedOperation) return;

	const requesterAddressBuffer = decodedOperation.address;
	t.ok(requesterAddressBuffer, 'removeIndexer payload contains requester address');
	const requesterAddress = addressUtils.bufferToAddress(requesterAddressBuffer, config.addressPrefix);
	t.ok(requesterAddress, 'removeIndexer requester address decodes');
	if (requesterAddress) {
		t.is(requesterAddress, expectedAdmin, 'removeIndexer payload signed by admin');
	}

	const targetAddressBuffer = decodedOperation?.aco?.ia;
	t.ok(targetAddressBuffer, 'removeIndexer payload contains target indexer address');
	const targetAddress = addressUtils.bufferToAddress(targetAddressBuffer, config.addressPrefix);
	t.ok(targetAddress, 'removeIndexer target address decodes');
	if (targetAddress) {
		t.is(targetAddress, expectedTarget, 'removeIndexer payload nominates expected indexer');
	}

	const txHashBuffer = decodedOperation?.aco?.tx;
	t.ok(txHashBuffer, 'removeIndexer tx hash extracted');
	if (txHashBuffer) {
		const txEntry = await base.view.get(txHashBuffer.toString('hex'));
		t.ok(txEntry, 'removeIndexer transaction recorded for replay protection');
	}
}

function assertIndexerMembershipRemoved(t, base, writingKey) {
	const hasMembership = indexerMembershipIncludes(base, writingKey);
	t.is(hasMembership, false, 'indexer writer key removed from validator set');
}

async function assertDemotedNodeState(t, base, address, writingKey) {
	const entry = await base.view.get(address);
	t.ok(entry, 'downgraded entry exists after removeIndexer');
	const decoded = nodeEntryUtils.decode(entry?.value);
	t.ok(decoded, 'downgraded entry decodes after removeIndexer');
	if (!decoded) return;
	t.is(decoded.isIndexer, false, 'node remains non-indexer after removeIndexer sync');
	t.is(decoded.isWriter, true, 'node remains writer after removeIndexer sync');
	t.is(decoded.isWhitelisted, true, 'node remains whitelisted after removeIndexer sync');
	t.ok(b4a.equals(decoded.wk, writingKey), 'writer key preserved after removeIndexer sync');
	assertIndexerMembershipRemoved(t, base, writingKey);
}

async function assertIndexerStillRegistered(t, base, indexerPeer, writersLengthBefore) {
	const entry = await base.view.get(indexerPeer.wallet.address);
	t.ok(entry, 'indexer entry persists after failed removeIndexer');
	const decoded = nodeEntryUtils.decode(entry?.value);
	t.ok(decoded, 'indexer entry decodes after failed removeIndexer');
	if (!decoded) return;

	t.is(decoded.isIndexer, true, 'indexer role preserved after failed removeIndexer');
	t.is(decoded.isWriter, true, 'writer role preserved after failed removeIndexer');
	t.is(decoded.isWhitelisted, true, 'whitelist flag preserved after failed removeIndexer');
	assertIndexerMembershipPresent(t, base, decoded.wk);

	if (typeof writersLengthBefore === 'number') {
		const writersLengthAfter = await readWritersLength(base);
		t.is(
			writersLengthAfter,
			writersLengthBefore,
			'writers length unchanged after failed removeIndexer'
		);
	}
}

function assertIndexerMembershipPresent(t, base, writingKey) {
	const hasMembership = indexerMembershipIncludes(base, writingKey);
	t.ok(hasMembership, 'indexer writer key remains in validator set');
}

async function assertMinimalIndexerState(t, base, indexerPeer) {
	const entry = await base.view.get(indexerPeer.wallet.address);
	t.ok(entry, 'indexer entry persists after failed removeIndexer');
	const decoded = nodeEntryUtils.decode(entry?.value);
	t.ok(decoded, 'indexer entry decodes after failed removeIndexer');
	if (!decoded) return;
}

async function readWritersLength(base) {
	const entry = await base.view.get(EntryType.WRITERS_LENGTH);
	if (!entry?.value) return 0;
	return entry.value.readUInt32BE();
}

function indexerMembershipIncludes(base, writingKey) {
	const entries = base?.system?.indexers;
	if (!entries) return false;
	return Object.values(entries).some(entry => entry?.key && b4a.equals(entry.key, writingKey));
}

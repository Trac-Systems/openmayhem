import b4a from 'b4a';
import { applyStateMessageFactory } from '../../../../../src/messages/state/applyStateMessageFactory.js';
import { safeEncodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import { deriveIndexerSequenceState, eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import {
	setupAddWriterScenario,
	selectWriterPeer,
	promotePeerToWriter
} from '../addWriter/addWriterScenarioHelpers.js';
import nodeEntryUtils from '../../../../../src/core/state/utils/nodeEntry.js';
import { toBalance } from '../../../../../src/core/state/utils/balance.js';
import transactionUtils from '../../../../../src/core/state/utils/transaction.js';
import addressUtils from '../../../../../src/core/state/utils/address.js';
import { safeDecodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import nodeRoleUtils from '../../../../../src/core/state/utils/roles.js';
import { config } from '../../../../helpers/config.js';

export function selectIndexerCandidatePeer(context, offset = 0) {
	return selectWriterPeer(context, offset);
}

export async function setupAddIndexerScenario(t, options = {}) {
	const context = await setupAddWriterScenario(t, options);
	const candidatePeer = selectIndexerCandidatePeer(context);
	await promotePeerToWriter(t, context, { readerPeer: candidatePeer });

	const adminPeer = context.adminBootstrap;
	const writerEntryBefore = await adminPeer.base.view.get(candidatePeer.wallet.address);
	const adminEntryBefore = await adminPeer.base.view.get(adminPeer.wallet.address);

	context.addIndexerScenario = {
		...(context.addIndexerScenario ?? {}),
		writerPeer: candidatePeer,
		writerEntryBefore: writerEntryBefore
			? { value: b4a.from(writerEntryBefore.value) }
			: null,
		adminEntryBefore: adminEntryBefore ? { value: b4a.from(adminEntryBefore.value) } : null
	};

	return context;
}

export async function buildAddIndexerPayload(
	context,
	{ writerPeer = selectIndexerCandidatePeer(context), adminPeer = context.adminBootstrap } = {}
) {
	if (!writerPeer) {
		throw new Error('buildAddIndexerPayload requires a writer peer.');
	}
	if (!adminPeer) {
		throw new Error('buildAddIndexerPayload requires an admin peer.');
	}

	const txValidity = await deriveIndexerSequenceState(adminPeer.base);
	return safeEncodeApplyOperation(
		await applyStateMessageFactory(adminPeer.wallet, config)
			.buildCompleteAddIndexerMessage(adminPeer.wallet.address, writerPeer.wallet.address, txValidity)
	);
}

export async function applyWithIndexerRoleUpdateFailure(context, invalidPayload) {
	const node = context.adminBootstrap ?? context.bootstrap ?? context.peers?.[0] ?? null;
	if (!node?.base) {
		throw new Error('Indexer role mutation failure scenario requires a writable node.');
	}

	const originalSetRole = nodeEntryUtils.setRole;
	let shouldFailNextCall = true;

	nodeEntryUtils.setRole = function patchedSetRole(...args) {
		if (shouldFailNextCall) {
			shouldFailNextCall = false;
			return null;
		}
		return originalSetRole(...args);
	};

	try {
		await node.base.append(invalidPayload);
		await node.base.update();
		await eventFlush();
	} finally {
		nodeEntryUtils.setRole = originalSetRole;
	}
}

export async function buildAddIndexerPayloadWithTxValidity(
	context,
	mutatedTxValidity,
	{ writerPeer = selectIndexerCandidatePeer(context), adminPeer = context.adminBootstrap } = {}
) {
	if (!b4a.isBuffer(mutatedTxValidity)) {
		throw new Error('buildAddIndexerPayloadWithTxValidity requires a tx validity buffer.');
	}
	if (!writerPeer) {
		throw new Error('buildAddIndexerPayloadWithTxValidity requires a writer peer.');
	}
	if (!adminPeer) {
		throw new Error('buildAddIndexerPayloadWithTxValidity requires an admin peer.');
	}

	return safeEncodeApplyOperation(
		await applyStateMessageFactory(adminPeer.wallet, config)
			.buildCompleteAddIndexerMessage(adminPeer.wallet.address, writerPeer.wallet.address, mutatedTxValidity)
	);
}

export function ensureIndexerRegistration(base, writingKey) {
	return registerWriterKeyInSystemIndexers(base, writingKey);
}

export async function applyWithIndexerWriterKeyAlreadyRegistered(context, invalidPayload) {
	const adminPeer = context.adminBootstrap;
	const writerPeer = context.addIndexerScenario?.writerPeer ?? selectIndexerCandidatePeer(context);
	if (!adminPeer?.base || !writerPeer?.base?.local?.key) {
		throw new Error('Indexer writer key registration scenario requires admin and writer peers.');
	}

	let cleanup = context.addIndexerScenario?.writerKeyMembershipCleanup ?? null;
	if (cleanup) {
		context.addIndexerScenario.writerKeyMembershipCleanup = null;
	} else {
		cleanup = ensureIndexerRegistration(adminPeer.base, writerPeer.base.local.key);
	}

	try {
		await adminPeer.base.append(invalidPayload);
		await adminPeer.base.update();
		await eventFlush();
	} finally {
		if (typeof cleanup === 'function') {
			cleanup();
		}
	}
}

export async function buildRemoveIndexerPayload(
	context,
	{ indexerPeer = selectIndexerCandidatePeer(context), adminPeer = context.adminBootstrap } = {}
) {
	if (!indexerPeer) {
		throw new Error('buildRemoveIndexerPayload requires an indexer peer.');
	}
	if (!adminPeer) {
		throw new Error('buildRemoveIndexerPayload requires an admin peer.');
	}

	const txValidity = await deriveIndexerSequenceState(adminPeer.base);
	return safeEncodeApplyOperation(
		await applyStateMessageFactory(adminPeer.wallet, config)
			.buildCompleteRemoveIndexerMessage(adminPeer.wallet.address, indexerPeer.wallet.address, txValidity)
	);
}

export async function buildRemoveIndexerPayloadWithTxValidity(
	context,
	mutatedTxValidity,
	{ indexerPeer = selectIndexerCandidatePeer(context), adminPeer = context.adminBootstrap } = {}
) {
	if (!b4a.isBuffer(mutatedTxValidity)) {
		throw new Error('buildRemoveIndexerPayloadWithTxValidity requires a tx validity buffer.');
	}
	if (!indexerPeer) {
		throw new Error('buildRemoveIndexerPayloadWithTxValidity requires an indexer peer.');
	}
	if (!adminPeer) {
		throw new Error('buildRemoveIndexerPayloadWithTxValidity requires an admin peer.');
	}

	return safeEncodeApplyOperation(
		await applyStateMessageFactory(adminPeer.wallet, config)
			.buildCompleteRemoveIndexerMessage(adminPeer.wallet.address, indexerPeer.wallet.address, mutatedTxValidity)
	);
}

export async function assertAddIndexerSuccessState(
	t,
	context,
	{
		writerPeer = selectIndexerCandidatePeer(context),
		adminPeer = context.adminBootstrap,
		writerEntryBefore,
		adminEntryBefore,
		payload,
		skipSync = false
	} = {}
) {
	if (!writerEntryBefore?.value) {
		throw new Error('assertAddIndexerSuccessState requires writerEntryBefore.');
	}
	if (!adminEntryBefore?.value) {
		throw new Error('assertAddIndexerSuccessState requires adminEntryBefore.');
	}
	if (!payload) {
		throw new Error('assertAddIndexerSuccessState requires the processed payload.');
	}

	const writerAddress = writerPeer.wallet.address;
	const decodedWriterBefore = nodeEntryUtils.decode(b4a.from(writerEntryBefore.value));
	t.ok(decodedWriterBefore, 'writer entry before addIndexer decodes');
	if (!decodedWriterBefore) return;

	await assertIndexerNodeEntry(t, adminPeer.base, writerAddress, decodedWriterBefore, {
		verifyMembership: false
	});

	await assertAdminPaidFee(t, adminPeer.base, adminPeer.wallet.address, adminEntryBefore.value);

	await assertAddIndexerPayloadMetadata(
		t,
		adminPeer.base,
		payload,
		adminPeer.wallet.address,
		writerAddress
	);

	if (!skipSync) {
		await context.sync();
		await assertIndexerNodeEntry(t, writerPeer.base, writerAddress, decodedWriterBefore, {
			verifyMembership: true
		});
	}
}

export async function assertAddIndexerFailureState(
	t,
	context,
	{ writerPeer = selectIndexerCandidatePeer(context), adminPeer = context.adminBootstrap, skipSync = false } = {}
) {
	await assertWriterRemainsNonIndexer(t, adminPeer.base, writerPeer);

	if (!skipSync) {
		await context.sync();
		await assertWriterRemainsNonIndexer(t, writerPeer.base, writerPeer);
	}
}

async function assertIndexerNodeEntry(t, base, address, referenceNodeEntry, { verifyMembership = true } = {}) {
	const entry = await base.view.get(address);
	t.ok(entry, 'indexer node entry exists');
	const decoded = nodeEntryUtils.decode(entry?.value);
	t.ok(decoded, 'indexer node entry decodes');
	if (!decoded) return;

	t.is(decoded.isWhitelisted, true, 'indexer remains whitelisted');
	t.is(decoded.isWriter, true, 'indexer retains writer role');
	t.is(decoded.isIndexer, true, 'indexer role assigned');
	t.ok(b4a.equals(decoded.wk, referenceNodeEntry.wk), 'indexer writing key preserved');
	t.ok(b4a.equals(decoded.balance, referenceNodeEntry.balance), 'indexer balance preserved');
	t.ok(
		b4a.equals(decoded.stakedBalance, referenceNodeEntry.stakedBalance),
		'indexer staked balance preserved'
	);
	t.ok(b4a.equals(decoded.license, referenceNodeEntry.license), 'indexer license preserved');
	if (verifyMembership) {
		assertIndexerMembership(t, base, referenceNodeEntry.wk);
	}
}

async function assertAdminPaidFee(t, base, adminAddress, adminEntryBeforeValue) {
	const decodedBefore = nodeEntryUtils.decode(b4a.from(adminEntryBeforeValue));
	t.ok(decodedBefore, 'admin entry before addIndexer decodes');
	if (!decodedBefore) return;

	const adminEntryAfter = await base.view.get(adminAddress);
	t.ok(adminEntryAfter, 'admin entry exists after addIndexer');
	const decodedAfter = nodeEntryUtils.decode(adminEntryAfter?.value);
	t.ok(decodedAfter, 'admin entry decodes after addIndexer');
	if (!decodedAfter) return;

	const balanceBefore = toBalance(decodedBefore.balance);
	t.ok(balanceBefore, 'admin balance before addIndexer decodes');
	if (!balanceBefore) return;

	const feeAmount = toBalance(transactionUtils.FEE);
	t.ok(feeAmount, 'addIndexer fee decodes');
	if (!feeAmount) return;

	const expectedBalance = balanceBefore.sub(feeAmount);
	t.ok(expectedBalance, 'admin balance after fee computation succeeds');
	if (!expectedBalance) return;

	t.ok(
		b4a.equals(decodedAfter.balance, expectedBalance.value),
		'admin balance reduced by addIndexer fee'
	);
	t.ok(
		b4a.equals(decodedAfter.stakedBalance, decodedBefore.stakedBalance),
		'admin staked balance remains unchanged after addIndexer'
	);
}

async function assertAddIndexerPayloadMetadata(t, base, payload, expectedAdminAddress, expectedCandidate) {
	const decodedOperation = safeDecodeApplyOperation(payload);
	t.ok(decodedOperation, 'addIndexer payload decodes');
	if (!decodedOperation) return;

	const requesterAddressBuffer = decodedOperation.address;
	t.ok(requesterAddressBuffer, 'addIndexer payload contains requester address');
	const requesterAddress = addressUtils.bufferToAddress(requesterAddressBuffer, config.addressPrefix);
	t.ok(requesterAddress, 'addIndexer requester address decodes');
	if (requesterAddress) {
		t.is(requesterAddress, expectedAdminAddress, 'addIndexer payload signed by admin');
	}

	const candidateAddressBuffer = decodedOperation?.aco?.ia;
	t.ok(candidateAddressBuffer, 'addIndexer payload contains candidate address');
	const candidateAddress = addressUtils.bufferToAddress(candidateAddressBuffer, config.addressPrefix);
	t.ok(candidateAddress, 'addIndexer candidate address decodes');
	if (candidateAddress) {
		t.is(candidateAddress, expectedCandidate, 'addIndexer payload nominates expected writer');
	}

	const txHashBuffer = decodedOperation?.aco?.tx;
	t.ok(txHashBuffer, 'addIndexer tx hash extracted');
	if (txHashBuffer) {
		const txEntry = await base.view.get(txHashBuffer.toString('hex'));
		t.ok(txEntry, 'addIndexer transaction recorded for replay protection');
	}
}

async function assertWriterRemainsNonIndexer(t, base, writerPeer) {
	const entry = await base.view.get(writerPeer.wallet.address);
	t.ok(entry, 'writer entry exists after failed addIndexer');
	const decoded = nodeEntryUtils.decode(entry?.value);
	t.ok(decoded, 'writer entry decodes after failed addIndexer');
	if (!decoded) return;
	t.is(decoded.isIndexer, false, 'writer not promoted to indexer');
	assertIndexerMembershipAbsent(t, base, decoded.wk);
}

function assertIndexerMembership(t, base, writingKey) {
	const hasMembership = indexerMembershipIncludes(base, writingKey);
	t.ok(hasMembership, 'indexer writer key added to validator set');
}

function assertIndexerMembershipAbsent(t, base, writingKey) {
	const hasMembership = indexerMembershipIncludes(base, writingKey);
	t.is(hasMembership, false, 'writer key absent from validator set');
}

function indexerMembershipIncludes(base, writingKey) {
	const entries = base?.system?.indexers;
	if (!entries) return false;
	return Object.values(entries).some(entry => entry?.key && b4a.equals(entry.key, writingKey));
}

function registerWriterKeyInSystemIndexers(base, writingKey) {
	const system = base?.system;
	if (!system) return () => {};

	if (!Array.isArray(system.indexers)) {
		system.indexers = system.indexers ? Array.from(system.indexers) : [];
	}

	const membership = system.indexers;
	const existing = membership.find(entry => entry?.key && b4a.equals(entry.key, writingKey));
	if (existing) {
		return () => {};
	}

	const entry = { key: writingKey, length: 0 };
	membership.push(entry);

	if (system._indexerMap instanceof Map) {
		system._indexerMap.set(writingKey.toString('hex'), entry);
	}

	return () => {
		const index = membership.indexOf(entry);
		if (index !== -1) {
			membership.splice(index, 1);
		}
		if (system._indexerMap instanceof Map) {
			system._indexerMap.delete(writingKey.toString('hex'));
		}
	};
}

export async function applyWithPretenderRoleMutation(context, invalidPayload, role) {
	const adminPeer = context.adminBootstrap;
	const writerPeer = context.addIndexerScenario?.writerPeer ?? selectIndexerCandidatePeer(context);
	if (!adminPeer?.base || !writerPeer?.wallet?.address) {
		throw new Error('Pretender role mutation scenario requires admin and writer peers.');
	}

	const address = writerPeer.wallet.address;
	const originalApply = adminPeer.base._handlers.apply;
	const roleValue = typeof role === 'number' ? role : nodeRoleUtils.NodeRole.INDEXER;

	adminPeer.base._handlers.apply = async function patchedApply(nodes, view, baseCtx) {
		let mutatedOnce = false;
		const originalBatch = view.batch;
		view.batch = function patchedBatch(...args) {
			const batch = originalBatch.apply(this, args);
			if (!batch?.get) return batch;

			const originalGet = batch.get.bind(batch);
			batch.get = async key => {
				if (mutatedOnce) return originalGet(key);
				if (typeof key === 'string' ? key !== address : true) {
					// string path comparison; buffer path is unlikely for address entries here
					if (b4a.isBuffer(key)) {
						const addrBuf = addressUtils.addressToBuffer(address, config.addressPrefix);
						if (!addrBuf || !b4a.equals(addrBuf, key)) {
							return originalGet(key);
						}
					} else {
						return originalGet(key);
					}
				}

				const entry = await originalGet(key);
				if (!entry?.value) {
					throw new Error('Pretender role mutation scenario requires an existing writer entry.');
				}

				const mutated = nodeEntryUtils.setRole(b4a.from(entry.value), roleValue);
				if (!mutated) {
					throw new Error('Failed to mutate pretender node role.');
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

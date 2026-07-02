import b4a from 'b4a';
import { setupStateNetwork } from '../../../../helpers/StateNetworkFactory.js';
import {
	seedBootstrapIndexer,
	defaultOpenHyperbeeView,
	deriveIndexerSequenceState,
	eventFlush
} from '../../../../helpers/autobaseTestHelpers.js';
import { applyStateMessageFactory } from '../../../../../src/messages/state/applyStateMessageFactory.js';
import {
	safeDecodeApplyOperation,
	safeEncodeApplyOperation
} from '../../../../../src/utils/protobuf/operationHelpers.js';
import {
	AUTOBASE_VALUE_ENCODING,
	EntryType,
	ADMIN_INITIAL_BALANCE,
	ADMIN_INITIAL_STAKED_BALANCE
} from '../../../../../src/utils/constants.js';
import adminEntryUtils from '../../../../../src/core/state/utils/adminEntry.js';
import nodeEntryUtils from '../../../../../src/core/state/utils/nodeEntry.js';
import lengthEntryUtils from '../../../../../src/core/state/utils/lengthEntry.js';
import addressUtils from '../../../../../src/core/state/utils/address.js';
import { safeWriteUInt32BE } from '../../../../../src/utils/buffer.js';
import { config } from '../../../../helpers/config.js';

export async function setupAddAdminScenario(t) {
	const context = await setupStateNetwork({
		nodes: 2,
		valueEncoding: AUTOBASE_VALUE_ENCODING,
		open: defaultOpenHyperbeeView
	});

	seedBootstrapIndexer(context);

	t.teardown(async () => {
		await context.teardown();
	});
	return context;
}

export async function buildAddAdminRequesterPayload(context) {
	const adminNode = context.adminBootstrap;
	const txValidity = await deriveIndexerSequenceState(adminNode.base);
	return safeEncodeApplyOperation(
		await applyStateMessageFactory(adminNode.wallet, config)
			.buildCompleteAddAdminMessage(
				adminNode.wallet.address,
				adminNode.base.local.key,
				txValidity
			)
	);
}

export async function assertAddAdminRequesterFailureState(t, context) {
	const adminNode = context.adminBootstrap;
	const readerNodes = context.peers.slice(1);

	const adminEntryRecord = await adminNode.base.view.get(EntryType.ADMIN);
	t.is(adminEntryRecord, null, 'admin entry remains absent');

	const initializationEntry = await adminNode.base.view.get(EntryType.INITIALIZATION);
	t.is(initializationEntry, null, 'initialization flag not set');

	const writerRegistry = await adminNode.base.view.get(
		EntryType.WRITER_ADDRESS + adminNode.base.local.key.toString('hex')
	);
	t.is(writerRegistry, null, 'writer registry not created');

	await context.sync();
	for (const reader of readerNodes) {
		const readerAdminEntry = await reader.base.view.get(EntryType.ADMIN);
		t.is(readerAdminEntry, null, 'reader node never observes admin entry');
	}
}

export async function assertAddAdminRequesterFailureStateLocal(t, context) {
	const adminNode = context.adminBootstrap;

	const adminEntryRecord = await adminNode.base.view.get(EntryType.ADMIN);
	t.is(adminEntryRecord, null, 'admin entry remains absent');

	const initializationEntry = await adminNode.base.view.get(EntryType.INITIALIZATION);
	t.is(initializationEntry, null, 'initialization flag not set');

	const writerRegistry = await adminNode.base.view.get(
		EntryType.WRITER_ADDRESS + adminNode.base.local.key.toString('hex')
	);
	t.is(writerRegistry, null, 'writer registry not created');

	for (const reader of context.peers.slice(1)) {
		const readerAdminEntry = await reader.base.view.get(EntryType.ADMIN);
		t.is(readerAdminEntry, null, 'reader node never observes admin entry');
	}
}

export function mutateAddAdminPayloadForInvalidSchema(t, validPayload) {
	const decodedPayload = safeDecodeApplyOperation(validPayload);
	t.ok(decodedPayload, 'fixtures decode');

	decodedPayload.cao.tx = b4a.alloc(decodedPayload.cao.tx.length);
	return safeEncodeApplyOperation(decodedPayload);
}

export async function assertAdminState(t, base, wallet, writingKey, payload) {
	const adminEntryRecord = await base.view.get(EntryType.ADMIN);
	t.ok(adminEntryRecord, 'admin entry should exist');

	const decodedAdminEntry = adminEntryUtils.decode(adminEntryRecord.value, config.addressPrefix);
	t.ok(decodedAdminEntry, 'admin entry decodes');
	t.is(decodedAdminEntry.address, wallet.address, 'admin entry stores wallet address');
	t.ok(b4a.equals(decodedAdminEntry.wk, writingKey), 'admin entry stores writing key');

	const nodeEntryRecord = await base.view.get(wallet.address);
	t.ok(nodeEntryRecord, 'node entry should exist for admin address');

	const nodeEntry = nodeEntryUtils.decode(nodeEntryRecord.value);
	t.ok(nodeEntry, 'node entry decodes');
	t.is(nodeEntry.isWriter, true, 'node entry flagged as writer');
	t.is(nodeEntry.isIndexer, true, 'node entry flagged as indexer');
	t.ok(b4a.equals(nodeEntry?.wk, writingKey), 'node entry writing key matches');
	t.ok(b4a.equals(nodeEntry?.balance, ADMIN_INITIAL_BALANCE), 'admin initial balance is set');
	t.ok(
		b4a.equals(nodeEntry?.stakedBalance, ADMIN_INITIAL_STAKED_BALANCE),
		'admin initial staked balance is set'
	);
	t.ok(
		b4a.equals(nodeEntry?.license, lengthEntryUtils.encodeBE(1)),
		'admin license id assigned'
	);

	const adminAddressBuffer = addressUtils.addressToBuffer(wallet.address, config.addressPrefix);
	t.ok(adminAddressBuffer.length > 0, 'admin address encoded as buffer');
	const writerRegistry = await base.view.get(EntryType.WRITER_ADDRESS + writingKey.toString('hex'));
	t.ok(writerRegistry, 'writer registry entry exists');
	t.ok(
		b4a.equals(writerRegistry.value, adminAddressBuffer),
		'writer registry links writing key to admin'
	);

	const writersLengthEntry = await base.view.get(EntryType.WRITERS_LENGTH);
	t.ok(writersLengthEntry, 'writers length entry exists');
	const writersLength = lengthEntryUtils.decodeBE(writersLengthEntry.value);
	t.is(writersLength, 1, 'writers length increments to 1');

	const writerIndexEntry = await base.view.get(`${EntryType.WRITERS_INDEX}0`);
	t.ok(writerIndexEntry, 'writer index entry exists');
	t.is(
		writerIndexEntry.value.toString('ascii'),
		adminAddressBuffer.toString('ascii'),
		'writer index 0 stores admin address'
	);

	const licenseCountEntry = await base.view.get(EntryType.LICENSE_COUNT);
	t.ok(licenseCountEntry, 'license count entry exists');
	const licenseCount = lengthEntryUtils.decodeBE(licenseCountEntry.value);
	t.is(licenseCount, 1, 'license count increments to 1');

	const licenseIndexEntry = await base.view.get(`${EntryType.LICENSE_INDEX}1`);
	t.ok(licenseIndexEntry, 'license index entry exists');
	t.is(
		licenseIndexEntry.value.toString('ascii'),
		adminAddressBuffer.toString('ascii'),
		'license index 1 stores admin address'
	);

	const initializationEntry = await base.view.get(EntryType.INITIALIZATION);
	t.ok(initializationEntry, 'initialization entry exists');
	t.ok(
		b4a.equals(initializationEntry.value, safeWriteUInt32BE(1, 0)),
		'initialization flag set to 1'
	);

	const decodedOperation = safeDecodeApplyOperation(payload);
	t.ok(decodedOperation?.cao?.tx, 'operation decodes');
	const txKey = decodedOperation.cao.tx.toString('hex');
	const txEntry = await base.view.get(txKey);
	t.ok(txEntry, 'operation hash stored to prevent replays');
}

export async function assertAdminStatePersists(t, context, payload) {
	const adminNode = context.adminBootstrap;
	const reader = context.peers[1];

	await assertAdminState(t, adminNode.base, adminNode.wallet, adminNode.base.local.key, payload);

	await eventFlush();
	await context.sync();

	await assertAdminState(t, reader.base, adminNode.wallet, adminNode.base.local.key, payload);
}

export function bypassAddAdminReplayGuardsOnce(context) {
	const adminNode = context.adminBootstrap;
	if (!adminNode?.base) {
		throw new Error('AddAdmin replay guard bypass requires an admin bootstrap node.');
	}

	const writerRegistryKey = EntryType.WRITER_ADDRESS + adminNode.base.local.key.toString('hex');
	const keysToBypass = new Set([writerRegistryKey, EntryType.ADMIN]);

	return patchViewGetOnce(adminNode.base, keysToBypass);
}

export function bypassBalanceInitializationAdminConsistencyOnce(context) {
	const adminNode = context.adminBootstrap;
	if (!adminNode?.base) {
		throw new Error('Admin consistency bypass requires an admin bootstrap node.');
	}

	const keysToBypass = new Set([EntryType.ADMIN]);

	return patchViewGetOnce(adminNode.base, keysToBypass, {
		mutateAdminEntry: async entryPromise => {
			const entry = await entryPromise;
			if (!entry?.value) return entry;
			const mutated = b4a.from(entry.value);
			mutated[mutated.length - 1] ^= 0xff;
			return { ...entry, value: mutated };
		}
	});
}

function patchViewGetOnce(base, keysToBypass, { mutateAdminEntry } = {}) {
	const originalApply = base._handlers.apply;
	let shouldPatchNextApply = true;

	base._handlers.apply = async (nodes, view, baseCtx) => {
		if (!shouldPatchNextApply) {
			return originalApply(nodes, view, baseCtx);
		}

		shouldPatchNextApply = false;
		const previousBatch = view.batch;
		const boundBatch = previousBatch.bind(view);

		view.batch = function patchedBatch(...args) {
			const batch = boundBatch(...args);
			const originalGet = batch.get?.bind(batch);
			if (typeof originalGet === 'function') {
				batch.get = async key => {
					const identifier = resolveKeyIdentifier(key);
				if (keysToBypass.has(identifier)) {
					if (identifier === EntryType.ADMIN && typeof mutateAdminEntry === 'function') {
						return mutateAdminEntry(originalGet(key));
					}
					return null;
				}
					return originalGet(key);
				};
			}
			return batch;
		};

		try {
			return await originalApply(nodes, view, baseCtx);
		} finally {
			view.batch = previousBatch;
			base._handlers.apply = originalApply;
		}
	};

	return () => {
		base._handlers.apply = originalApply;
	};
}

function resolveKeyIdentifier(key) {
	if (typeof key === 'string') {
		return key;
	}
	if (b4a.isBuffer(key)) {
		return key.toString('utf8');
	}
	return `${key}`;
}

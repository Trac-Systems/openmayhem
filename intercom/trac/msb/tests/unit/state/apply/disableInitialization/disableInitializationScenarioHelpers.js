import b4a from 'b4a';
import { setupStateNetwork } from '../../../../helpers/StateNetworkFactory.js';
import {
	seedBootstrapIndexer,
	defaultOpenHyperbeeView,
	deriveIndexerSequenceState,
	eventFlush
} from '../../../../helpers/autobaseTestHelpers.js';
import { applyStateMessageFactory } from '../../../../../src/messages/state/applyStateMessageFactory.js';
import { AUTOBASE_VALUE_ENCODING, EntryType } from '../../../../../src/utils/constants.js';
import { safeDecodeApplyOperation, safeEncodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import { safeWriteUInt32BE } from '../../../../../src/utils/buffer.js';
import { buildAddAdminRequesterPayload } from '../addAdmin/addAdminScenarioHelpers.js';
import { config } from '../../../../helpers/config.js';

export async function setupDisableInitializationScenario(t) {
	const context = await setupStateNetwork({
		nodes: 2,
		valueEncoding: AUTOBASE_VALUE_ENCODING,
		open: defaultOpenHyperbeeView
	});

	seedBootstrapIndexer(context);

	t.teardown(async () => {
		await context.teardown();
	});

	await bootstrapAdmin(context);
	return context;
}

export async function setupDisableInitializationAdminReaderScenario(t) {
	const context = await setupDisableInitializationScenario(t);
	const adminNode = context.adminBootstrap;
	const readerNode = context.peers[1];

	if (!readerNode) {
		throw new Error('Disable initialization scenarios require at least one reader node.');
	}

	return { context, adminNode, readerNode };
}

async function bootstrapAdmin(context) {
	const adminNode = context.adminBootstrap;
	const payload = await buildAddAdminRequesterPayload(context);

	await adminNode.base.append(payload);
	await adminNode.base.update();
	await eventFlush();
}

export async function buildDisableInitializationPayload(context) {
	const adminNode = context.adminBootstrap;
	const txValidity = await deriveIndexerSequenceState(adminNode.base);

	return safeEncodeApplyOperation(
		await applyStateMessageFactory(adminNode.wallet, config)
			.buildCompleteDisableInitializationMessage(
				adminNode.wallet.address,
				adminNode.base.local.key,
				txValidity
			)
	);
}

export async function buildDisableInitializationPayloadWithTxValidity(context, txValidity) {
	const adminNode = context.adminBootstrap;
	return safeEncodeApplyOperation(
		await applyStateMessageFactory(adminNode.wallet, config)
			.buildCompleteDisableInitializationMessage(
				adminNode.wallet.address,
				adminNode.base.local.key,
				txValidity
			)
	);
}

export async function assertInitializationDisabledState(t, base, payload) {
	const initializationEntry = await base.view.get(EntryType.INITIALIZATION);
	t.ok(initializationEntry, 'initialization entry exists');
	t.ok(
		b4a.equals(initializationEntry.value, safeWriteUInt32BE(0, 0)),
		'initialization flag cleared'
	);

	if (!payload) return;

	const decodedPayload = safeDecodeApplyOperation(payload);
	t.ok(decodedPayload?.cao?.tx, 'disable payload decodes');

	const txKey = decodedPayload.cao.tx.toString('hex');
	const txEntry = await base.view.get(txKey);
	t.ok(txEntry, 'disable initialization tx recorded to prevent replays');
}

export function mutateDisableInitializationPayloadForInvalidSchema(t, validPayload) {
	const decodedPayload = safeDecodeApplyOperation(validPayload);
	t.ok(decodedPayload, 'fixtures decode');

	decodedPayload.cao.tx = b4a.alloc(decodedPayload.cao.tx.length);
	return safeEncodeApplyOperation(decodedPayload);
}

export async function assertDisableInitializationFailureState(
	t,
	context,
	{ skipSync = false, validPayload = null } = {}
) {
	const adminNode = context.adminBootstrap;
	const readerNodes = context.peers.slice(1);

	await assertInitializationEnabled(t, adminNode.base);
	await assertOperationNotRecorded(t, adminNode.base, validPayload);

	if (!skipSync) {
		await context.sync();
		for (const reader of readerNodes) {
			await assertInitializationEnabled(t, reader.base);
			await assertOperationNotRecorded(t, reader.base, validPayload);
		}
	}
}

async function assertInitializationEnabled(t, base) {
	const initializationEntry = await base.view.get(EntryType.INITIALIZATION);
	t.ok(initializationEntry, 'initialization entry exists');
	t.ok(
		b4a.equals(initializationEntry.value, safeWriteUInt32BE(1, 0)),
		'initialization flag remains enabled'
	);
}

async function assertOperationNotRecorded(t, base, payload) {
	if (!payload) return;

	const decodedPayload = safeDecodeApplyOperation(payload);
	if (!decodedPayload?.cao?.tx) return;

	const txKey = decodedPayload.cao.tx.toString('hex');
	const txEntry = await base.view.get(txKey);
	t.is(txEntry, null, 'disable initialization tx not recorded');
}

export function bypassDisableInitializationAlreadyDisabledGuardOnce(context) {
	const adminNode = context.adminBootstrap;
	if (!adminNode?.base) {
		throw new Error('Disable initialization guard bypass requires an admin bootstrap node.');
	}

	const originalApply = adminNode.base._handlers.apply;
	let shouldBypass = true;

	adminNode.base._handlers.apply = async (nodes, view, baseCtx) => {
		if (!shouldBypass) {
			return originalApply(nodes, view, baseCtx);
		}

		shouldBypass = false;
		const previousBatch = view.batch;
		const boundBatch = previousBatch.bind(view);

		view.batch = function patchedBatch(...args) {
			const batch = boundBatch(...args);
			const originalGet = batch.get?.bind(batch);
			if (typeof originalGet === 'function') {
				batch.get = async key => {
					const entry = await originalGet(key);
					if (isInitializationEntryKey(key) && entry?.value) {
						return {
							...entry,
							value: safeWriteUInt32BE(1, 0)
						};
					}
					return entry;
				};
			}
			return batch;
		};

		try {
			return await originalApply(nodes, view, baseCtx);
		} finally {
			view.batch = previousBatch;
		}
	};

	return () => {
		adminNode.base._handlers.apply = originalApply;
	};
}

function isInitializationEntryKey(key) {
	if (typeof key === 'string') {
		return key === EntryType.INITIALIZATION;
	}
	if (b4a.isBuffer(key)) {
		return b4a.equals(key, b4a.from(EntryType.INITIALIZATION));
	}
	return false;
}

export default setupDisableInitializationScenario;

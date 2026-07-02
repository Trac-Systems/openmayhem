import b4a from 'b4a';
import adminEntryUtils from '../../../../../src/core/state/utils/adminEntry.js';
import nodeEntryUtils, { setWritingKey } from '../../../../../src/core/state/utils/nodeEntry.js';
import { EntryType } from '../../../../../src/utils/constants.js';
import { decimalStringToBigInt, bigIntTo16ByteBuffer } from '../../../../../src/utils/amountSerialization.js';
import { deriveIndexerSequenceState, eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import { applyStateMessageFactory } from '../../../../../src/messages/state/applyStateMessageFactory.js';
import { safeEncodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import {
	setupAdminNetwork,
	initializeBalances,
	whitelistAddress
} from '../common/commonScenarioHelper.js';
import { promotePeerToWriter } from '../addWriter/addWriterScenarioHelpers.js';
import { buildAddIndexerPayload } from '../addIndexer/addIndexerScenarioHelpers.js';
import { toBalance, BALANCE_FEE } from '../../../../../src/core/state/utils/balance.js';
import lengthEntryUtils from '../../../../../src/core/state/utils/lengthEntry.js';
import { safeDecodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import { config } from '../../../../helpers/config.js';

export const DEFAULT_FUNDING = bigIntTo16ByteBuffer(decimalStringToBigInt('50'));
export const TRANSFER_AMOUNT = bigIntTo16ByteBuffer(decimalStringToBigInt('1'));
export const TRANSFER_COUNT = 20;

export async function setupAdminRecoveryScenario(t) {
	const context = await setupAdminNetwork(t, { nodes: 5 });
	const [adminPeer, indexerPeer1, indexerPeer2, validatorPeer1, validatorPeer2] = context.peers;

	// Fund everyone (including admin) so fees/stakes succeed.
	await initializeBalances(context, [
		[adminPeer.wallet.address, DEFAULT_FUNDING],
		[indexerPeer1.wallet.address, DEFAULT_FUNDING],
		[indexerPeer2.wallet.address, DEFAULT_FUNDING],
		[validatorPeer1.wallet.address, DEFAULT_FUNDING],
		[validatorPeer2.wallet.address, DEFAULT_FUNDING]
	]);

	context.addWriterScenario = { writerInitialBalance: DEFAULT_FUNDING };

	// Whitelist all non-admin peers.
	for (const peer of [indexerPeer1, indexerPeer2, validatorPeer1, validatorPeer2]) {
		await whitelistAddress(context, peer.wallet.address);
	}

	// Promote validators to writers.
	await promotePeerToWriter(t, context, {
		readerPeer: validatorPeer1,
		expectedWriterIndex: await currentWritersLength(adminPeer)
	});
	await promotePeerToWriter(t, context, {
		readerPeer: validatorPeer2,
		expectedWriterIndex: await currentWritersLength(adminPeer)
	});

	// Register validator writers as indexers.
	await addIndexer(context, validatorPeer1);
	await addIndexer(context, validatorPeer2);

	// Use an existing peer's writing key (currently not registered as a writer) for recovery.
	const newWriterKey = indexerPeer1.base.local.key;

	context.adminRecovery = {
		adminPeer,
		indexerPeer1,
		indexerPeer2,
		validatorPeer1,
		validatorPeer2,
		oldAdminWriterKey: adminPeer.base.local.key,
		newAdminWriterKey: newWriterKey
	};

	await context.sync();
	return context;
}

async function addIndexer(context, writerPeer) {
	const adminPeer = context.adminBootstrap;
	const payload = await buildAddIndexerPayload(context, { writerPeer, adminPeer });
	await adminPeer.base.append(payload);
	await adminPeer.base.update();
	await eventFlush();
}

async function currentWritersLength(adminPeer) {
	const writersLengthEntry = await adminPeer.base.view.get(EntryType.WRITERS_LENGTH);
	return writersLengthEntry ? lengthEntryUtils.decodeBE(writersLengthEntry.value) : 0;
}

export async function buildAdminRecoveryPayload(context) {
	const { adminPeer, validatorPeer1, newAdminWriterKey } = context.adminRecovery;
	const txValidity = await deriveIndexerSequenceState(validatorPeer1.base);

    const partial = await applyStateMessageFactory(adminPeer.wallet, config)
        .buildPartialAdminRecoveryMessage(
            adminPeer.wallet.address,
            b4a.toString(newAdminWriterKey, 'hex'),
            b4a.toString(txValidity, 'hex'),
            'json'
        );

    const payload = await applyStateMessageFactory(validatorPeer1.wallet, config)
        .buildCompleteAdminRecoveryMessage(
            partial.address,
            b4a.from(partial.rao.tx, 'hex'),
            b4a.from(partial.rao.txv, 'hex'),
            b4a.from(partial.rao.iw, 'hex'),
            b4a.from(partial.rao.in, 'hex'),
            b4a.from(partial.rao.is, 'hex')
        );
    return safeEncodeApplyOperation(payload);
}

export async function buildAdminRecoveryPayloadWithTxValidity(context, mutatedTxValidity) {
	if (!b4a.isBuffer(mutatedTxValidity)) {
		throw new Error('buildAdminRecoveryPayloadWithTxValidity requires a tx validity buffer.');
	}

	const { adminPeer, validatorPeer1, newAdminWriterKey } = context.adminRecovery;
    const partial = await applyStateMessageFactory(adminPeer.wallet, config)
        .buildPartialAdminRecoveryMessage(
            adminPeer.wallet.address,
            b4a.toString(newAdminWriterKey, 'hex'),
            b4a.toString(mutatedTxValidity, 'hex'),
            'json'
        );

    const payload = await applyStateMessageFactory(validatorPeer1.wallet, config)
        .buildCompleteAdminRecoveryMessage(
			partial.address,
            b4a.from(partial.rao.tx, 'hex'),
            mutatedTxValidity,
            b4a.from(partial.rao.iw, 'hex'),
            b4a.from(partial.rao.in, 'hex'),
            b4a.from(partial.rao.is, 'hex')
        );
    return safeEncodeApplyOperation(payload);
}

export async function applyAdminRecovery(context, payload) {
	const { validatorPeer1 } = context.adminRecovery;
	await validatorPeer1.base.append(payload);
	await validatorPeer1.base.update();
	await eventFlush();
}

export async function applyAdminRecoveryViaValidator(context, payload) {
	const { validatorPeer1 } = context.adminRecovery;
	await validatorPeer1.base.append(payload);
	await validatorPeer1.base.update();
	await eventFlush();
}

export async function applyWithAdminEncodeFailure(context, payload) {
	const originalEncode = adminEntryUtils.encode;
	adminEntryUtils.encode = () => b4a.alloc(0);
	try {
		await applyAdminRecoveryViaValidator(context, payload);
	} finally {
		adminEntryUtils.encode = originalEncode;
	}
}

export async function applyWithAdminBalanceDecodeFailure(context, payload) {
	const originalDecode = nodeEntryUtils.decode;
	let shouldMutateBalance = false;

	nodeEntryUtils.decode = function patchedDecode(buffer) {
		const decoded = originalDecode(buffer);
		if (shouldMutateBalance && decoded) {
			shouldMutateBalance = false;
			return { ...decoded, balance: b4a.alloc(1) };
		}
		return decoded;
	};

	try {
		await applyWithAdminNodeEntryMutation(context, payload, entry => {
			if (!entry?.value) return entry;
			shouldMutateBalance = true;
			return entry;
		});
	} finally {
		nodeEntryUtils.decode = originalDecode;
	}
}

export async function applyWithAdminInsufficientBalance(context, payload) {
	await applyWithAdminNodeEntryMutation(context, payload, entry => {
		if (!entry?.value) return entry;
		const mutated = b4a.from(entry.value);
		const updated = nodeEntryUtils.setBalance(mutated, b4a.alloc(16, 0x00));
		return updated ? { ...entry, value: updated } : entry;
	});
}

export async function applyWithAdminFeeSubtractionFailure(context, payload) {
	const balanceUtils = await import('../../../../../src/core/state/utils/balance.js');
	const sample = balanceUtils.toBalance(b4a.alloc(16));
	const prototype = sample ? Object.getPrototypeOf(sample) : null;
	const originalSub = prototype?.sub;

	if (!prototype || typeof originalSub !== 'function') {
		throw new Error('Failed to patch balance subtraction for admin fee scenario.');
	}

	prototype.sub = function patchedSub() {
		prototype.sub = originalSub;
		return null;
	};

	try {
		await applyAdminRecoveryViaValidator(context, payload);
	} finally {
		prototype.sub = originalSub;
	}
}

export async function applyWithValidatorBalanceUpdateFailure(context, payload) {
	const balanceUtils = await import('../../../../../src/core/state/utils/balance.js');
	const sample = balanceUtils.toBalance(b4a.alloc(16));
	const prototype = sample ? Object.getPrototypeOf(sample) : null;
	const originalUpdate = prototype?.update;
	if (!prototype || typeof originalUpdate !== 'function') {
		throw new Error('Failed to patch balance update for validator fee.');
	}

	let targetBuffer = null;
	prototype.update = function patchedUpdate(buffer) {
		if (targetBuffer && buffer && b4a.isBuffer(buffer) && b4a.equals(buffer, targetBuffer)) {
			prototype.update = originalUpdate;
			return null;
		}
		return originalUpdate.call(this, buffer);
	};

	const { validatorPeer1 } = context.adminRecovery;
	const originalApply = validatorPeer1.base._handlers.apply;
	validatorPeer1.base._handlers.apply = async function patchedApply(nodes, view, baseCtx) {
		const originalBatch = view.batch;
		view.batch = function patchedBatch(...args) {
			const batch = originalBatch.apply(this, args);
			const originalGet = batch.get?.bind(batch);
			if (typeof originalGet === 'function') {
				batch.get = async key => {
					if (key === validatorPeer1.wallet.address) {
						const entry = await originalGet(key);
						targetBuffer = entry?.value ?? null;
						return entry;
					}
					return originalGet(key);
				};
			}
			return batch;
		};
		try {
			return await originalApply.call(this, nodes, view, baseCtx);
		} finally {
			view.batch = originalBatch;
		}
	};

	try {
		await applyAdminRecoveryViaValidator(context, payload);
	} finally {
		prototype.update = originalUpdate;
		validatorPeer1.base._handlers.apply = originalApply;
	}
}

export async function applyWithValidatorNodeDecodeFailure(context, payload) {
	const originalDecode = nodeEntryUtils.decode;
	let validatorKey = null;
	let seenOnce = false;

	nodeEntryUtils.decode = function patchedDecode(buffer) {
		if (validatorKey && b4a.isBuffer(buffer) && b4a.equals(buffer, validatorKey)) {
			if (seenOnce) return null;
			seenOnce = true;
		}
		return originalDecode(buffer);
	};

	const { validatorPeer1 } = context.adminRecovery;
	const originalApply = validatorPeer1.base._handlers.apply;
	validatorPeer1.base._handlers.apply = async function patchedApply(nodes, view, baseCtx) {
		const originalBatch = view.batch;
		view.batch = function patchedBatch(...args) {
			const batch = originalBatch.apply(this, args);
			const originalGet = batch.get?.bind(batch);
			if (typeof originalGet === 'function') {
				batch.get = async key => {
					if (key === validatorPeer1.wallet.address) {
						const entry = await originalGet(key);
						validatorKey = entry?.value ?? null;
						return entry;
					}
					return originalGet(key);
				};
			}
			return batch;
		};
		try {
			return await originalApply.call(this, nodes, view, baseCtx);
		} finally {
			view.batch = originalBatch;
		}
	};

	try {
		await applyAdminRecoveryViaValidator(context, payload);
	} finally {
		nodeEntryUtils.decode = originalDecode;
		validatorPeer1.base._handlers.apply = originalApply;
	}
}

export async function applyWithValidatorBalanceDecodeFailure(context, payload) {
	const originalDecode = nodeEntryUtils.decode;
	let validatorKey = null;
	let seenOnce = false;

	nodeEntryUtils.decode = function patchedDecode(buffer) {
		const decoded = originalDecode(buffer);
		if (validatorKey && b4a.isBuffer(buffer) && b4a.equals(buffer, validatorKey)) {
			if (seenOnce) {
				return decoded ? { ...decoded, balance: b4a.alloc(1) } : decoded;
			}
			seenOnce = true;
			return decoded ? { ...decoded, balance: b4a.alloc(1) } : decoded;
		}
		return decoded;
	};

	const { validatorPeer1 } = context.adminRecovery;
	const originalApply = validatorPeer1.base._handlers.apply;
	validatorPeer1.base._handlers.apply = async function patchedApply(nodes, view, baseCtx) {
		const originalBatch = view.batch;
		view.batch = function patchedBatch(...args) {
			const batch = originalBatch.apply(this, args);
			const originalGet = batch.get?.bind(batch);
			if (typeof originalGet === 'function') {
				batch.get = async key => {
					if (key === validatorPeer1.wallet.address) {
						const entry = await originalGet(key);
						validatorKey = entry?.value ?? null;
						return entry;
					}
					return originalGet(key);
				};
			}
			return batch;
		};
		try {
			return await originalApply.call(this, nodes, view, baseCtx);
		} finally {
			view.batch = originalBatch;
		}
	};

	try {
		await applyAdminRecoveryViaValidator(context, payload);
	} finally {
		nodeEntryUtils.decode = originalDecode;
		validatorPeer1.base._handlers.apply = originalApply;
	}
}

export async function applyWithAdminNodeDecodeFailure(context, payload) {
	const originalDecode = nodeEntryUtils.decode;
	const adminNodeEntry = await context.adminRecovery.adminPeer.base.view.get(
		context.adminRecovery.adminPeer.wallet.address
	);
	const expectedBuffer =
		adminNodeEntry?.value && context.adminRecovery.newAdminWriterKey
			? setWritingKey(adminNodeEntry.value, context.adminRecovery.newAdminWriterKey)
			: null;

	let triggered = false;

	nodeEntryUtils.decode = function patchedDecode(buffer) {
		if (
			!triggered &&
			expectedBuffer &&
			buffer &&
			b4a.isBuffer(buffer) &&
			b4a.equals(buffer, expectedBuffer)
		) {
			triggered = true;
			return null;
		}
		return originalDecode(buffer);
	};

	try {
		await applyAdminRecoveryViaValidator(context, payload);
	} finally {
		nodeEntryUtils.decode = originalDecode;
	}
}

export async function applyWithValidatorFeeTransferFailure(context, payload) {
	const balanceUtils = await import('../../../../../src/core/state/utils/balance.js');
	const sample = balanceUtils.toBalance(b4a.alloc(16));
	const prototype = sample ? Object.getPrototypeOf(sample) : null;
	const originalAdd = prototype?.add;
	if (!prototype || typeof originalAdd !== 'function') {
		throw new Error('Failed to patch balance add for validator fee transfer.');
	}

	prototype.add = function patchedAdd() {
		prototype.add = originalAdd;
		return null;
	};

	try {
		await applyAdminRecoveryViaValidator(context, payload);
	} finally {
		prototype.add = originalAdd;
	}
}

export async function applyWithDuplicateOperation(context, payload) {
	const decoded = safeDecodeApplyOperation(payload);
	const txHashHexString = decoded?.rao?.tx ? b4a.toString(decoded.rao.tx, 'hex') : null;
	const { validatorPeer1 } = context.adminRecovery;
	const base = validatorPeer1.base;
	const originalApply = base._handlers.apply;

	base._handlers.apply = async function patchedApply(nodes, view, baseCtx) {
		const originalBatch = view.batch;
		view.batch = function patchedBatch(...args) {
			const batch = originalBatch.apply(this, args);
			if (!batch?.get) return batch;

			const originalGet = batch.get.bind(batch);
			batch.get = async key => {
				if (txHashHexString && key === txHashHexString) {
					return { value: b4a.from('applied') };
				}
				return originalGet(key);
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
		await applyAdminRecoveryViaValidator(context, payload);
	} finally {
		base._handlers.apply = originalApply;
	}
}

export async function applyWithOldWriterKeyMissing(context, payload) {
	const base = context.adminRecovery.validatorPeer1.base;
	const backup = cloneIndexers(base.system.indexers);
	const filteredEntries = Object.values(cloneIndexers(base.system.indexers) || {}).filter(
		entry => !b4a.equals(entry.key, context.adminRecovery.oldAdminWriterKey)
	);
	base.system.indexers = Array.isArray(backup)
		? filteredEntries
		: Object.fromEntries(filteredEntries.map((entry, idx) => [idx, entry]));

	try {
		const rebuiltPayload = await buildAdminRecoveryPayload(context);
		await applyAdminRecoveryViaValidator(context, rebuiltPayload);
	} finally {
		base.system.indexers = backup;
	}
}

export async function applyWithNewWriterKeyPresent(context, payload) {
	const base = context.adminRecovery.validatorPeer1.base;
	const backup = cloneIndexers(base.system.indexers);
	const entries = Object.values(cloneIndexers(base.system.indexers) || {});
	const templateEntry = entries[0] ?? {};
	entries.push({ ...templateEntry, key: b4a.from(context.adminRecovery.newAdminWriterKey) });
	base.system.indexers = Array.isArray(backup)
		? entries
		: Object.fromEntries(entries.map((entry, idx) => [idx, entry]));

	try {
		const rebuiltPayload = await buildAdminRecoveryPayload(context);
		await applyAdminRecoveryViaValidator(context, rebuiltPayload);
	} finally {
		base.system.indexers = backup;
	}
}

export function adminRecoveryScenarioDefaults({
	assertStateUnchanged = (t, context) => assertAdminRecoveryFailureState(t, context)
} = {}) {
	return {
		setupScenario: setupAdminRecoveryScenario,
		buildValidPayload: buildAdminRecoveryPayload,
		assertStateUnchanged,
		applyInvalidPayload: async (context, payload) => applyAdminRecoveryViaValidator(context, payload)
	};
}

export async function applyTransferSeries(context, count = TRANSFER_COUNT) {
	const { indexerPeer1, indexerPeer2, validatorPeer2 } = context.adminRecovery;

	for (let i = 0; i < count; i++) {
		const transferPayload = await buildSimpleTransferPayload({
			requesterPeer: indexerPeer2,
			validatorPeer: validatorPeer2,
			recipientPeer: indexerPeer1,
			amount: TRANSFER_AMOUNT
		});

		await validatorPeer2.base.append(transferPayload);
		await validatorPeer2.base.update();
		await eventFlush();
	}
}

async function buildSimpleTransferPayload({ requesterPeer, validatorPeer, recipientPeer, amount }) {
	const txValidity = await deriveIndexerSequenceState(validatorPeer.base);
	const partial = await applyStateMessageFactory(requesterPeer.wallet, config)
		.buildPartialTransferOperationMessage(
			requesterPeer.wallet.address,
			recipientPeer.wallet.address,
			b4a.toString(amount, 'hex'),
			b4a.toString(txValidity, 'hex'),
			'json'
		);

	const payload = await applyStateMessageFactory(validatorPeer.wallet, config)
		.buildCompleteTransferOperationMessage(
			partial.address,
			b4a.from(partial.tro.tx, 'hex'),
			b4a.from(partial.tro.txv, 'hex'),
			b4a.from(partial.tro.in, 'hex'),
			partial.tro.to,
			b4a.from(partial.tro.am, 'hex'),
			b4a.from(partial.tro.is, 'hex')
		);
	return safeEncodeApplyOperation(payload);
}

export async function assertAdminRecoverySuccessState(t, context, { viewBase } = {}) {
	const {
		adminPeer,
		validatorPeer2,
		oldAdminWriterKey,
		newAdminWriterKey
	} = context.adminRecovery;

	const base = viewBase ?? validatorPeer2.base;
	const adminEntry = await base.view.get(EntryType.ADMIN);
	t.ok(adminEntry, 'admin entry exists');

	const decodedAdminEntry = adminEntryUtils.decode(adminEntry.value, config.addressPrefix);
	t.ok(decodedAdminEntry, 'admin entry decodes');
	t.ok(b4a.equals(decodedAdminEntry.wk, newAdminWriterKey), 'admin writer key updated');

	const adminNodeEntry = await base.view.get(adminPeer.wallet.address);
	const decodedNodeEntry = nodeEntryUtils.decode(adminNodeEntry?.value);
	t.ok(decodedNodeEntry, 'admin node entry decodes');
	t.ok(b4a.equals(decodedNodeEntry.wk, newAdminWriterKey), 'admin node entry writer key updated');

	const adminBalance = toBalance(decodedNodeEntry.balance);
	const initialAdminBalance = toBalance(DEFAULT_FUNDING);
	const feeBalance = toBalance(BALANCE_FEE);
	if (adminBalance && initialAdminBalance && feeBalance) {
		const expectedBalance = initialAdminBalance
			.sub(feeBalance)
			?.sub(feeBalance)
			?.sub(feeBalance);
		if (expectedBalance) {
			t.ok(
				b4a.equals(adminBalance.value, expectedBalance.value),
				'admin balance reduced by accumulated fees'
			);
		}
	}

	const writerAddressEntry = await base.view.get(
		EntryType.WRITER_ADDRESS + newAdminWriterKey.toString('hex')
	);
	t.ok(writerAddressEntry, 'writer address mapping exists for new admin key');

	const indexerKeys = Object.values(base.system.indexers || {}).map(entry =>
		b4a.toString(entry.key, 'hex')
	);
	const oldKeyHex = b4a.toString(oldAdminWriterKey, 'hex');
	const newKeyHex = b4a.toString(newAdminWriterKey, 'hex');
	t.ok(indexerKeys.includes(newKeyHex), 'new admin writer key present in indexers');
	t.ok(!indexerKeys.includes(oldKeyHex), 'bootstrap writer key removed from indexers');
}

export async function assertAdminRecoveryFailureState(t, context, { skipSync } = {}) {
	const { adminPeer, oldAdminWriterKey, newAdminWriterKey } = context.adminRecovery;

	if (!skipSync && typeof context.sync === 'function') {
		await context.sync();
	}

	const adminEntry = await adminPeer.base.view.get(EntryType.ADMIN);
	t.ok(adminEntry, 'admin entry persists');

	const decodedAdminEntry = adminEntryUtils.decode(adminEntry.value, config.addressPrefix);
	t.ok(decodedAdminEntry, 'admin entry decodes');
	t.ok(b4a.equals(decodedAdminEntry.wk, oldAdminWriterKey), 'admin writer key remains unchanged');

	const adminNodeEntry = await adminPeer.base.view.get(adminPeer.wallet.address);
	const decodedNodeEntry = nodeEntryUtils.decode(adminNodeEntry?.value);
	t.ok(decodedNodeEntry, 'admin node entry decodes');
	t.ok(b4a.equals(decodedNodeEntry.wk, oldAdminWriterKey), 'admin node entry writer key unchanged');

	const writerRegistryEntry = await adminPeer.base.view.get(
		EntryType.WRITER_ADDRESS + newAdminWriterKey.toString('hex')
	);
	t.ok(!writerRegistryEntry, 'new admin writer key not registered');

	const indexerKeys = Object.values(adminPeer.base.system.indexers || {}).map(entry =>
		b4a.toString(entry.key, 'hex')
	);
	const oldKeyHex = b4a.toString(oldAdminWriterKey, 'hex');
	const newKeyHex = b4a.toString(newAdminWriterKey, 'hex');
	t.ok(indexerKeys.includes(oldKeyHex), 'old admin writer key still in indexers');
	t.ok(!indexerKeys.includes(newKeyHex), 'new admin writer key not in indexers');
}

export async function applyWithRoleAccessBypass(context, invalidPayload) {
	const { validatorPeer1 } = context.adminRecovery;
	const state = validatorPeer1.state;
	const originalValidate = state.check.validateRoleAccessOperation;
	state.check.validateRoleAccessOperation = () => true;
	try {
		await validatorPeer1.base.append(invalidPayload);
		await validatorPeer1.base.update();
		await eventFlush();
	} finally {
		state.check.validateRoleAccessOperation = originalValidate;
	}
}

export async function applyWithMissingComponentBypass(context, invalidPayload, { missingKey = 'vs' } = {}) {
	const { validatorPeer1 } = context.adminRecovery;
	const state = validatorPeer1.state;
	const originalValidate = state.check.validateRoleAccessOperation;
	const originalHasOwn = Object.hasOwn;
	Object.hasOwn = (obj, prop) => {
		if (prop === missingKey) return false;
		return originalHasOwn(obj, prop);
	};
	state.check.validateRoleAccessOperation = () => true;
	try {
		await validatorPeer1.base.append(invalidPayload);
		await validatorPeer1.base.update();
		await eventFlush();
	} finally {
		state.check.validateRoleAccessOperation = originalValidate;
		Object.hasOwn = originalHasOwn;
	}
}

export async function applyWithInvalidRequesterMessage(context, payload) {
	const originalConcat = b4a.concat;
	b4a.concat = (...args) => {
		const stack = new Error().stack || '';
		if (stack.includes('utils/buffer.js') && stack.includes('createMessage')) {
			b4a.concat = originalConcat;
			return b4a.alloc(0);
		}
		return originalConcat(...args);
	};

	try {
		await applyAdminRecoveryViaValidator(context, payload);
	} finally {
		b4a.concat = originalConcat;
	}
}

export async function applyWithInvalidValidatorMessage(context, payload) {
	const originalConcat = b4a.concat;
	let createMessageCalls = 0;
	b4a.concat = (...args) => {
		const stack = new Error().stack || '';
		if (stack.includes('utils/buffer.js') && stack.includes('createMessage')) {
			createMessageCalls += 1;
			if (createMessageCalls === 2) {
				b4a.concat = originalConcat;
				return b4a.alloc(0);
			}
		}
		return originalConcat(...args);
	};

	try {
		await applyAdminRecoveryViaValidator(context, payload);
	} finally {
		b4a.concat = originalConcat;
	}
}

export async function applyWithRegisteredWriterKey(context, payload) {
	const decoded = safeDecodeApplyOperation(payload);
	const writerKeyHex = decoded?.rao?.iw ? b4a.toString(decoded.rao.iw, 'hex') : null;
	const { validatorPeer1 } = context.adminRecovery;
	const base = validatorPeer1.base;
	const originalApply = base._handlers.apply;

	try {
		base._handlers.apply = async function patchedApply(nodes, view, baseCtx) {
			const originalBatch = view.batch;
			view.batch = function patchedBatch(...args) {
				const batch = originalBatch.apply(this, args);
				if (!batch?.get) return batch;

				const originalGet = batch.get.bind(batch);
				batch.get = async key => {
					if (writerKeyHex && key === EntryType.WRITER_ADDRESS + writerKeyHex) {
						return { value: b4a.from('registered') };
					}
					return originalGet(key);
				};
				return batch;
			};

			try {
				return await originalApply.call(this, nodes, view, baseCtx);
			} finally {
				view.batch = originalBatch;
			}
		};

		await applyAdminRecoveryViaValidator(context, payload);
	} finally {
		base._handlers.apply = originalApply;
	}
}

export async function applyWithIndexerSequenceFailure(context, payload) {
	const { validatorPeer1 } = context.adminRecovery;
	const system = validatorPeer1.base.system;
	const originalDescriptor = Object.getOwnPropertyDescriptor(system, 'indexers');
	const originalValue = system.indexers;
	let injected = false;

	Object.defineProperty(system, 'indexers', {
		configurable: true,
		enumerable: true,
		get() {
			if (!injected) {
				injected = true;
				throw new Error('forced indexer sequence failure');
			}
			return originalValue;
		},
		set(value) {
			return Reflect.set(system, 'indexers', value);
		}
	});

	try {
		await validatorPeer1.base.append(payload).catch(() => {});
		await validatorPeer1.base.update().catch(() => {});
		await eventFlush();
	} finally {
		if (originalDescriptor) {
			Object.defineProperty(system, 'indexers', originalDescriptor);
		} else {
			system.indexers = originalValue;
		}
	}
}

export async function applyWithIndexerSequenceCorruption(context, payload) {
	const { PeerWallet } = await import('trac-wallet');
	const originalHash = PeerWallet.blake3;
	const originalHashSafe = PeerWallet.blake3Safe;

	PeerWallet.blake3 = async () => {
		throw new Error('forced indexer sequence state failure');
	};
	PeerWallet.blake3Safe = async () => {
		return b4a.alloc(0);
	}
	
	try {
		await applyAdminRecoveryViaValidator(context, payload);
	} finally {
		PeerWallet.blake3 = originalHash;
		PeerWallet.blake3Safe = originalHashSafe;
	}
}

export async function applyWithAdminEntryMutation(context, payload, mutateEntry) {
	const { validatorPeer1 } = context.adminRecovery;
	const base = validatorPeer1.base;
	const originalApply = base._handlers.apply;

	base._handlers.apply = async function patchedApply(nodes, view, baseCtx) {
		const originalBatch = view.batch;
		view.batch = function patchedBatch(...args) {
			const batch = originalBatch.apply(this, args);
			if (!batch?.get) return batch;

			const originalGet = batch.get.bind(batch);
			batch.get = async key => {
				if (key === EntryType.ADMIN) {
					const entry = await originalGet(key);
					return mutateEntry(entry);
				}
				return originalGet(key);
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
		await applyAdminRecoveryViaValidator(context, payload);
	} finally {
		base._handlers.apply = originalApply;
	}
}

export async function applyWithAdminNodeEntryMutation(context, payload, mutateEntry) {
	const { validatorPeer1, adminPeer } = context.adminRecovery;
	const targetKey = adminPeer.wallet.address;
	const base = validatorPeer1.base;
	const originalApply = base._handlers.apply;

	base._handlers.apply = async function patchedApply(nodes, view, baseCtx) {
		const originalBatch = view.batch;
		view.batch = function patchedBatch(...args) {
			const batch = originalBatch.apply(this, args);
			if (!batch?.get) return batch;

			const originalGet = batch.get.bind(batch);
			batch.get = async key => {
				if (key === targetKey) {
					const entry = await originalGet(key);
					return mutateEntry(entry);
				}
				return originalGet(key);
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
		await applyAdminRecoveryViaValidator(context, payload);
	} finally {
		base._handlers.apply = originalApply;
	}
}

export function cloneIndexers(indexers) {
	if (Array.isArray(indexers)) {
		return indexers.map(entry => ({ ...entry, key: entry?.key ? b4a.from(entry.key) : entry.key }));
	}

	return Object.fromEntries(
		Object.entries(indexers || {}).map(([k, v]) => [
			k,
			{ ...v, key: v?.key ? b4a.from(v.key) : v.key }
		])
	);
}

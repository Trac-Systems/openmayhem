import b4a from 'b4a';
import { applyStateMessageFactory } from '../../../../../src/messages/state/applyStateMessageFactory.js';
import { safeEncodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import { deriveIndexerSequenceState, eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import { safeDecodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import nodeEntryUtils, { ZERO_LICENSE } from '../../../../../src/core/state/utils/nodeEntry.js';
import lengthEntryUtils from '../../../../../src/core/state/utils/lengthEntry.js';
import addressUtils from '../../../../../src/core/state/utils/address.js';
import { EntryType } from '../../../../../src/utils/constants.js';
import {
	toBalance,
	BALANCE_FEE,
	BALANCE_TO_STAKE,
	BALANCE_ZERO,
	PERCENT_75
} from '../../../../../src/core/state/utils/balance.js';
import { decimalStringToBigInt, bigIntTo16ByteBuffer } from '../../../../../src/utils/amountSerialization.js';
import { setupAdminAndWhitelistedReaderNetwork } from '../common/commonScenarioHelper.js';
import { config } from '../../../../helpers/config.js';

const DEFAULT_WRITER_FUNDING = bigIntTo16ByteBuffer(decimalStringToBigInt('10'));
const STAKE_ENTRY_MARK = Symbol('stake-entry-mark');

export async function setupAddWriterScenario(
	t,
	{ nodes = 2, writerInitialBalance = DEFAULT_WRITER_FUNDING } = {}
) {
	const context = await setupAdminAndWhitelistedReaderNetwork(t, {
		nodes: Math.max(nodes, 2),
		readerInitialBalance: writerInitialBalance
	});
	context.addWriterScenario = { writerInitialBalance };
	return context;
}

export function selectWriterPeer(context, offset = 0) {
	const candidates = context.peers.slice(1);
	if (!candidates.length) {
		throw new Error('AddWriter scenarios require at least one reader peer.');
	}
	return candidates[Math.min(offset, candidates.length - 1)];
}

export function selectValidatorPeerWithoutEntry(context) {
	const writerPeer = selectWriterPeer(context);
	return (
		context.peers.find(
			peer =>
				peer.wallet.address !== context.adminBootstrap.wallet.address &&
				peer.wallet.address !== writerPeer.wallet.address
		) ?? null
	);
}

export async function promotePeerToWriter(
	t,
	context,
	{
		readerPeer = selectWriterPeer(context),
		validatorPeer = context.adminBootstrap,
		writerKeyBuffer = null,
		expectedWriterIndex = null
	} = {}
) {
	if (!readerPeer) {
		throw new Error('promotePeerToWriter requires a reader peer.');
	}
	if (!validatorPeer) {
		throw new Error('promotePeerToWriter requires a validator peer.');
	}

	const payload = await buildAddWriterPayload(context, {
		readerPeer,
		validatorPeer,
		writerKeyBuffer
	});

	await validatorPeer.base.append(payload);
	await validatorPeer.base.update();
	await eventFlush();

	const assertOptions = {
		readerPeer,
		validatorPeer,
		writerKeyBuffer,
		payload
	};

	if (typeof expectedWriterIndex === 'number') {
		assertOptions.expectedWriterIndex = expectedWriterIndex;
	}

	await assertAddWriterSuccessState(t, context, assertOptions);

	return payload;
}

export async function buildAddWriterPayload(
	context,
	{
		readerPeer = selectWriterPeer(context),
		validatorPeer = context.adminBootstrap,
		writerKeyBuffer = null
	} = {}
) {
	const txValidity = await deriveIndexerSequenceState(validatorPeer.base);
	const writingKey = writerKeyBuffer ?? readerPeer.base.local.key;
	const partial = await applyStateMessageFactory(readerPeer.wallet, config)
		.buildPartialAddWriterMessage(
			readerPeer.wallet.address,
			writingKey.toString('hex'),
			txValidity.toString('hex'),
			'json'
		);

	const payload = await applyStateMessageFactory(validatorPeer.wallet, config)
		.buildCompleteAddWriterMessage(
			partial.address,
			b4a.from(partial.rao.tx, 'hex'),
			b4a.from(partial.rao.txv, 'hex'),
			b4a.from(partial.rao.iw, 'hex'),
			b4a.from(partial.rao.in, 'hex'),
			b4a.from(partial.rao.is, 'hex')
		);
	return safeEncodeApplyOperation(payload);
}

export async function buildAddWriterPayloadWithTxValidity(
	context,
	mutatedTxValidity,
	{
		readerPeer = selectWriterPeer(context),
		validatorPeer = context.adminBootstrap,
		writerKeyBuffer = null
	} = {}
) {
	if (!b4a.isBuffer(mutatedTxValidity)) {
		throw new Error('buildAddWriterPayloadWithTxValidity requires a tx validity buffer.');
	}

	const writingKey = writerKeyBuffer ?? readerPeer.base.local.key;
	const partial = await applyStateMessageFactory(readerPeer.wallet, config)
		.buildPartialAddWriterMessage(
			readerPeer.wallet.address,
			writingKey.toString('hex'),
			mutatedTxValidity.toString('hex'),
			'json'
		);

	const payload = await applyStateMessageFactory(validatorPeer.wallet, config)
		.buildCompleteAddWriterMessage(
			partial.address,
			b4a.from(partial.rao.tx, 'hex'),
			mutatedTxValidity,
			b4a.from(partial.rao.iw, 'hex'),
			b4a.from(partial.rao.in, 'hex'),
			b4a.from(partial.rao.is, 'hex')
		);
	return safeEncodeApplyOperation(payload);
}

export async function buildRemoveWriterPayload(
	context,
	{
		readerPeer = selectWriterPeer(context),
		validatorPeer = context.adminBootstrap,
		writerKeyBuffer = null
	} = {}
) {
	const txValidity = await deriveIndexerSequenceState(validatorPeer.base);
	const writerKey = writerKeyBuffer ?? readerPeer.base.local.key;
	const partial = await applyStateMessageFactory(readerPeer.wallet, config)
		.buildPartialRemoveWriterMessage(
			readerPeer.wallet.address,
			writerKey.toString('hex'),
			txValidity.toString('hex'),
			'json'
		);

	const payload = await applyStateMessageFactory(validatorPeer.wallet, config)
		.buildCompleteRemoveWriterMessage(
			partial.address,
			b4a.from(partial.rao.tx, 'hex'),
			b4a.from(partial.rao.txv, 'hex'),
			b4a.from(partial.rao.iw, 'hex'),
			b4a.from(partial.rao.in, 'hex'),
			b4a.from(partial.rao.is, 'hex')
		);
	return safeEncodeApplyOperation(payload);
}

export async function assertAddWriterSuccessState(
	t,
	context,
	{
		readerPeer = selectWriterPeer(context),
		validatorPeer = context.adminBootstrap,
		writerInitialBalance = context.addWriterScenario?.writerInitialBalance,
		validatorBalanceBefore = null,
		payload = null,
		expectedWriterIndex = 1,
		skipSync = false,
		writerKeyBuffer = null
	} = {}
) {
	if (!writerInitialBalance) {
		throw new Error('assertAddWriterSuccessState requires writerInitialBalance buffer.');
	}

	const writerInitial = toBalance(writerInitialBalance);
	const afterFee = writerInitial.sub(BALANCE_FEE);
	const expectedLiquid = afterFee?.sub(BALANCE_TO_STAKE);
	const expectedStaked = BALANCE_TO_STAKE.value;
	if (!expectedLiquid) {
		throw new Error('Failed to derive expected writer balance.');
	}

	const writerAddress = readerPeer.wallet.address;
    const writerAddressBuffer = addressUtils.addressToBuffer(writerAddress, config.addressPrefix);
	const writingKey = writerKeyBuffer ?? readerPeer.base.local.key;
	const writingKeyHex = writingKey.toString('hex');

	await assertWriterEntry(t, validatorPeer.base, writerAddress, writingKey, {
		expectedBalance: expectedLiquid.value,
		expectedStakedBalance: expectedStaked
	});

	await assertWriterRegistry(t, validatorPeer.base, writerAddressBuffer, writingKeyHex, expectedWriterIndex);

	if (validatorBalanceBefore) {
		await assertValidatorReward(t, validatorPeer, validatorBalanceBefore);
	}

	if (payload) {
		await assertReplayProtection(t, validatorPeer.base, payload);
	}

	if (!skipSync) {
		await context.sync();
		await assertWriterEntry(t, readerPeer.base, writerAddress, writingKey, {
			expectedBalance: expectedLiquid.value,
			expectedStakedBalance: expectedStaked
		});
	}
}

async function assertWriterEntry(
	t,
	base,
	address,
	writingKey,
	{ expectedBalance, expectedStakedBalance }
) {
	const entry = await base.view.get(address);
	t.ok(entry, 'writer node entry exists');
	const decoded = nodeEntryUtils.decode(entry.value);
	t.ok(decoded, 'writer node entry decodes');
	t.is(decoded.isWhitelisted, true, 'writer remains whitelisted');
	t.is(decoded.isWriter, true, 'writer role assigned');
	t.is(decoded.isIndexer, false, 'writer not promoted to indexer');
	t.ok(b4a.equals(decoded.wk, writingKey), 'writer key stored on node entry');
	if (expectedBalance) {
		t.ok(
			b4a.equals(decoded.balance, expectedBalance),
			'writer liquid balance reflects stake and fee deductions'
		);
	}
	if (expectedStakedBalance) {
		t.ok(
			b4a.equals(decoded.stakedBalance, expectedStakedBalance),
			'writer staked balance recorded'
		);
	}
}

async function assertReaderNotPromoted(t, base, peer, { expectRegistryEntry = false } = {}) {
	const address = peer.wallet.address;
	const entry = await base.view.get(address);
	t.ok(entry, 'reader entry exists');
	const decoded = nodeEntryUtils.decode(entry.value);
	t.ok(decoded, 'reader entry decodes');
	t.is(decoded.isWhitelisted, true, 'reader stays whitelisted');
	t.is(decoded.isWriter, false, 'reader not promoted to writer');
	t.is(decoded.isIndexer, false, 'reader not an indexer');
	t.ok(
		b4a.equals(decoded.stakedBalance, BALANCE_ZERO.value),
		'reader staked balance remains zero'
	);
	const registryEntry = await base.view.get(
		EntryType.WRITER_ADDRESS + peer.base.local.key.toString('hex')
	);
	if (expectRegistryEntry) {
		t.ok(registryEntry, 'writer registry entry persists for reader');
	} else {
		t.is(registryEntry, null, 'writer registry entry remains absent for reader');
	}
}

export async function assertAddWriterFailureState(
	t,
	context,
	{ skipSync = false, expectRegistryEntry = false } = {}
) {
	const writerPeer = selectWriterPeer(context);
	const validatorPeer = context.adminBootstrap;

	await assertReaderNotPromoted(t, validatorPeer.base, writerPeer, { expectRegistryEntry });

	if (!skipSync) {
		await context.sync();
		await assertReaderNotPromoted(t, writerPeer.base, writerPeer, { expectRegistryEntry });
	}
}

async function assertWriterRegistry(t, base, addressBuffer, writingKeyHex, expectedWriterIndex) {
	const registryEntry = await base.view.get(EntryType.WRITER_ADDRESS + writingKeyHex);
	t.ok(registryEntry, 'writer registry entry exists');
	t.ok(
		b4a.equals(registryEntry.value, addressBuffer),
		'writer registry links writing key to node address'
	);

	const writersLengthEntry = await base.view.get(EntryType.WRITERS_LENGTH);
	t.ok(writersLengthEntry, 'writers length entry exists');
	const writersLength = lengthEntryUtils.decodeBE(writersLengthEntry.value);
	t.is(writersLength, expectedWriterIndex + 1, 'writers length increments');

	const indexEntry = await base.view.get(`${EntryType.WRITERS_INDEX}${expectedWriterIndex}`);
	t.ok(indexEntry, 'writers index entry stored');
	t.ok(b4a.equals(indexEntry.value, addressBuffer), 'writer index stores node address');
}

export async function assertValidatorReward(t, validatorPeer, validatorBalanceBefore) {
	const validatorEntry = await validatorPeer.base.view.get(validatorPeer.wallet.address);
	t.ok(validatorEntry, 'validator entry exists');
	const decodedValidator = nodeEntryUtils.decode(validatorEntry.value);
	t.ok(decodedValidator, 'validator entry decodes');
	const initialBalance = toBalance(validatorBalanceBefore);
	const expected = initialBalance.add(BALANCE_FEE.percentage(PERCENT_75));
	const afterBalance = toBalance(decodedValidator.balance);
	t.ok(afterBalance, 'validator balance decodes');
	t.ok(
		b4a.equals(afterBalance.value, expected.value),
		'validator receives 75% of the writer fee'
	);
}

async function assertReplayProtection(t, base, payload) {
	const decodedPayload = safeDecodeApplyOperation(payload);
	t.ok(decodedPayload, 'addWriter payload decodes');
	const txKey = decodedPayload?.rao?.tx?.toString('hex');
	t.ok(txKey, 'addWriter tx hash extracted');
	const txEntry = await base.view.get(txKey);
	t.ok(txEntry, 'addWriter transaction recorded for replay protection');
}

export async function assertWriterRemovalState(
	t,
	context,
	{
		readerPeer = selectWriterPeer(context),
		validatorPeer = context.adminBootstrap,
		writerKeyBuffer = null,
		expectedBalanceBuffer = null,
		expectedLicenseBuffer = null,
		payload = null,
		skipSync = false
	} = {}
) {
	const writerAddress = readerPeer.wallet.address;
	const writingKey = writerKeyBuffer ?? readerPeer.base.local.key;
	await assertWriterDowngradedEntry(
		t,
		validatorPeer.base,
		writerAddress,
		writingKey,
		expectedBalanceBuffer,
		expectedLicenseBuffer
	);

	if (payload) {
		await assertReplayProtection(t, validatorPeer.base, payload);
	}

	if (!skipSync) {
		await context.sync();
		await assertWriterDowngradedEntry(
			t,
			readerPeer.base,
			writerAddress,
			writingKey,
			expectedBalanceBuffer,
			expectedLicenseBuffer
		);
	}
}

export async function applyWithRoleAccessBypass(context, invalidPayload) {
	const node = context.bootstrap ?? context.adminBootstrap;
	const state = node.state;
	const originalValidate = state.check.validateRoleAccessOperation;
	state.check.validateRoleAccessOperation = () => true;
	try {
		await node.base.append(invalidPayload);
		await node.base.update();
		await eventFlush();
	} finally {
		state.check.validateRoleAccessOperation = originalValidate;
	}
}

export async function applyWithMissingComponentBypass(context, invalidPayload) {
	const node = context.bootstrap ?? context.adminBootstrap;
	const state = node.state;
	const originalValidate = state.check.validateRoleAccessOperation;
	const originalHasOwn = Object.hasOwn;
	Object.hasOwn = (obj, prop) => {
		if (prop === 'vs') return false;
		return originalHasOwn(obj, prop);
	};
	state.check.validateRoleAccessOperation = () => true;
	try {
		await node.base.append(invalidPayload);
		await node.base.update();
		await eventFlush();
	} finally {
		state.check.validateRoleAccessOperation = originalValidate;
		Object.hasOwn = originalHasOwn;
	}
}

export async function applyWithRequesterEntryRemoval(context, invalidPayload, { peer = null } = {}) {
	const writerPeer = peer ?? selectWriterPeer(context);
	await withPeerEntryOverrideOnApply({
		context,
		peer: writerPeer,
		mutateEntry: () => null,
		fn: async node => {
			await node.base.append(invalidPayload);
			await node.base.update();
			await eventFlush();
		}
	});
}

export async function applyWithRequesterEntryCorruption(
	context,
	invalidPayload,
	{ peer = null } = {}
) {
	const writerPeer = peer ?? selectWriterPeer(context);
	await withPeerEntryOverrideOnApply({
		context,
		peer: writerPeer,
		mutateEntry: entry => {
			if (!entry?.value) {
				throw new Error('Requester entry corruption requires an existing entry.');
			}
			return { ...entry, value: b4a.alloc(1) };
		},
		fn: async node => {
			await node.base.append(invalidPayload);
			await node.base.update();
			await eventFlush();
		}
	});
}

export async function applyWithRequesterWriterKeyMismatch(context, invalidPayload) {
	const writerPeer = selectWriterPeer(context);
	await withPeerEntryOverrideOnApply({
		context,
		peer: writerPeer,
		mutateEntry: entry => {
			if (!entry?.value) {
				throw new Error('Requester writer key mutation requires an existing entry.');
			}
			const decoded = nodeEntryUtils.decode(entry.value);
			if (!decoded?.wk) {
				throw new Error('Requester writer key mutation requires a decodable entry.');
			}
			const mutatedWk = b4a.from(decoded.wk);
			mutatedWk[0] ^= 0xff;
			const mutatedEntry = nodeEntryUtils.setWritingKey(b4a.from(entry.value), mutatedWk);
			if (!mutatedEntry) {
				throw new Error('Failed to mutate requester writing key.');
			}
			return { ...entry, value: mutatedEntry };
		},
		fn: async node => {
			await node.base.append(invalidPayload);
			await node.base.update();
			await eventFlush();
		}
	});
}

export async function applyWithStakeEntryMutation(
	context,
	invalidPayload,
	mutateNodeEntryBuffer,
	mutateDecodedEntry = null
) {
	if (typeof mutateNodeEntryBuffer !== 'function') {
		throw new Error('Stake entry mutation requires a mutateNodeEntryBuffer function.');
	}

	const writerPeer = selectWriterPeer(context);
	const balancePrototype = Object.getPrototypeOf(BALANCE_ZERO);
	const originalUpdate = balancePrototype.update;
	const originalDecode = nodeEntryUtils.decode;
	let hasMutated = false;

	await withPeerEntryOverrideOnApply({
		context,
		peer: writerPeer,
		mutateEntry: entry => {
			if (!entry?.value) {
				throw new Error('Stake entry mutation requires an existing entry.');
			}
			return entry;
		},
		fn: async node => {
			balancePrototype.update = function patchedUpdate(nodeEntryBuffer) {
				const result = originalUpdate.call(this, nodeEntryBuffer);
				if (!hasMutated) {
					hasMutated = true;
					const mutatedEntry =
						mutateNodeEntryBuffer(result ?? nodeEntryBuffer, nodeEntryBuffer) ??
						result ??
						nodeEntryBuffer;

					if (mutateDecodedEntry && mutatedEntry && typeof mutatedEntry === 'object') {
						mutatedEntry[STAKE_ENTRY_MARK] = true;
					}

					return mutatedEntry;
				}
				return result;
			};

			if (typeof mutateDecodedEntry === 'function') {
				nodeEntryUtils.decode = function patchedDecode(buffer) {
					const decoded = originalDecode(buffer);
					if (buffer?.[STAKE_ENTRY_MARK]) {
						const mutatedDecoded = mutateDecodedEntry(decoded, buffer);
						if (mutatedDecoded && typeof mutatedDecoded === 'object') {
							return mutatedDecoded;
						}
					}
					return decoded;
				};
			}

			try {
				await node.base.append(invalidPayload);
				await node.base.update();
				await eventFlush();
			} finally {
				balancePrototype.update = originalUpdate;
				nodeEntryUtils.decode = originalDecode;
			}
		}
	});
}

async function assertWriterDowngradedEntry(
	t,
	base,
	address,
	writingKey,
	expectedBalanceBuffer,
	expectedLicenseBuffer
) {
	const entry = await base.view.get(address);
	t.ok(entry, 'writer node entry exists');
	const decoded = nodeEntryUtils.decode(entry.value);
	t.ok(decoded, 'writer node entry decodes');
	t.is(decoded.isWhitelisted, true, 'node remains whitelisted');
	t.is(decoded.isWriter, false, 'writer role removed');
	t.is(decoded.isIndexer, false, 'node not promoted to indexer');
	t.ok(b4a.equals(decoded.wk, writingKey), 'writer key preserved on entry');
	t.ok(
		b4a.equals(decoded.stakedBalance, BALANCE_ZERO.value),
		'writer staked balance cleared'
	);
	t.ok(!b4a.equals(decoded.license, ZERO_LICENSE), 'license retained after downgrade');
	if (expectedLicenseBuffer) {
		t.ok(
			b4a.equals(decoded.license, expectedLicenseBuffer),
			'writer license remains unchanged after downgrade'
		);
	}
	if (expectedBalanceBuffer) {
		t.ok(
			b4a.equals(decoded.balance, expectedBalanceBuffer),
			'writer liquid balance matches expected amount after downgrade'
		);
	}
    const addressBuffer = addressUtils.addressToBuffer(address, config.addressPrefix);
	const writerRegistryEntry = await base.view.get(
		EntryType.WRITER_ADDRESS + writingKey.toString('hex')
	);
	t.ok(writerRegistryEntry, 'writer registry entry persists for ownership tracking');
	t.ok(
		b4a.equals(writerRegistryEntry.value, addressBuffer),
		'writer registry continues to link downgraded node to its previous writing key'
	);
}

export const defaultWriterFunding = DEFAULT_WRITER_FUNDING;

export default {
	setupAddWriterScenario,
	selectWriterPeer,
	selectValidatorPeerWithoutEntry,
	buildAddWriterPayload,
	buildAddWriterPayloadWithTxValidity,
	buildRemoveWriterPayload,
	assertAddWriterSuccessState,
	assertAddWriterFailureState,
	assertWriterRemovalState,
	assertValidatorReward,
	applyWithRoleAccessBypass,
	applyWithMissingComponentBypass,
	applyWithRequesterEntryRemoval,
	applyWithRequesterEntryCorruption,
	applyWithRequesterWriterKeyMismatch,
	applyWithRequesterRoleOverride,
	mutateValidatorEntry,
	defaultWriterFunding
};

function assertWritableNode(node) {
	if (!node.base.view.batch) {
		throw new Error('Validator entry mutation requires a writable node.');
	}
	return node;
}

async function writeValidatorEntry(base, key, value) {
	const batch = base.view.batch();
	await batch.put(key, value);
	await batch.flush();
}

async function withPeerEntryOverrideOnApply({
	context,
	peer,
	selectNode = defaultSelectNode,
	mutateEntry,
	fn
}) {
	if (typeof mutateEntry !== 'function') {
		throw new Error('Peer entry override requires a mutateEntry function.');
	}

	const targetPeer = peer ?? selectWriterPeer(context);
	if (!targetPeer?.wallet?.address) {
		throw new Error('Peer entry override requires a target peer.');
	}

	const node = assertWritableNode(selectNode(context));
	const base = node.base;
	const targetAddress = targetPeer.wallet.address;
    const targetBuffer = addressUtils.addressToBuffer(targetAddress, config.addressPrefix);
	const originalApply = base._handlers.apply;

	base._handlers.apply = async function patchedApply(nodes, view, baseCtx) {
		const originalBatch = view.batch;
		view.batch = function patchedBatch(...args) {
			const batch = originalBatch.apply(this, args);
			if (!batch?.get) {
				return batch;
			}

			const originalGet = batch.get.bind(batch);
			batch.get = async key => {
				if (isTargetKey(key, targetAddress, targetBuffer)) {
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
		await fn(node, targetPeer);
	} finally {
		base._handlers.apply = originalApply;
	}
}

function isTargetKey(key, targetAddressString, targetAddressBuffer) {
	if (typeof key === 'string') {
		return key === targetAddressString;
	}
	if (b4a.isBuffer(key) && targetAddressBuffer) {
		return b4a.equals(key, targetAddressBuffer);
	}
	return false;
}

export async function applyWithRequesterRoleOverride(context, invalidPayload, role) {
	const writerPeer = selectWriterPeer(context);
	await withPeerEntryOverrideOnApply({
		context,
		peer: writerPeer,
		mutateEntry: entry => {
			if (!entry?.value) {
				throw new Error('Requester role override requires an existing entry.');
			}
			const mutatedValue = nodeEntryUtils.setRole(b4a.from(entry.value), role);
			if (!mutatedValue) {
				throw new Error('Failed to mutate requester role entry.');
			}
			return { ...entry, value: mutatedValue };
		},
		fn: async node => {
			await node.base.append(invalidPayload);
			await node.base.update();
			await eventFlush();
		}
	});
}

export async function mutateValidatorEntry({
	context,
	selectNode = defaultSelectNode,
	mutateValue,
	fn
}) {
	const node = assertWritableNode(selectNode(context));
	const validatorAddress = node.wallet.address;
	const originalEntry = await node.base.view.get(validatorAddress);
	if (!originalEntry?.value) {
		throw new Error('Validator entry mutation requires an existing validator entry.');
	}

	const originalValue = b4a.from(originalEntry.value);
	const mutatedValue = mutateValue(b4a.from(originalValue));
	if (!b4a.isBuffer(mutatedValue) || mutatedValue.length === 0) {
		throw new Error('Validator entry mutation must return a non-empty buffer.');
	}

	await writeValidatorEntry(node.base, validatorAddress, mutatedValue);

	try {
		await fn(node);
	} finally {
		await writeValidatorEntry(node.base, validatorAddress, originalValue);
	}
}

function defaultSelectNode(context) {
	return context.adminBootstrap ?? context.bootstrap ?? context.peers?.[0] ?? null;
}

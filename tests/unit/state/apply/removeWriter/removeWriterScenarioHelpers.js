import b4a from 'b4a';
import {
	setupAddWriterScenario,
	selectWriterPeer,
	buildRemoveWriterPayload,
	assertWriterRemovalState,
	assertValidatorReward,
	promotePeerToWriter
} from '../addWriter/addWriterScenarioHelpers.js';
import { applyStateMessageFactory } from '../../../../../src/messages/state/applyStateMessageFactory.js';
import { safeEncodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import nodeEntryUtils from '../../../../../src/core/state/utils/nodeEntry.js';
import addressUtils from '../../../../../src/core/state/utils/address.js';
import { toBalance, BALANCE_FEE, BALANCE_TO_STAKE } from '../../../../../src/core/state/utils/balance.js';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import { safeDecodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import { EntryType } from '../../../../../src/utils/constants.js';
import { config } from '../../../../helpers/config.js';

export async function setupRemoveWriterScenario(t, options = {}) {
	const context = await setupAddWriterScenario(t, options);
	const writerPeer = selectWriterPeer(context);
	await promotePeerToWriter(t, context, { readerPeer: writerPeer });
	return context;
}

export async function assertRemoveWriterSuccessState(
	t,
	context,
	{
		writerPeer = selectWriterPeer(context),
		validatorPeer = context.adminBootstrap,
		writerEntryBefore,
		validatorEntryBefore,
		payload,
		skipSync = false
	} = {}
) {
	if (!writerEntryBefore?.value) {
		throw new Error('assertRemoveWriterSuccessState requires the writer entry before removal.');
	}
	if (!validatorEntryBefore?.value) {
		throw new Error('assertRemoveWriterSuccessState requires the validator entry before removal.');
	}
	if (!payload) {
		throw new Error('assertRemoveWriterSuccessState requires the removeWriter payload.');
	}

	const decodedWriterBefore = nodeEntryUtils.decode(writerEntryBefore.value);
	if (!decodedWriterBefore) {
		throw new Error('Failed to decode writer entry before removal.');
	}

	const writerBalanceBefore = toBalance(decodedWriterBefore.balance);
	t.ok(writerBalanceBefore, 'writer balance before removeWriter decodes');
	if (!writerBalanceBefore) return;

	const writerStakedBefore = toBalance(decodedWriterBefore.stakedBalance);
	t.ok(writerStakedBefore, 'writer staked balance before removeWriter decodes');
	if (!writerStakedBefore) return;

	const balanceAfterFee = writerBalanceBefore.sub(BALANCE_FEE);
	t.ok(balanceAfterFee, 'writer balance after deducting removeWriter fee computed');
	if (!balanceAfterFee) return;

	const expectedFinalBalance = balanceAfterFee.add(writerStakedBefore);
	t.ok(expectedFinalBalance, 'writer balance after unstaking computed');
	if (!expectedFinalBalance) return;

	await assertWriterRemovalState(t, context, {
		readerPeer: writerPeer,
		validatorPeer,
		writerKeyBuffer: decodedWriterBefore.wk,
		expectedBalanceBuffer: expectedFinalBalance.value,
		expectedLicenseBuffer: decodedWriterBefore.license,
		payload,
		skipSync
	});

	const decodedValidatorBefore = nodeEntryUtils.decode(validatorEntryBefore.value);
	if (!decodedValidatorBefore) {
		throw new Error('Failed to decode validator entry before removeWriter.');
	}

	await assertValidatorReward(t, validatorPeer, b4a.from(decodedValidatorBefore.balance));

	await assertPayloadProcessedByValidator(t, payload, validatorPeer.wallet.address);
}

export async function assertRemoveWriterFailureState(t, context, { skipSync = false } = {}) {
	const writerPeer = selectWriterPeer(context);
	const validatorPeer = context.adminBootstrap;
	const expected = deriveActiveWriterState(context);

	await assertWriterStillPromoted(t, validatorPeer.base, writerPeer, expected);

	if (!skipSync) {
		await context.sync();
		await assertWriterStillPromoted(t, writerPeer.base, writerPeer, expected);
	}
}

function deriveActiveWriterState(context) {
	const writerInitialBalance = context.addWriterScenario?.writerInitialBalance;
	if (!writerInitialBalance) {
		throw new Error('removeWriter scenarios require writerInitialBalance buffer.');
	}
	const initialBalance = toBalance(writerInitialBalance);
	if (!initialBalance) {
		throw new Error('Failed to decode writerInitialBalance buffer.');
	}
	const balanceAfterFee = initialBalance.sub(BALANCE_FEE);
	if (!balanceAfterFee) {
		throw new Error('Failed to compute writer balance after fee deduction.');
	}
	const availableBalance = balanceAfterFee.sub(BALANCE_TO_STAKE);
	if (!availableBalance) {
		throw new Error('Failed to compute writer liquid balance after staking.');
	}
	return {
		balanceBuffer: availableBalance.value,
		stakedBalanceBuffer: BALANCE_TO_STAKE.value
	};
}

async function assertWriterStillPromoted(t, base, writerPeer, expected) {
	const writerAddress = writerPeer.wallet.address;
	const writerEntry = await base.view.get(writerAddress);
	t.ok(writerEntry, 'writer entry still exists after failed removeWriter');
	const decodedWriter = nodeEntryUtils.decode(writerEntry?.value);
	t.ok(decodedWriter, 'writer entry decodes after failed removeWriter');
	if (!decodedWriter) return;

	t.is(decodedWriter.isWhitelisted, true, 'writer stays whitelisted');
	t.is(decodedWriter.isWriter, true, 'writer role remains assigned');
	t.is(decodedWriter.isIndexer, false, 'writer does not become an indexer');
	t.ok(
		b4a.equals(decodedWriter.balance, expected.balanceBuffer),
		'writer balance remains untouched'
	);
	t.ok(
		b4a.equals(decodedWriter.stakedBalance, expected.stakedBalanceBuffer),
		'writer stake remains untouched'
	);

	const writerAddressBuffer = addressUtils.addressToBuffer(writerAddress, config.addressPrefix);
	const writingKeyHex = writerPeer.base.local.key.toString('hex');
	const registryKey = EntryType.WRITER_ADDRESS + writingKeyHex;
	const registryEntry = await base.view.get(registryKey);
	t.ok(registryEntry, 'writer registry entry persists after failed removeWriter');
	if (registryEntry?.value) {
		t.ok(
			b4a.equals(registryEntry.value, writerAddressBuffer),
			'writer registry still maps writing key to address'
		);
	}
}

async function assertPayloadProcessedByValidator(t, payload, expectedValidatorAddress) {
	const decodedOperation = safeDecodeApplyOperation(payload);
	t.ok(decodedOperation, 'removeWriter payload decodes');
	const validatorAddressBuffer = decodedOperation?.rao?.va;
	t.ok(validatorAddressBuffer, 'removeWriter payload carries validator address');
	if (!validatorAddressBuffer) return;
	const validatorAddress = addressUtils.bufferToAddress(validatorAddressBuffer, config.addressPrefix);
	t.ok(validatorAddress, 'removeWriter payload validator address decodes');
	if (!validatorAddress) return;
	t.is(
		validatorAddress,
		expectedValidatorAddress,
		'removeWriter payload signed by the expected validator'
	);
}

export { buildRemoveWriterPayload, selectWriterPeer };

export async function buildRemoveWriterPayloadWithTxValidity(context, mutatedTxValidity, options = {}) {
	if (!b4a.isBuffer(mutatedTxValidity)) {
		throw new Error('buildRemoveWriterPayloadWithTxValidity requires a tx validity buffer.');
	}
	const { readerPeer = selectWriterPeer(context), validatorPeer = context.adminBootstrap, writerKeyBuffer = null } = options;
	const writerKey = writerKeyBuffer ?? readerPeer.base.local.key;
	const partial = await applyStateMessageFactory(readerPeer.wallet, config)
		.buildPartialRemoveWriterMessage(
			readerPeer.wallet.address,
			writerKey.toString('hex'),
			mutatedTxValidity.toString('hex'),
			'json'
		);
	const payload = await applyStateMessageFactory(validatorPeer.wallet, config)
		.buildCompleteRemoveWriterMessage(
			partial.address,
			b4a.from(partial.rao.tx, 'hex'),
			mutatedTxValidity,
			b4a.from(partial.rao.iw, 'hex'),
			b4a.from(partial.rao.in, 'hex'),
			b4a.from(partial.rao.is, 'hex')
		);
	return safeEncodeApplyOperation(payload);
}

export async function snapshotDowngradedWriterEntry(context) {
	const writerPeer = selectWriterPeer(context);
	const validatorPeer = context.adminBootstrap;
	const entry = await validatorPeer.base.view.get(writerPeer.wallet.address);
	if (!entry?.value) {
		throw new Error('Snapshot requires writer entry after removeWriter.');
	}
	context.removeWriterScenario = {
		...(context.removeWriterScenario ?? {}),
		writerAddress: writerPeer.wallet.address,
		downgradedEntry: b4a.from(entry.value)
	};
}

export async function assertDowngradedWriterSnapshot(t, context, { skipSync = false } = {}) {
	const snapshot = context.removeWriterScenario?.downgradedEntry;
	const writerAddress = context.removeWriterScenario?.writerAddress;
	if (!snapshot || !writerAddress) {
		throw new Error('Missing writer snapshot for removeWriter replay assertion.');
	}

	await assertEntryMatchesSnapshot(t, context.adminBootstrap.base, writerAddress, snapshot);

	if (!skipSync) {
		await context.sync();
		const writerPeer = selectWriterPeer(context);
		await assertEntryMatchesSnapshot(t, writerPeer.base, writerAddress, snapshot);
	}
}

async function assertEntryMatchesSnapshot(t, base, address, snapshot) {
	const entry = await base.view.get(address);
	t.ok(entry, 'writer entry exists during replay assertion');
	t.ok(entry?.value, 'writer entry has a value during replay assertion');
	if (!entry?.value) return;
	t.ok(b4a.equals(entry.value, snapshot), 'writer entry remains unchanged after replay attempt');
}

export async function applyWithWriterRegistryRemoval(context, invalidPayload) {
	const writerPeer = selectWriterPeer(context);
	const writingKeyHex = writerPeer.base.local.key.toString('hex');
	await withWriterRegistryOverrideOnApply({
		context,
		writingKeyHex,
		getOverride: () => null,
		fn: async node => {
			await node.base.append(invalidPayload);
			await node.base.update();
			await eventFlush();
		}
	});
}

export async function applyWithWriterRegistryForeignAddress(context, invalidPayload, foreignPeer) {
	const writerPeer = selectWriterPeer(context);
	const writingKeyHex = writerPeer.base.local.key.toString('hex');
	const foreignAddress = foreignPeer?.wallet?.address;
	if (!foreignAddress) {
		throw new Error('Foreign registry override requires a peer with an address.');
	}
	const foreignEntry = {
		value: addressUtils.addressToBuffer(foreignAddress, config.addressPrefix)
	};
	await withWriterRegistryOverrideOnApply({
		context,
		writingKeyHex,
		getOverride: () => foreignEntry,
		fn: async node => {
			await node.base.append(invalidPayload);
			await node.base.update();
			await eventFlush();
		}
	});
}

async function withWriterRegistryOverrideOnApply({
	context,
	writingKeyHex,
	getOverride,
	fn,
	selectNode = defaultSelectNode
}) {
	if (!writingKeyHex) {
		throw new Error('Writer registry override requires a writing key.');
	}
	const node = selectNode(context);
	const base = node.base;
	const registryKey = EntryType.WRITER_ADDRESS + writingKeyHex;
	const originalApply = base._handlers.apply;

	base._handlers.apply = async function patchedApply(nodes, view, baseCtx) {
		const originalBatch = view.batch;
		view.batch = function patchedBatch(...args) {
			const batch = originalBatch.apply(this, args);
			if (!batch?.get) return batch;

			const originalGet = batch.get.bind(batch);
			batch.get = async key => {
				if (key === registryKey) {
					return getOverride();
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
		await fn(node);
	} finally {
		base._handlers.apply = originalApply;
	}
}

function defaultSelectNode(context) {
	return context.adminBootstrap ?? context.bootstrap ?? context.peers?.[0] ?? null;
}

export async function applyWithRequesterRoleMutationFailure(context, invalidPayload) {
	const node = context.adminBootstrap ?? context.bootstrap;
	if (!node?.base) {
		throw new Error('Role mutation failure scenario requires a writable node.');
	}
	const originalSetRole = nodeEntryUtils.setRole;
	let shouldFailNextSetRole = true;
	nodeEntryUtils.setRole = function patchedSetRole(...args) {
		if (shouldFailNextSetRole) {
			shouldFailNextSetRole = false;
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

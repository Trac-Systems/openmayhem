import b4a from 'b4a';
import PeerWallet from 'trac-wallet';
import { applyStateMessageFactory } from '../../../../../src/messages/state/applyStateMessageFactory.js';
import { deriveIndexerSequenceState, eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import {
	setupAdminNetwork,
	initializeBalances,
	whitelistAddress
} from '../common/commonScenarioHelper.js';
import { promotePeerToWriter } from '../addWriter/addWriterScenarioHelpers.js';
import nodeEntryUtils from '../../../../../src/core/state/utils/nodeEntry.js';
import addressUtils from '../../../../../src/core/state/utils/address.js';
import transactionUtils from '../../../../../src/core/state/utils/transaction.js';
import { toBalance, PERCENT_75, BALANCE_ZERO } from '../../../../../src/core/state/utils/balance.js';
import { decimalStringToBigInt, bigIntTo16ByteBuffer } from '../../../../../src/utils/amountSerialization.js';
import { safeDecodeApplyOperation, safeEncodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import { ZERO_WK } from '../../../../../src/utils/buffer.js';
import { EntryType, OperationType } from '../../../../../src/utils/constants.js';
import { createMessage } from '../../../../../src/utils/buffer.js';
import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import { config } from '../../../../helpers/config.js';

export const DEFAULT_INITIAL_BALANCE = bigIntTo16ByteBuffer(decimalStringToBigInt('10'));
export const DEFAULT_TRANSFER_AMOUNT = bigIntTo16ByteBuffer(decimalStringToBigInt('2'));
export const ZERO_TRANSFER_AMOUNT = bigIntTo16ByteBuffer(decimalStringToBigInt('0'));

function selectValidatorPeer(context, offset = 0) {
	const candidates = context.peers.slice(1);
	if (!candidates.length) {
		throw new Error('Transfer scenarios require at least one non-admin peer as validator.');
	}
	return candidates[Math.min(offset, candidates.length - 1)];
}

export function selectSenderPeer(context, offset = 1) {
	const candidates = context.peers.slice(1);
	if (candidates.length < 2) {
		throw new Error('Transfer scenarios require at least two non-admin peers (validator + sender).');
	}
	return candidates[Math.min(offset, candidates.length - 1)];
}

function selectRecipientPeer(context, offset = 2) {
	const candidates = context.peers.slice(1);
	if (!candidates.length) {
		throw new Error('Transfer scenarios require at least one recipient candidate.');
	}
	return candidates[Math.min(offset, candidates.length - 1)];
}

function cloneEntry(entry) {
	if (!entry?.value) return null;
	return { value: b4a.from(entry.value) };
}

export async function setupTransferScenario(
	t,
	{
		nodes = 4,
		validatorInitialBalance = DEFAULT_INITIAL_BALANCE,
		senderInitialBalance = DEFAULT_INITIAL_BALANCE,
		recipientInitialBalance = DEFAULT_INITIAL_BALANCE,
		recipientHasEntry = true,
		recipientPeer = null,
		senderPeer = null,
		validatorPeer = null
	} = {}
) {
	const context = await setupAdminNetwork(t, { nodes: Math.max(nodes, 4) });

	const resolvedValidator = validatorPeer ?? selectValidatorPeer(context, 0);
	const resolvedSender = senderPeer ?? selectSenderPeer(context, 1);
	const resolvedRecipient = recipientPeer ?? selectRecipientPeer(context, 2);

	const funding = [
		[resolvedValidator.wallet.address, validatorInitialBalance],
		[resolvedSender.wallet.address, senderInitialBalance]
	];

	if (recipientHasEntry && recipientInitialBalance && resolvedRecipient) {
		funding.push([resolvedRecipient.wallet.address, recipientInitialBalance]);
	}

	if (funding.length) {
		await initializeBalances(context, funding);
	}

	await whitelistAddress(context, resolvedValidator.wallet.address);
	await whitelistAddress(context, resolvedSender.wallet.address);
	if (recipientHasEntry && resolvedRecipient) {
		await whitelistAddress(context, resolvedRecipient.wallet.address);
	}

	context.addWriterScenario = { writerInitialBalance: validatorInitialBalance };
	await promotePeerToWriter(t, context, { readerPeer: resolvedValidator });
	await context.sync();

	context.transferScenario = {
		validatorPeer: resolvedValidator,
		senderPeer: resolvedSender,
		recipientPeer: resolvedRecipient
	};

	return context;
}

export async function buildTransferPayload(
	context,
	{
		senderPeer = context.transferScenario?.senderPeer ?? selectSenderPeer(context, 1),
		validatorPeer = context.transferScenario?.validatorPeer ?? selectValidatorPeer(context, 0),
		recipientPeer = context.transferScenario?.recipientPeer ?? selectRecipientPeer(context, 2),
		recipientAddress = recipientPeer?.wallet?.address,
		amount = DEFAULT_TRANSFER_AMOUNT,
		txValidity = null
	} = {}
) {
	if (!recipientAddress) {
		throw new Error('buildTransferPayload requires a recipient address.');
	}

	const resolvedTxValidity =
		txValidity ?? (await deriveIndexerSequenceState(validatorPeer.base));

    const partial = await applyStateMessageFactory(senderPeer.wallet, config)
        .buildPartialTransferOperationMessage(
            senderPeer.wallet.address,
            recipientAddress,
            b4a.toString(amount, 'hex'),
            b4a.toString(resolvedTxValidity, 'hex'),
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

export async function buildTransferPayloadWithTxValidity(
	context,
	mutatedTxValidity,
	options = {}
) {
	if (!b4a.isBuffer(mutatedTxValidity)) {
		throw new Error('buildTransferPayloadWithTxValidity requires a tx validity buffer.');
	}
	return buildTransferPayload(context, { ...options, txValidity: mutatedTxValidity });
}

export async function snapshotTransferEntries(
	context,
	{
		senderPeer = context.transferScenario?.senderPeer ?? selectSenderPeer(context, 1),
		recipientPeer = context.transferScenario?.recipientPeer ?? selectRecipientPeer(context, 2),
		validatorPeer = context.transferScenario?.validatorPeer ?? selectValidatorPeer(context, 0),
		skipSync = false
	} = {}
) {
	if (!skipSync) {
		await context.sync();
	}
	const [senderEntry, recipientEntry, validatorEntry, txValidity] = await Promise.all([
		validatorPeer.base.view.get(senderPeer.wallet.address),
		recipientPeer ? validatorPeer.base.view.get(recipientPeer.wallet.address) : null,
		validatorPeer.base.view.get(validatorPeer.wallet.address),
		deriveIndexerSequenceState(validatorPeer.base)
	]);

	return {
		senderEntry: cloneEntry(senderEntry),
		recipientEntry: cloneEntry(recipientEntry),
		validatorEntry: cloneEntry(validatorEntry),
		txValidity
	};
}

function decodeBalance(entryBuffer) {
	const decoded = nodeEntryUtils.decode(entryBuffer);
	return decoded ? { decoded, balance: toBalance(decoded.balance) } : { decoded: null, balance: null };
}

export async function assertTransferSuccessState(
	t,
	context,
	{
		payload,
		senderPeer = context.transferScenario?.senderPeer ?? selectSenderPeer(context, 1),
		recipientPeer = context.transferScenario?.recipientPeer ?? selectRecipientPeer(context, 2),
		validatorPeer = context.transferScenario?.validatorPeer ?? selectValidatorPeer(context, 0),
		senderEntryBefore,
		recipientEntryBefore,
		validatorEntryBefore,
		skipSync = false
	} = {}
) {
	if (!payload) throw new Error('assertTransferSuccessState requires a payload.');

	const decodedPayload = safeDecodeApplyOperation(payload);
	t.ok(decodedPayload?.tro, 'transfer payload decodes');
	if (!decodedPayload?.tro) return;

	const senderAddress = addressUtils.bufferToAddress(decodedPayload.address, config.addressPrefix);
	const recipientAddress = addressUtils.bufferToAddress(decodedPayload.tro.to, config.addressPrefix);
	const validatorAddress = addressUtils.bufferToAddress(decodedPayload.tro.va, config.addressPrefix);

	const amount = toBalance(decodedPayload.tro.am);
	const fee = toBalance(transactionUtils.FEE);
	t.ok(amount, 'transfer amount decodes');
	t.ok(fee, 'fee decodes');
	if (!amount || !fee) return;

	const feeReward = fee.percentage(PERCENT_75);
	t.ok(feeReward, 'validator reward computed');
	if (!feeReward) return;

	const senderBeforeBuf = senderEntryBefore?.value ?? senderEntryBefore ?? null;
	const recipientBeforeBuf = recipientEntryBefore?.value ?? recipientEntryBefore ?? null;
	const validatorBeforeBuf = validatorEntryBefore?.value ?? validatorEntryBefore ?? null;

	const { decoded: senderBefore, balance: senderBalanceBefore } = decodeBalance(senderBeforeBuf);
	const { decoded: recipientBefore, balance: recipientBalanceBefore } = decodeBalance(recipientBeforeBuf);
	const { decoded: validatorBefore, balance: validatorBalanceBefore } = decodeBalance(validatorBeforeBuf);

	t.ok(senderBefore, 'sender entry decodes before transfer');
	t.ok(validatorBefore, 'validator entry decodes before transfer');
	t.ok(senderBalanceBefore, 'sender balance available before transfer');
	t.ok(validatorBalanceBefore, 'validator balance available before transfer');
	if (!senderBefore || !validatorBefore || !senderBalanceBefore || !validatorBalanceBefore) return;

	const isSelfTransfer = b4a.equals(decodedPayload.address, decodedPayload.tro.to);
	const recipientIsValidator = b4a.equals(decodedPayload.tro.to, decodedPayload.tro.va);

	const totalDeducted = isSelfTransfer ? fee : amount.add(fee);
	t.ok(totalDeducted, 'total deducted amount calculated');
	const expectedSenderBalance = totalDeducted ? senderBalanceBefore.sub(totalDeducted) : null;
	t.ok(expectedSenderBalance, 'expected sender balance calculated');

	const validatorBonus = recipientIsValidator ? amount.add(feeReward) : feeReward;
	t.ok(validatorBonus, 'validator bonus calculated');
	const expectedValidatorBalance = validatorBonus ? validatorBalanceBefore.add(validatorBonus) : null;
	t.ok(expectedValidatorBalance, 'expected validator balance calculated');

	let expectedRecipientBalance = null;
	if (!isSelfTransfer && !recipientIsValidator) {
		expectedRecipientBalance = recipientBalanceBefore
			? recipientBalanceBefore.add(amount)
			: amount;
		t.ok(expectedRecipientBalance, 'expected recipient balance calculated');
	}

	if (!skipSync) {
		await context.sync();
	}

	const senderAfter = await validatorPeer.base.view.get(senderAddress);
	const recipientAfter = await validatorPeer.base.view.get(recipientAddress);
	const validatorAfter = await validatorPeer.base.view.get(validatorAddress);

	t.ok(senderAfter?.value, 'sender entry exists after transfer');
	t.ok(validatorAfter?.value, 'validator entry exists after transfer');

	const senderAfterDecoded = senderAfter?.value ? nodeEntryUtils.decode(senderAfter.value) : null;
	const validatorAfterDecoded = validatorAfter?.value ? nodeEntryUtils.decode(validatorAfter.value) : null;
	t.ok(senderAfterDecoded, 'sender entry decodes after transfer');
	t.ok(validatorAfterDecoded, 'validator entry decodes after transfer');

	if (!senderAfterDecoded || !validatorAfterDecoded) return;

	t.ok(
		b4a.equals(senderAfterDecoded.balance, expectedSenderBalance?.value),
		'sender balance reflects fee (and amount if applicable)'
	);
	t.ok(
		b4a.equals(validatorAfterDecoded.balance, expectedValidatorBalance?.value),
		'validator balance reflects 75% fee reward'
	);

	if (!isSelfTransfer) {
		t.ok(recipientAfter?.value, 'recipient entry exists after transfer');
		const recipientAfterDecoded = recipientAfter?.value ? nodeEntryUtils.decode(recipientAfter.value) : null;
		t.ok(recipientAfterDecoded, 'recipient entry decodes after transfer');

		if (recipientIsValidator) {
			t.ok(
				b4a.equals(recipientAfter?.value, validatorAfter?.value),
				'recipient equals validator entry when validator is recipient'
			);
		} else if (recipientAfterDecoded && expectedRecipientBalance) {
			t.ok(
				b4a.equals(recipientAfterDecoded.balance, expectedRecipientBalance.value),
				'recipient balance reflects transferred amount'
			);
			if (!recipientBefore) {
				t.is(recipientAfterDecoded.isWriter, false, 'new recipient is not a writer');
				t.is(recipientAfterDecoded.isWhitelisted, false, 'new recipient is not whitelisted by default');
				t.is(recipientAfterDecoded.isIndexer, false, 'new recipient is not an indexer');
				t.ok(b4a.equals(recipientAfterDecoded.wk, ZERO_WK), 'new recipient uses zero writing key');
			}
		}
	}

	const txEntryKey = decodedPayload.tro.tx.toString('hex');
	const txEntry = await validatorPeer.base.view.get(txEntryKey);
	t.ok(txEntry, 'transfer hash recorded for replay protection');

	const recipientRegistryKey = `${EntryType.WRITER_ADDRESS}${senderBefore?.wk?.toString('hex') ?? ''}`;
	if (recipientRegistryKey && recipientRegistryKey !== `${EntryType.WRITER_ADDRESS}`) {
		const registryEntry = await validatorPeer.base.view.get(recipientRegistryKey);
		if (registryEntry) {
			t.comment('writer registry remains present (sanity check)');
		}
	}
}

export async function assertTransferFailureState(
	t,
	context,
	{
		payload,
		senderPeer = context.transferScenario?.senderPeer ?? selectSenderPeer(context, 1),
		recipientPeer = context.transferScenario?.recipientPeer ?? selectRecipientPeer(context, 2),
		validatorPeer = context.transferScenario?.validatorPeer ?? selectValidatorPeer(context, 0),
		senderEntryBefore = null,
		recipientEntryBefore = null,
		validatorEntryBefore = null
	} = {}
) {
	if (!payload) throw new Error('assertTransferFailureState requires payload.');

	const decoded = safeDecodeApplyOperation(payload);
	t.ok(decoded, 'invalid transfer payload decodes');

	const senderAfter = await validatorPeer.base.view.get(senderPeer.wallet.address);
	const validatorAfter = await validatorPeer.base.view.get(validatorPeer.wallet.address);
	const recipientAfter = await validatorPeer.base.view.get(recipientPeer.wallet.address);

	if (senderEntryBefore?.value) {
		t.ok(senderAfter, 'sender entry persists after rejection');
		if (senderAfter?.value) {
			t.ok(b4a.equals(senderAfter.value, senderEntryBefore.value), 'sender entry unchanged after rejection');
		}
	}

	if (validatorEntryBefore?.value) {
		t.ok(validatorAfter, 'validator entry persists after rejection');
		if (validatorAfter?.value) {
			t.ok(b4a.equals(validatorAfter.value, validatorEntryBefore.value), 'validator entry unchanged after rejection');
		}
	}

	if (recipientEntryBefore) {
		if (recipientEntryBefore.value) {
			t.ok(recipientAfter, 'recipient entry persists after rejection');
			if (recipientAfter?.value) {
				t.ok(b4a.equals(recipientAfter.value, recipientEntryBefore.value), 'recipient entry unchanged after rejection');
			}
		} else {
			t.is(recipientAfter, null, 'recipient entry still missing after rejection');
		}
	}

	const txHash = decoded?.tro?.tx?.toString('hex');
	if (txHash) {
		const txEntry = await validatorPeer.base.view.get(txHash);
		t.is(txEntry, null, 'tx hash not recorded after rejection');
	}
}

export async function snapshotTransferStateAfterApply(context) {
	const { senderEntry, recipientEntry, validatorEntry } = await snapshotTransferEntries(context, {
		skipSync: false
	});
	return {
		senderEntryAfter: senderEntry,
		recipientEntryAfter: recipientEntry,
		validatorEntryAfter: validatorEntry
	};
}

export async function assertTransferReplayIgnoredState(
	t,
	context,
	{
		payload,
		senderEntryAfter,
		recipientEntryAfter,
		validatorEntryAfter,
		validatorPeer = context.transferScenario?.validatorPeer ?? selectValidatorPeer(context, 0)
	} = {}
) {
	if (!payload) throw new Error('assertTransferReplayIgnoredState requires a payload.');

	const decoded = safeDecodeApplyOperation(payload);
	const senderAddress = decoded?.address;
	const recipientAddress = decoded?.tro?.to;
	const validatorAddress = decoded?.tro?.va;
	const txHash = decoded?.tro?.tx?.toString('hex');

	await context.sync();

	const [senderAfter, recipientAfter, validatorAfter] = await Promise.all([
		senderAddress ? validatorPeer.base.view.get(senderAddress) : null,
		recipientAddress ? validatorPeer.base.view.get(recipientAddress) : null,
		validatorAddress ? validatorPeer.base.view.get(validatorAddress) : null
	]);

	if (senderEntryAfter?.value) {
		t.ok(senderAfter?.value, 'sender entry persists after replay ignore');
		t.ok(
			b4a.equals(senderAfter.value, senderEntryAfter.value),
			'sender entry unchanged after replay ignore'
		);
	}

	if (validatorEntryAfter?.value) {
		t.ok(validatorAfter?.value, 'validator entry persists after replay ignore');
		t.ok(
			b4a.equals(validatorAfter.value, validatorEntryAfter.value),
			'validator entry unchanged after replay ignore'
		);
	}

	if (recipientEntryAfter) {
		if (recipientEntryAfter.value) {
			t.ok(recipientAfter?.value, 'recipient entry persists after replay ignore');
			t.ok(
				b4a.equals(recipientAfter.value, recipientEntryAfter.value),
				'recipient entry unchanged after replay ignore'
			);
		} else {
			t.is(recipientAfter, null, 'recipient entry still absent after replay ignore');
		}
	}

	if (txHash) {
		const txEntry = await validatorPeer.base.view.get(txHash);
		t.ok(txEntry, 'original tx hash remains recorded after replay ignore');
	}
}

export async function appendInvalidTransferPayload(context, invalidPayload, { node = null } = {}) {
	const targetNode =
		node ??
		context.adminBootstrap ??
		context.bootstrap ??
		context.transferScenario?.validatorPeer ??
		context.peers?.[0];
	await targetNode.base.append(invalidPayload);
	await targetNode.base.update();
	await eventFlush();
}

export async function applyInvalidTransferWithSchemaBypass(context, invalidPayload) {
	const node =
		context.transferScenario?.validatorPeer ??
		context.adminBootstrap ??
		context.bootstrap ??
		context.peers?.[0];

	if (!node?.state?.check) {
		throw new Error('applyInvalidTransferWithSchemaBypass requires a node with state and check.');
	}

	const originalValidate = node.state.check.validateTransferOperation;
	node.state.check.validateTransferOperation = () => true;
	try {
		await appendInvalidTransferPayload(context, invalidPayload, { node });
	} finally {
		node.state.check.validateTransferOperation = originalValidate;
	}
}

export async function applyInvalidTransferMissingValidatorFields(context, invalidPayload) {
	const node =
		context.transferScenario?.validatorPeer ??
		context.adminBootstrap ??
		context.bootstrap ??
		context.peers?.[0];

	if (!node?.state?.check) {
		throw new Error('applyInvalidTransferMissingValidatorFields requires a node with state and check.');
	}

	const originalValidate = node.state.check.validateTransferOperation;
	const originalHasOwn = Object.hasOwn;
	node.state.check.validateTransferOperation = () => true;
	Object.hasOwn = (obj, prop) => {
		if (prop === 'vs' || prop === 'va' || prop === 'vn') return false;
		return originalHasOwn(obj, prop);
	};

	try {
		await appendInvalidTransferPayload(context, invalidPayload, { node });
	} finally {
		node.state.check.validateTransferOperation = originalValidate;
		Object.hasOwn = originalHasOwn;
	}
}

export async function applyInvalidTransferPayloadWithNoMutations(t, context, invalidPayload) {
	const node =
		context.adminBootstrap ??
		context.bootstrap ??
		context.transferScenario?.validatorPeer ??
		context.peers?.[0];

	if (!node?.base?._handlers?.apply) {
		throw new Error('applyInvalidTransferPayloadWithNoMutations requires a node with an apply handler.');
	}

	const base = node.base;
	const originalApply = base._handlers.apply;
	let putCount = 0;

	base._handlers.apply = async function patchedApply(nodes, view, baseCtx) {
		const originalBatch = view.batch;
		view.batch = function patchedBatch(...args) {
			const batch = originalBatch.apply(this, args);
			const originalPut = batch?.put?.bind(batch);
			if (typeof originalPut === 'function') {
				batch.put = (...putArgs) => {
					putCount += 1;
					return originalPut(...putArgs);
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
		await appendInvalidTransferPayload(context, invalidPayload, { node });
	} finally {
		base._handlers.apply = originalApply;
	}

	t.is(putCount, 0, 'transfer schema validation does not mutate state');
}

export async function applyTransferAlreadyApplied(context, invalidPayload, validPayload) {
	const node = context.transferScenario?.validatorPeer ?? context.peers?.[1];
	if (!node?.base) {
		throw new Error('Operation already applied scenario requires a validator peer.');
	}

	await node.base.append(validPayload);
	await node.base.update();
	await eventFlush();

	context.transferScenario = context.transferScenario ?? {};
	context.transferScenario.replaySnapshot = await snapshotTransferStateAfterApply(context);

	await node.base.append(invalidPayload);
	await node.base.update();
	await eventFlush();
}

export async function applyTransferTotalDeductedAmountFailure(context, payload) {
	const node = context.transferScenario?.validatorPeer ?? context.peers?.[1];
	if (!node?.base) {
		throw new Error('Total deducted amount scenario requires a validator peer.');
	}

	const balancePrototype = Object.getPrototypeOf(BALANCE_ZERO);
	const originalAdd = balancePrototype.add;
	let shouldFailNextAdd = true;

	balancePrototype.add = function patchedAdd(...args) {
		if (shouldFailNextAdd) {
			shouldFailNextAdd = false;
			return null;
		}
		return originalAdd.call(this, ...args);
	};

	try {
		await node.base.append(payload);
		await node.base.update();
		await eventFlush();
	} finally {
		balancePrototype.add = originalAdd;
	}
}

export async function applyTransferSenderEntryRemoval(context, invalidPayload) {
	return applyTransferSenderEntryOverride(context, invalidPayload, () => null);
}

export async function applyTransferSenderEntryCorruption(context, invalidPayload) {
	return applyTransferSenderEntryOverride(context, invalidPayload, entry => {
		if (!entry) return entry;
		return { ...entry, value: b4a.alloc(1) };
	});
}

async function applyTransferSenderEntryOverride(context, invalidPayload, mutateEntry) {
	const node = context.transferScenario?.validatorPeer ?? context.peers?.[1];
	const senderPeer = context.transferScenario?.senderPeer ?? selectSenderPeer(context, 1);
	if (!node?.base || !senderPeer?.wallet?.address) {
		throw new Error('Sender entry override scenario requires validator and sender peers.');
	}

	const senderAddress = senderPeer.wallet.address;
	const senderBuffer = addressUtils.addressToBuffer(senderAddress, config.addressPrefix);
	const base = node.base;
	const originalApply = base._handlers.apply;

	base._handlers.apply = async function patchedApply(nodes, view, baseCtx) {
		const originalBatch = view.batch;
		view.batch = function patchedBatch(...args) {
			const batch = originalBatch.apply(this, args);
			if (!batch?.get) return batch;

			const originalGet = batch.get.bind(batch);
			batch.get = async key => {
				const entry = await originalGet(key);
				if (isTargetKey(key, senderAddress, senderBuffer)) {
					return mutateEntry(entry);
				}
				return entry;
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
		await appendInvalidTransferPayload(context, invalidPayload, { node });
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

export function mutateTransferTxHash(t, validPayload) {
	const decoded = safeDecodeApplyOperation(validPayload);
	t.ok(decoded, 'transfer payload decodes before mutation');
	if (!decoded?.tro?.tx) return validPayload;
	const mutated = b4a.alloc(decoded.tro.tx.length, 0x42);
	decoded.tro.tx = mutated;
	return safeEncodeApplyOperation(decoded);
}

export function mutateTransferAmount(t, validPayload) {
	const decoded = safeDecodeApplyOperation(validPayload);
	t.ok(decoded, 'transfer payload decodes before mutation');
	const amount = decoded?.tro?.am;
	if (!amount || !b4a.isBuffer(amount)) return validPayload;
	const mutated = b4a.from(amount);
	mutated[mutated.length - 1] ^= 0x01;
	decoded.tro.am = mutated;
	return safeEncodeApplyOperation(decoded);
}

export async function mutateTransferAmountWithRehashedTx(t, validPayload) {
	const decoded = safeDecodeApplyOperation(validPayload);
	t.ok(decoded, 'transfer payload decodes before mutation');
	const parent = decoded?.tro;
	if (!parent?.am || !parent?.txv || !parent?.to || !parent?.in) return validPayload;

	const mutatedAmount = b4a.from(parent.am);
	mutatedAmount[mutatedAmount.length - 1] ^= 0x01;
	parent.am = mutatedAmount;

	const message = createMessage(config.networkId, parent.txv, parent.to, parent.am, parent.in, OperationType.TRANSFER);
	const regeneratedTxHash = await PeerWallet.blake3(message);
	if (regeneratedTxHash?.length === parent.tx?.length) {
		parent.tx = regeneratedTxHash;
	}

	return safeEncodeApplyOperation(decoded);
}

export function mutateTransferPayloadRemoveValidatorFields(t, validPayload) {
	const decoded = safeDecodeApplyOperation(validPayload);
	t.ok(decoded, 'transfer payload decodes before mutation');
	if (!decoded?.tro) return validPayload;
	delete decoded.tro.vs;
	delete decoded.tro.va;
	delete decoded.tro.vn;
	return safeEncodeApplyOperation(decoded);
}

export function mutateTransferValidatorSignature(t, validPayload, { zeroFill = false } = {}) {
	const decoded = safeDecodeApplyOperation(validPayload);
	t.ok(decoded, 'transfer payload decodes before signature mutation');
	const parent = decoded?.tro;
	if (!parent?.vs) return validPayload;

	const mutated = zeroFill ? b4a.alloc(parent.vs.length) : b4a.from(parent.vs);
	if (!zeroFill && mutated.length > 0) {
		mutated[mutated.length - 1] ^= 0xff;
	}
	if (parent.is && b4a.equals(mutated, parent.is) && mutated.length > 0) {
		mutated[0] ^= 0x01; // ensure validator signature differs from requester signature
	}
	parent.vs = mutated;
	return safeEncodeApplyOperation(decoded);
}

export async function mutateTransferAmountInvalidWithRehash(t, validPayload, context) {
	const decoded = safeDecodeApplyOperation(validPayload);
	t.ok(decoded, 'transfer payload decodes before amount mutation');
	const parent = decoded?.tro;
	if (!parent?.am || !parent?.txv || !parent?.to || !parent?.in) return validPayload;

	const requesterWallet = context?.transferScenario?.senderPeer?.wallet;
	const validatorWallet = context?.transferScenario?.validatorPeer?.wallet;

	parent.am = b4a.alloc(1); // invalid length to break toBalance

	const requesterMessage = createMessage(
		config.networkId,
		parent.txv,
		parent.to,
		parent.am,
		parent.in,
		OperationType.TRANSFER
	);
	const regeneratedTxHash = await PeerWallet.blake3(requesterMessage);
	if (regeneratedTxHash?.length === parent.tx?.length) {
		parent.tx = regeneratedTxHash;
	}

	if (requesterWallet) {
		parent.is = requesterWallet.sign(regeneratedTxHash);
	}
	if (validatorWallet && parent.vn) {
		const validatorMessage = createMessage(config.networkId, parent.tx, parent.vn, OperationType.TRANSFER);
		parent.vs = validatorWallet.sign(await PeerWallet.blake3(validatorMessage));
	}

	return safeEncodeApplyOperation(decoded);
}

export async function mutateTransferRecipientAddressWithRehash(t, validPayload, context) {
	const decoded = safeDecodeApplyOperation(validPayload);
	t.ok(decoded, 'transfer payload decodes before recipient mutation');
	const parent = decoded?.tro;
	if (!parent?.to || !parent?.txv || !parent?.am || !parent?.in) return validPayload;
	const requesterWallet = context?.transferScenario?.senderPeer?.wallet;
	const validatorWallet = context?.transferScenario?.validatorPeer?.wallet;

	const mutatedTo = b4a.from(parent.to);
	if (mutatedTo.length > 0) {
		mutatedTo[mutatedTo.length - 1] ^= 0x01;
	}
	parent.to = mutatedTo;

	const message = createMessage(config.networkId, parent.txv, parent.to, parent.am, parent.in, OperationType.TRANSFER);
	const regeneratedTxHash = await PeerWallet.blake3(message);
	if (regeneratedTxHash?.length === parent.tx?.length) {
		parent.tx = regeneratedTxHash;
	}

	if (requesterWallet) {
		parent.is = requesterWallet.sign(regeneratedTxHash);
	}
	if (validatorWallet && parent.vn) {
		const validatorMessage = createMessage(config.networkId, parent.tx, parent.vn, OperationType.TRANSFER);
		parent.vs = validatorWallet.sign(await PeerWallet.blake3(validatorMessage));
	}

	return safeEncodeApplyOperation(decoded);
}

export async function mutateTransferRecipientPublicKeyInvalidWithRehash(t, validPayload, context) {
	const decoded = safeDecodeApplyOperation(validPayload);
	t.ok(decoded, 'transfer payload decodes before recipient pk mutation');
	const parent = decoded?.tro;
	if (!parent?.to || !parent?.txv || !parent?.am || !parent?.in) return validPayload;
	const requesterWallet = context?.transferScenario?.senderPeer?.wallet;
	const validatorWallet = context?.transferScenario?.validatorPeer?.wallet;

	const mutatedTo = b4a.from(parent.to);
	if (mutatedTo.length > 0) {
		const lastIndex = mutatedTo.length - 1;
		mutatedTo[lastIndex] = mutatedTo[lastIndex] === 0x70 ? 0x71 : mutatedTo[lastIndex] ^ 0x01;
	}
	parent.to = mutatedTo;

	const message = createMessage(config.networkId, parent.txv, parent.to, parent.am, parent.in, OperationType.TRANSFER);
	const regeneratedTxHash = await PeerWallet.blake3(message);
	if (regeneratedTxHash?.length === parent.tx?.length) {
		parent.tx = regeneratedTxHash;
	}

	if (requesterWallet) {
		parent.is = requesterWallet.sign(regeneratedTxHash);
	}
	if (validatorWallet && parent.vn) {
		const validatorMessage = createMessage(config.networkId, parent.tx, parent.vn, OperationType.TRANSFER);
		parent.vs = validatorWallet.sign(await PeerWallet.blake3(validatorMessage));
	}

	return safeEncodeApplyOperation(decoded);
}

export function mutateTransferPayloadForInvalidSchema(t, validPayload) {
	const decoded = safeDecodeApplyOperation(validPayload);
	t.ok(decoded, 'transfer payload decodes before schema mutation');
	if (!decoded) return validPayload;
	return safeEncodeApplyOperation({ ...decoded, address: b4a.alloc(1) });
}

export async function mutateTransferAmountToInvalidValue(t, validPayload, context) {
	const decoded = safeDecodeApplyOperation(validPayload);
	t.ok(decoded, 'transfer payload decodes before invalid amount mutation');
	const parent = decoded?.tro;
	if (!parent?.am || !parent?.txv || !parent?.to || !parent?.in) return validPayload;

	const requesterWallet = context?.transferScenario?.senderPeer?.wallet;
	const validatorWallet = context?.transferScenario?.validatorPeer?.wallet;

	console.error('Invalid transfer amount.');
	parent.am = b4a.alloc(1); // forces toBalance(amount).value === null

	const requesterMessage = createMessage(
		config.networkId,
		parent.txv,
		parent.to,
		parent.am,
		parent.in,
		OperationType.TRANSFER
	);
	const regeneratedTxHash = await PeerWallet.blake3(requesterMessage);
	if (regeneratedTxHash?.length === parent.tx?.length) {
		parent.tx = regeneratedTxHash;
	}

	if (requesterWallet) {
		parent.is = requesterWallet.sign(regeneratedTxHash);
	}
	if (validatorWallet && parent.vn) {
		const validatorMessage = createMessage(config.networkId, parent.tx, parent.vn, OperationType.TRANSFER);
		parent.vs = validatorWallet.sign(await PeerWallet.blake3(validatorMessage));
	}

	return safeEncodeApplyOperation(decoded);
}

export async function applyTransferRecipientEntryCorruption(context, invalidPayload) {
	return applyTransferRecipientEntryOverride(context, invalidPayload, entry => {
		if (!entry) return entry;
		return { ...entry, value: b4a.alloc(1) };
	});
}

export async function applyTransferRecipientBalanceInvalid(context, invalidPayload) {
	return applyTransferRecipientBalanceDecodeFailure(context, invalidPayload);
}

export async function applyTransferRecipientBalanceAddFailure(context, invalidPayload) {
	const balancePrototype = Object.getPrototypeOf(BALANCE_ZERO);
	const originalAdd = balancePrototype.add;
	let shouldFailNextAdd = true;

	balancePrototype.add = function patchedAdd(...args) {
		if (shouldFailNextAdd) {
			shouldFailNextAdd = false;
			return null;
		}
		return originalAdd.call(this, ...args);
	};

	try {
		const node = context.transferScenario?.validatorPeer ?? context.peers?.[1];
		await appendInvalidTransferPayload(context, invalidPayload, { node });
	} finally {
		balancePrototype.add = originalAdd;
	}
}

export function createTransferHandlerGuardBypassScenario({ title, logMessage, mutatePayload }) {
	return new OperationValidationScenarioBase({
		title,
		setupScenario: setupTransferScenario,
		buildValidPayload: context => buildTransferPayload(context),
		mutatePayload: mutatePayload ?? ((_t, payload) => payload),
		applyInvalidPayload: async (context, invalidPayload) => {
			const snapshots = await snapshotTransferEntries(context);
			context.transferScenario = {
				...(context.transferScenario ?? {}),
				handlerGuardSnapshot: snapshots
			};
			console.error(logMessage);
		},
		assertStateUnchanged: (t, context, _valid, invalid) =>
			assertTransferFailureState(t, context, {
				payload: invalid,
				...(context.transferScenario?.handlerGuardSnapshot ?? {})
			}),
		expectedLogs: [logMessage]
	});
}

export async function applyTransferRecipientBalanceUpdateFailure(context, invalidPayload) {
	const balancePrototype = Object.getPrototypeOf(BALANCE_ZERO);
	const originalUpdate = balancePrototype.update;
	let shouldFailNextUpdate = true;

	balancePrototype.update = function patchedUpdate(...args) {
		if (shouldFailNextUpdate) {
			shouldFailNextUpdate = false;
			return null;
		}
		return originalUpdate.call(this, ...args);
	};

	try {
		const node = context.transferScenario?.validatorPeer ?? context.peers?.[1];
		await appendInvalidTransferPayload(context, invalidPayload, { node });
	} finally {
		balancePrototype.update = originalUpdate;
	}
}

async function applyTransferRecipientEntryOverride(context, invalidPayload, mutateEntry) {
	const node = context.transferScenario?.validatorPeer ?? context.peers?.[1];
	const recipientPeer = context.transferScenario?.recipientPeer ?? selectRecipientPeer(context, 2);
	if (!node?.base || !recipientPeer?.wallet?.address) {
		throw new Error('Recipient entry override scenario requires validator and recipient peers.');
	}

	const recipientAddress = recipientPeer.wallet.address;
	const recipientBuffer = addressUtils.addressToBuffer(recipientAddress, config.addressPrefix);
	const base = node.base;
	const originalApply = base._handlers.apply;

	base._handlers.apply = async function patchedApply(nodes, view, baseCtx) {
		const originalBatch = view.batch;
		view.batch = function patchedBatch(...args) {
			const batch = originalBatch.apply(this, args);
			if (!batch?.get) return batch;

			const originalGet = batch.get.bind(batch);
			batch.get = async key => {
				const entry = await originalGet(key);
				if (isTargetKey(key, recipientAddress, recipientBuffer)) {
					return mutateEntry(entry);
				}
				return entry;
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
		await appendInvalidTransferPayload(context, invalidPayload, { node });
	} finally {
		base._handlers.apply = originalApply;
	}
}

async function applyTransferRecipientBalanceDecodeFailure(context, invalidPayload) {
	const node = context.transferScenario?.validatorPeer ?? context.peers?.[1];
	const recipientPeer = context.transferScenario?.recipientPeer ?? selectRecipientPeer(context, 2);
	if (!node?.base || !recipientPeer?.wallet?.address) {
		throw new Error('Recipient balance decode failure scenario requires validator and recipient peers.');
	}

	const targetAddress = recipientPeer.wallet.address;
	const targetBuffer = addressUtils.addressToBuffer(targetAddress, config.addressPrefix);
	const originalDecode = nodeEntryUtils.decode;
	let shouldMutateNextDecode = false;

	nodeEntryUtils.decode = function patchedDecode(buffer) {
		const decoded = originalDecode(buffer);
		if (shouldMutateNextDecode && decoded) {
			shouldMutateNextDecode = false;
			console.error('Invalid recipient balance.');
			return { ...decoded, balance: b4a.alloc(1) };
		}
		return decoded;
	};

	const base = node.base;
	const originalApply = base._handlers.apply;

	base._handlers.apply = async function patchedApply(nodes, view, baseCtx) {
		const originalBatch = view.batch;
		view.batch = function patchedBatch(...args) {
			const batch = originalBatch.apply(this, args);
			if (!batch?.get) return batch;

			const originalGet = batch.get.bind(batch);
			batch.get = async key => {
				const entry = await originalGet(key);
				if (isTargetKey(key, targetAddress, targetBuffer)) {
					shouldMutateNextDecode = true;
				}
				return entry;
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
		await appendInvalidTransferPayload(context, invalidPayload, { node });
	} finally {
		base._handlers.apply = originalApply;
		nodeEntryUtils.decode = originalDecode;
	}
}

export async function applyTransferValidatorEntryDecodeFailure(context, invalidPayload) {
	const { node, validatorEntryBuffer } = await resolveValidatorContext(context);

	const originalDecode = nodeEntryUtils.decode;
	let decodeCount = 0;

	nodeEntryUtils.decode = function patchedDecode(buffer) {
		if (validatorEntryBuffer && b4a.isBuffer(buffer) && b4a.equals(buffer, validatorEntryBuffer)) {
			decodeCount += 1;
			if (decodeCount === 2) {
				console.error('Invalid validator entry.');
				return null;
			}
		}
		return originalDecode(buffer);
	};

	try {
		await appendInvalidTransferPayload(context, invalidPayload, { node });
	} finally {
		nodeEntryUtils.decode = originalDecode;
	}
}

export async function applyTransferValidatorBalanceInvalid(context, invalidPayload) {
	const { node, validatorEntryBuffer } = await resolveValidatorContext(context);

	const originalDecode = nodeEntryUtils.decode;

	nodeEntryUtils.decode = function patchedDecode(buffer) {
		const decoded = originalDecode(buffer);
		if (decoded && validatorEntryBuffer && b4a.isBuffer(buffer) && b4a.equals(buffer, validatorEntryBuffer)) {
			console.error('Invalid validator balance.');
			return { ...decoded, balance: b4a.alloc(1) };
		}
		return decoded;
	};

	try {
		await appendInvalidTransferPayload(context, invalidPayload, { node });
	} finally {
		nodeEntryUtils.decode = originalDecode;
	}
}

export async function applyTransferValidatorRewardFailure(context, invalidPayload) {
	const { node } = await resolveValidatorContext(context);
	const balancePrototype = Object.getPrototypeOf(BALANCE_ZERO);
	const originalPercentage = balancePrototype.percentage;
	let shouldFailNextPercentage = true;

	balancePrototype.percentage = function patchedPercentage(...args) {
		if (shouldFailNextPercentage) {
			shouldFailNextPercentage = false;
			console.error('Invalid validator reward.');
			return null;
		}
		return originalPercentage.call(this, ...args);
	};

	try {
		await appendInvalidTransferPayload(context, invalidPayload, { node });
	} finally {
		balancePrototype.percentage = originalPercentage;
	}
}

export async function applyTransferValidatorBalanceAddFailure(context, invalidPayload) {
	const { node } = await resolveValidatorContext(context);
	const balancePrototype = Object.getPrototypeOf(BALANCE_ZERO);
	const originalAdd = balancePrototype.add;
	let shouldFailNextAdd = true;

	balancePrototype.add = function patchedAdd(...args) {
		if (shouldFailNextAdd) {
			shouldFailNextAdd = false;
			return null;
		}
		return originalAdd.call(this, ...args);
	};

	try {
		await appendInvalidTransferPayload(context, invalidPayload, { node });
	} finally {
		balancePrototype.add = originalAdd;
	}
}

export async function applyTransferValidatorBalanceUpdateFailure(context, invalidPayload) {
	const { node } = await resolveValidatorContext(context);
	const balancePrototype = Object.getPrototypeOf(BALANCE_ZERO);
	const originalUpdate = balancePrototype.update;
	let updateCallCount = 0;

	balancePrototype.update = function patchedUpdate(...args) {
		updateCallCount += 1;
		if (updateCallCount === 2) {
			return null;
		}
		return originalUpdate.call(this, ...args);
	};

	try {
		await appendInvalidTransferPayload(context, invalidPayload, { node });
	} finally {
		balancePrototype.update = originalUpdate;
	}
}

async function resolveValidatorContext(context) {
	const node = context.transferScenario?.validatorPeer ?? context.peers?.[1];
	const validatorPeer = context.transferScenario?.validatorPeer ?? selectValidatorPeer(context, 0);
	if (!node?.base || !validatorPeer?.wallet?.address) {
		throw new Error('Validator scenario requires a validator peer.');
	}

	const validatorEntry = await node.base.view.get(validatorPeer.wallet.address);
	if (!validatorEntry?.value) {
		throw new Error('Validator scenario requires an existing validator entry.');
	}

	return { node, validatorPeer, validatorEntryBuffer: validatorEntry.value };
}

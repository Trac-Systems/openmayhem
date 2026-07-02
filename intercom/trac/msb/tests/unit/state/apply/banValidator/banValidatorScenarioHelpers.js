import b4a from 'b4a';
import PeerWallet from 'trac-wallet';
import { deriveIndexerSequenceState, eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import { applyStateMessageFactory } from '../../../../../src/messages/state/applyStateMessageFactory.js';
import nodeEntryUtils, { ZERO_LICENSE } from '../../../../../src/core/state/utils/nodeEntry.js';
import addressUtils from '../../../../../src/core/state/utils/address.js';
import lengthEntryUtils from '../../../../../src/core/state/utils/lengthEntry.js';
import { EntryType } from '../../../../../src/utils/constants.js';
import { BALANCE_FEE, BALANCE_ZERO, toBalance } from '../../../../../src/core/state/utils/balance.js';
import { safeDecodeApplyOperation, safeEncodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import {
	setupAddWriterScenario,
	selectWriterPeer,
	promotePeerToWriter
} from '../addWriter/addWriterScenarioHelpers.js';
import { setupAdminAndWhitelistedReaderNetwork } from '../common/commonScenarioHelper.js';
import { applyWithRequesterEntryRemoval } from '../addWriter/addWriterScenarioHelpers.js';
import { createMessage } from '../../../../../src/utils/buffer.js';
import { OperationType } from '../../../../../src/utils/constants.js';
import { config } from '../../../../helpers/config.js';

export async function setupBanValidatorScenario(
	t,
	{ promoteToWriter = true, nodes = 2, readerInitialBalance = null } = {}
) {
	if (promoteToWriter) {
		const addWriterOptions = { nodes };
		if (readerInitialBalance) {
			addWriterOptions.writerInitialBalance = readerInitialBalance;
		}
		const context = await setupAddWriterScenario(t, addWriterOptions);
		const validatorPeer = selectWriterPeer(context);
		await promotePeerToWriter(t, context, { readerPeer: validatorPeer });
		const adminEntry = await context.adminBootstrap.base.view.get(context.adminBootstrap.wallet.address);
		const validatorEntry = await context.adminBootstrap.base.view.get(validatorPeer.wallet.address);
		context.banValidatorScenario = {
			validatorPeer,
			adminEntryBefore: adminEntry ? { ...adminEntry, value: b4a.from(adminEntry.value) } : null,
			validatorEntryBefore: validatorEntry ? { ...validatorEntry, value: b4a.from(validatorEntry.value) } : null
		};
		return context;
	}

	const context = await setupAdminAndWhitelistedReaderNetwork(t, {
		nodes,
		readerInitialBalance
	});
	const validatorPeer = selectWriterPeer(context);
	const adminEntry = await context.adminBootstrap.base.view.get(context.adminBootstrap.wallet.address);
	const validatorEntry = await context.adminBootstrap.base.view.get(validatorPeer.wallet.address);
	context.banValidatorScenario = {
		validatorPeer,
		adminEntryBefore: adminEntry ? { ...adminEntry, value: b4a.from(adminEntry.value) } : null,
		validatorEntryBefore: validatorEntry ? { ...validatorEntry, value: b4a.from(validatorEntry.value) } : null
	};
	return context;
}

export async function buildBanValidatorPayload(
	context,
	{ adminPeer = context.adminBootstrap, validatorPeer = selectWriterPeer(context) } = {}
	/* cover tests */
) {
	const txValidity = await deriveIndexerSequenceState(adminPeer.base);
	return safeEncodeApplyOperation(
		await applyStateMessageFactory(adminPeer.wallet, config)
			.buildCompleteBanWriterMessage(adminPeer.wallet.address, validatorPeer.wallet.address, txValidity)
	);
}

export async function buildBanValidatorPayloadWithTxValidity(
	context,
	mutatedTxValidity,
	{ adminPeer = context.adminBootstrap, validatorPeer = selectWriterPeer(context) } = {}
) {
	return safeEncodeApplyOperation(
		await applyStateMessageFactory(adminPeer.wallet, config)
			.buildCompleteBanWriterMessage(adminPeer.wallet.address, validatorPeer.wallet.address, mutatedTxValidity)
	);
}

export async function assertBanValidatorSuccessState(
	t,
	context,
	{
		validatorPeer = selectWriterPeer(context),
		adminPeer = context.adminBootstrap,
		validatorEntryBefore,
		adminEntryBefore,
	payload,
	expectedInitialRoles = { isWhitelisted: true, isWriter: true, isIndexer: false },
	expectWriterRegistry = null,
	skipSync = false
} = {}
) {
	if (!validatorEntryBefore?.value) {
		throw new Error('assertBanValidatorSuccessState requires the validator entry before ban.');
	}
	if (!adminEntryBefore?.value) {
		throw new Error('assertBanValidatorSuccessState requires the admin entry before ban.');
	}
	if (!payload) {
		throw new Error('assertBanValidatorSuccessState requires the banValidator payload.');
	}

	const decodedBefore = nodeEntryUtils.decode(validatorEntryBefore.value);
	t.ok(decodedBefore, 'validator entry before banValidator decodes');
	if (!decodedBefore) return;
	const expectWriterBefore = expectedInitialRoles?.isWriter !== false;
	const expectWhitelistedBefore = expectedInitialRoles?.isWhitelisted !== false;
	const expectIndexerBefore = expectedInitialRoles?.isIndexer === true;

	t.is(decodedBefore.isWhitelisted, expectWhitelistedBefore, 'validator whitelist status before banValidator');
	t.is(decodedBefore.isWriter, expectWriterBefore, 'validator writer status before banValidator');
	t.is(decodedBefore.isIndexer, expectIndexerBefore, 'validator indexer status before banValidator');

	const balanceBefore = toBalance(decodedBefore.balance);
	const stakedBefore = toBalance(decodedBefore.stakedBalance);
	t.ok(balanceBefore, 'validator balance before banValidator decodes');
	t.ok(stakedBefore, 'validator staked balance before banValidator decodes');

	const validatorEntryAfter = await adminPeer.base.view.get(validatorPeer.wallet.address);
	t.ok(validatorEntryAfter, 'validator entry exists after banValidator');
	const decodedAfter = nodeEntryUtils.decode(validatorEntryAfter?.value);
	t.ok(decodedAfter, 'validator entry decodes after banValidator');
	if (!decodedAfter || !balanceBefore || !stakedBefore) return;

	t.is(decodedAfter.isWhitelisted, false, 'validator whitelist flag cleared');
	t.is(decodedAfter.isWriter, false, 'validator writer flag cleared');
	t.is(decodedAfter.isIndexer, false, 'validator indexer flag remains cleared');
	t.ok(b4a.equals(decodedAfter.wk, decodedBefore.wk), 'validator writing key preserved');

	const expectedBalance = balanceBefore.add(stakedBefore);
	t.ok(expectedBalance, 'validator balance after unstaking computed');
	if (expectedBalance) {
		t.ok(
			b4a.equals(decodedAfter.balance, expectedBalance.value),
			'validator balance includes unstaked amount'
		);
	}
	t.ok(
		b4a.equals(decodedAfter.stakedBalance, BALANCE_ZERO.value),
		'validator staked balance cleared after ban'
	);

	t.ok(!b4a.equals(decodedAfter.license, ZERO_LICENSE), 'validator license retained after ban');
	t.ok(
		b4a.equals(decodedAfter.license, decodedBefore.license),
		'validator license unchanged after ban'
	);

	const licenseId = lengthEntryUtils.decodeBE(decodedAfter.license);
	const licenseIndexEntry = await adminPeer.base.view.get(`${EntryType.LICENSE_INDEX}${licenseId}`);
	t.ok(licenseIndexEntry, 'license index entry persists after banValidator');
        if (licenseIndexEntry?.value) {
            const validatorAddressBuffer = addressUtils.addressToBuffer(validatorPeer.wallet.address, config.addressPrefix);
		t.ok(
			b4a.equals(licenseIndexEntry.value, validatorAddressBuffer),
			'license index still maps to validator address'
		);
	}

	const shouldCheckRegistry = expectWriterRegistry ?? expectWriterBefore;
	if (shouldCheckRegistry) {
		const registryKey = EntryType.WRITER_ADDRESS + decodedBefore.wk.toString('hex');
		const registryEntry = await adminPeer.base.view.get(registryKey);
		t.ok(registryEntry, 'writer registry entry persists after banValidator');
        if (registryEntry?.value) {
            const validatorAddressBuffer = addressUtils.addressToBuffer(validatorPeer.wallet.address, config.addressPrefix);
			t.ok(
				b4a.equals(registryEntry.value, validatorAddressBuffer),
				'writer registry maps writing key to validator address'
			);
		}
	}

	const decodedAdminBefore = nodeEntryUtils.decode(adminEntryBefore.value);
	t.ok(decodedAdminBefore, 'admin entry before banValidator decodes');
	const adminBalanceBefore = toBalance(decodedAdminBefore?.balance);
	t.ok(adminBalanceBefore, 'admin balance before banValidator decodes');
	if (decodedAdminBefore && adminBalanceBefore) {
		const adminEntryAfter = await adminPeer.base.view.get(adminPeer.wallet.address);
		t.ok(adminEntryAfter, 'admin entry exists after banValidator');
		const decodedAdminAfter = nodeEntryUtils.decode(adminEntryAfter?.value);
		t.ok(decodedAdminAfter, 'admin entry decodes after banValidator');
		if (decodedAdminAfter) {
			const expectedAdminBalance = adminBalanceBefore.sub(BALANCE_FEE);
			t.ok(expectedAdminBalance, 'admin balance after fee computation succeeds');
			if (expectedAdminBalance) {
				t.ok(
					b4a.equals(decodedAdminAfter.balance, expectedAdminBalance.value),
					'admin balance reduced by banValidator fee'
				);
			}
			t.ok(
				b4a.equals(decodedAdminAfter.stakedBalance, decodedAdminBefore.stakedBalance),
				'admin staked balance unchanged after banValidator'
			);
		}
	}

	await assertBanValidatorPayloadMetadata(t, adminPeer.base, payload, {
		adminAddress: adminPeer.wallet.address,
		targetAddress: validatorPeer.wallet.address
	});

	if (!skipSync) {
		await context.sync();
		await assertBannedValidatorReplicated(t, validatorPeer.base, {
			address: validatorPeer.wallet.address,
			writingKey: decodedBefore.wk,
			expectedBalance: expectedBalance?.value,
			expectedLicense: decodedBefore.license
		});
	}
}

export async function assertBanValidatorFailureState(
	t,
	context,
	{
		validatorPeer = selectWriterPeer(context),
		adminPeer = context.adminBootstrap,
		validatorEntryBefore = null,
		adminEntryBefore = null,
		expectedRoles = null,
		allowEntryMutation = false,
		skipSync = false
	} = {}
) {
	const validatorSnapshot = validatorEntryBefore ?? context.banValidatorScenario?.validatorEntryBefore ?? null;
	const adminSnapshot = adminEntryBefore ?? context.banValidatorScenario?.adminEntryBefore ?? null;

	const entry = await adminPeer.base.view.get(validatorPeer.wallet.address);
	t.ok(entry, 'validator entry persists after failed banValidator');
	const decoded = nodeEntryUtils.decode(entry?.value);
	t.ok(decoded, 'validator entry decodes after failed banValidator');
	if (decoded) {
		let expectedWhitelisted = expectedRoles?.isWhitelisted ?? true;
		let expectedWriter = expectedRoles?.isWriter ?? true;
		let expectedIndexer = expectedRoles?.isIndexer ?? false;

		if (validatorSnapshot?.value) {
			const decodedBefore = nodeEntryUtils.decode(validatorSnapshot.value);
			if (decodedBefore) {
				expectedWhitelisted =
					expectedRoles?.isWhitelisted !== undefined
						? expectedRoles.isWhitelisted
						: decodedBefore.isWhitelisted;
				expectedWriter =
					expectedRoles?.isWriter !== undefined ? expectedRoles.isWriter : decodedBefore.isWriter;
				expectedIndexer =
					expectedRoles?.isIndexer !== undefined ? expectedRoles.isIndexer : decodedBefore.isIndexer;
			}
		}

		t.is(decoded.isWhitelisted, expectedWhitelisted, 'validator remains whitelisted after failure');
		t.is(decoded.isWriter, expectedWriter, 'validator retains writer role after failure');
		t.is(decoded.isIndexer, expectedIndexer, 'validator not promoted to indexer after failure');
	}
	if (validatorSnapshot?.value && !allowEntryMutation) {
		t.ok(
			b4a.equals(entry?.value, validatorSnapshot.value),
			'validator entry remains unchanged after failure'
		);
	}

	if (adminSnapshot?.value && !allowEntryMutation) {
		const adminEntryAfter = await adminPeer.base.view.get(adminPeer.wallet.address);
		if (adminEntryAfter?.value) {
			t.ok(
				b4a.equals(adminEntryAfter.value, adminSnapshot.value),
				'admin entry remains unchanged after failure'
			);
		}
	}

	if (!skipSync) {
		await context.sync();
		const peerEntry = await validatorPeer.base.view.get(validatorPeer.wallet.address);
		t.ok(peerEntry, 'validator entry replicated after failed banValidator');
		if (validatorEntryBefore?.value && peerEntry?.value) {
			t.ok(
				b4a.equals(peerEntry.value, validatorEntryBefore.value),
				'validator peer sees unchanged entry after failure'
			);
		}
	}
}

export async function applyWithTargetNodeEntryRemoval(context, invalidPayload, { peer = null } = {}) {
	const targetPeer = peer ?? context.banValidatorScenario?.validatorPeer ?? selectWriterPeer(context);
	await applyWithRequesterEntryRemoval(context, invalidPayload, { peer: targetPeer });
}

export async function applyInvalidTargetAddressPayload(context, validPayload) {
	const adminPeer = context.adminBootstrap;
	const decoded = safeDecodeApplyOperation(validPayload);
	if (!decoded?.aco) return;

	const invalidAddress = b4a.from(decoded.aco.ia); // preserve length
	invalidAddress[0] ^= 0xff; // corrupt address buffer without changing shape
	decoded.aco.ia = invalidAddress;

	const message = createMessage(
		config.networkId,
		decoded.aco.txv,
		decoded.aco.ia,
		decoded.aco.in,
		OperationType.BAN_VALIDATOR
	);
	const newHash = await PeerWallet.blake3(message);
	decoded.aco.tx = newHash;
	decoded.aco.is = adminPeer.wallet.sign(newHash);

	const invalidPayload = safeEncodeApplyOperation(decoded);
	await adminPeer.base.append(invalidPayload);
	await adminPeer.base.update();
	await eventFlush();
}

export async function applyInvalidIndexerSequenceStatePayload(context, validPayload) {
	const adminPeer = context.adminBootstrap;
	const base = adminPeer.base;
	const originalIndexers = base.system?.indexers;

	base.system.indexers = null;
	try {
		await adminPeer.base.append(validPayload);
		await adminPeer.base.update();
		await eventFlush();
	} finally {
		base.system.indexers = originalIndexers;
	}
}

export async function promoteValidatorToIndexer(
    context,
    { adminPeer = context.adminBootstrap, validatorPeer = selectWriterPeer(context) } = {}
) {
    const txValidity = await deriveIndexerSequenceState(adminPeer.base);
    const payload = safeEncodeApplyOperation(
        await applyStateMessageFactory(adminPeer.wallet, config)
            .buildCompleteAddIndexerMessage(adminPeer.wallet.address, validatorPeer.wallet.address, txValidity)
    );

	await adminPeer.base.append(payload);
	await adminPeer.base.update();
	await eventFlush();

	const updatedEntry = await adminPeer.base.view.get(validatorPeer.wallet.address);
	if (updatedEntry?.value) {
		context.banValidatorScenario = {
			...(context.banValidatorScenario ?? {}),
			validatorEntryBefore: { ...updatedEntry, value: b4a.from(updatedEntry.value) }
		};
	}

	const adminEntry = await adminPeer.base.view.get(adminPeer.wallet.address);
	if (adminEntry?.value) {
		context.banValidatorScenario = {
			...(context.banValidatorScenario ?? {}),
			adminEntryBefore: { ...adminEntry, value: b4a.from(adminEntry.value) }
		};
	}

	return payload;
}

export async function applyWithBanValidatorRoleUpdateFailure(context, invalidPayload) {
	const node = context.adminBootstrap ?? context.bootstrap ?? context.peers?.[0] ?? null;
	if (!node?.base) {
		throw new Error('Ban validator role update failure scenario requires a writable node.');
	}

	const originalSetRole = nodeEntryUtils.setRole;
	let shouldFailNextCall = true;

	nodeEntryUtils.setRole = function patchedSetRole(...args) {
		if (shouldFailNextCall) {
			shouldFailNextCall = false;
			return null;
		}
		return originalSetRole.apply(this, args);
	};

	try {
		await node.base.append(invalidPayload);
		await node.base.update();
		await eventFlush();
	} finally {
		nodeEntryUtils.setRole = originalSetRole;
	}
}

export async function applyWithBanValidatorRoleDecodeFailure(context, invalidPayload) {
	const node = context.adminBootstrap ?? context.bootstrap ?? context.peers?.[0] ?? null;
	if (!node?.base) {
		throw new Error('Ban validator role decode failure scenario requires a writable node.');
	}

	const originalSetRole = nodeEntryUtils.setRole;
	nodeEntryUtils.setRole = function patchedSetRole(...args) {
		// return an intentionally undecodable buffer to hit the decode guard
		return b4a.alloc(1);
	};

	try {
		await node.base.append(invalidPayload);
		await node.base.update();
		await eventFlush();
	} finally {
		nodeEntryUtils.setRole = originalSetRole;
	}
}

export async function applyWithBanValidatorWithdrawFailure(context, invalidPayload) {
	const targetPeer = context.banValidatorScenario?.validatorPeer ?? selectWriterPeer(context);
	const targetAddress = targetPeer.wallet.address;
    const targetBuffer = addressUtils.addressToBuffer(targetAddress, config.addressPrefix);
	const adminPeer = context.adminBootstrap;
	const base = adminPeer.base;
	const originalApply = base._handlers.apply;

	base._handlers.apply = async function patchedApply(nodes, view, baseCtx) {
		const originalBatch = view.batch;
		view.batch = function patchedBatch(...args) {
			const batch = originalBatch.apply(this, args);
			if (!batch?.get) return batch;

			const originalGet = batch.get.bind(batch);
			batch.get = async key => {
				const entry = await originalGet(key);
				if (!entry || !entry.value) return entry;
				if (isTargetKey(key, targetAddress, targetBuffer)) {
					const mutatedValue = nodeEntryUtils.setStakedBalance(
						b4a.from(entry.value),
						BALANCE_ZERO.value
					);
					if (mutatedValue) {
						return { ...entry, value: mutatedValue };
					}
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
		await adminPeer.base.append(invalidPayload);
		await adminPeer.base.update();
		await eventFlush();
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

async function assertBanValidatorPayloadMetadata(t, base, payload, { adminAddress, targetAddress }) {
	const decodedPayload = safeDecodeApplyOperation(payload);
	t.ok(decodedPayload, 'banValidator payload decodes');
	if (!decodedPayload) return;

    const requesterAddress = addressUtils.bufferToAddress(decodedPayload.address, config.addressPrefix);
	t.ok(requesterAddress, 'banValidator requester address decodes');
	if (requesterAddress) {
		t.is(requesterAddress, adminAddress, 'banValidator payload signed by admin');
	}

    const targetAddressDecoded = addressUtils.bufferToAddress(decodedPayload?.aco?.ia, config.addressPrefix);
	t.ok(targetAddressDecoded, 'banValidator target address decodes');
	if (targetAddressDecoded) {
		t.is(targetAddressDecoded, targetAddress, 'banValidator payload targets expected validator');
	}

	const txHashBuffer = decodedPayload?.aco?.tx;
	t.ok(txHashBuffer, 'banValidator tx hash extracted');
	if (txHashBuffer) {
		const txEntry = await base.view.get(txHashBuffer.toString('hex'));
		t.ok(txEntry, 'banValidator transaction recorded for replay protection');
	}
}

async function assertBannedValidatorReplicated(
	t,
	base,
	{ address, writingKey, expectedBalance, expectedLicense }
) {
	const entry = await base.view.get(address);
	t.ok(entry, 'validator entry replicated to peer after banValidator');
	const decoded = nodeEntryUtils.decode(entry?.value);
	t.ok(decoded, 'replicated validator entry decodes');
	if (!decoded) return;
	t.is(decoded.isWhitelisted, false, 'replicated entry not whitelisted');
	t.is(decoded.isWriter, false, 'replicated entry writer flag cleared');
	t.is(decoded.isIndexer, false, 'replicated entry indexer flag cleared');
	t.ok(b4a.equals(decoded.wk, writingKey), 'replicated entry preserves writing key');
	t.ok(
		b4a.equals(decoded.stakedBalance, BALANCE_ZERO.value),
		'replicated entry staked balance cleared'
	);
	if (expectedBalance) {
		t.ok(
			b4a.equals(decoded.balance, expectedBalance),
			'replicated entry balance matches expected amount'
		);
	}
	if (expectedLicense) {
		t.ok(
			b4a.equals(decoded.license, expectedLicense),
			'replicated entry license preserved'
		);
	}
}

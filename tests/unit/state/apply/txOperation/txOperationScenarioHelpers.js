import b4a from 'b4a';
import { applyStateMessageFactory } from '../../../../../src/messages/state/applyStateMessageFactory.js';
import {
	deriveIndexerSequenceState,
	eventFlush
} from '../../../../helpers/autobaseTestHelpers.js';
import {
	setupAdminNetwork,
	initializeBalances,
	whitelistAddress
} from '../common/commonScenarioHelper.js';
import { promotePeerToWriter } from '../addWriter/addWriterScenarioHelpers.js';
import nodeEntryUtils from '../../../../../src/core/state/utils/nodeEntry.js';
import addressUtils from '../../../../../src/core/state/utils/address.js';
import deploymentEntryUtils from '../../../../../src/core/state/utils/deploymentEntry.js';
import transactionUtils from '../../../../../src/core/state/utils/transaction.js';
import { toBalance, PERCENT_25, PERCENT_50, PERCENT_75 } from '../../../../../src/core/state/utils/balance.js';
import { EntryType } from '../../../../../src/utils/constants.js';
import { decimalStringToBigInt, bigIntTo16ByteBuffer } from '../../../../../src/utils/amountSerialization.js';
import {
	buildBootstrapDeploymentPayload
} from '../bootstrapDeployment/bootstrapDeploymentScenarioHelpers.js';
import {
	safeDecodeApplyOperation,
	safeEncodeApplyOperation
} from '../../../../../src/utils/protobuf/operationHelpers.js';
import { config } from '../../../../helpers/config.js';

const DEFAULT_FUNDING = bigIntTo16ByteBuffer(decimalStringToBigInt('10'));
const DEFAULT_CONTENT_HASH = b4a.alloc(32, 0xab);

function selectValidatorPeer(context, offset = 0) {
	const peers = context.peers.slice(1);
	if (!peers.length) {
		throw new Error('TxOperation scenarios require at least one non-admin peer.');
	}
	return peers[Math.min(offset, peers.length - 1)];
}

function selectDeployerPeer(context, offset = 1) {
	const peers = context.peers.slice(1);
	if (peers.length < 2) {
		throw new Error('TxOperation scenarios require a deployer peer.');
	}
	return peers[Math.min(offset, peers.length - 1)];
}

function selectBroadcasterPeer(context, offset = 2) {
	const peers = context.peers.slice(1);
	if (peers.length < 3) {
		throw new Error('TxOperation scenarios require a broadcaster peer.');
	}
	return peers[Math.min(offset, peers.length - 1)];
}

export async function setupTxOperationScenario(
	t,
	{
		nodes = 4,
		validatorInitialBalance = DEFAULT_FUNDING,
		deployerInitialBalance = DEFAULT_FUNDING,
		broadcasterInitialBalance = DEFAULT_FUNDING,
		contentHash = DEFAULT_CONTENT_HASH,
		creatorPeerKind = 'deployer' // 'deployer' | 'validator' | 'requester'
	} = {}
) {
	const context = await setupAdminNetwork(t, { nodes: Math.max(nodes, 4) });
	const validatorPeer = selectValidatorPeer(context);
	const deployerPeer = selectDeployerPeer(context);
	const broadcasterPeer = selectBroadcasterPeer(context);

	await initializeBalances(context, [
		[validatorPeer.wallet.address, validatorInitialBalance],
		[deployerPeer.wallet.address, deployerInitialBalance],
		[broadcasterPeer.wallet.address, broadcasterInitialBalance]
	]);

	context.addWriterScenario = { writerInitialBalance: validatorInitialBalance };
	await whitelistAddress(context, validatorPeer.wallet.address);
	await whitelistAddress(context, deployerPeer.wallet.address);
	await whitelistAddress(context, broadcasterPeer.wallet.address);

	await promotePeerToWriter(t, context, { readerPeer: validatorPeer });
	await context.sync();

	const creatorPeer =
		creatorPeerKind === 'validator'
			? validatorPeer
			: creatorPeerKind === 'requester'
			? broadcasterPeer
			: deployerPeer;
	const bootstrapValidatorPeer =
		creatorPeerKind === 'validator' ? context.adminBootstrap : validatorPeer;

	const externalBootstrap = b4a.from(creatorPeer.base.local.key);
	const bootstrapPayload = await buildBootstrapDeploymentPayload(context, {
		validatorPeer: bootstrapValidatorPeer,
		deployerPeer: creatorPeer,
		externalBootstrap
	});

	await bootstrapValidatorPeer.base.append(bootstrapPayload);
	await bootstrapValidatorPeer.base.update();
	await eventFlush();
	await context.sync();

	const validatorEntryBefore = await validatorPeer.base.view.get(validatorPeer.wallet.address);
	const deployerEntryBefore = await validatorPeer.base.view.get(creatorPeer.wallet.address);
	const requesterEntryBefore = await validatorPeer.base.view.get(broadcasterPeer.wallet.address);
	const txValidity = await deriveIndexerSequenceState(validatorPeer.base);

	context.txOperation = {
		validatorPeer,
		deployerPeer: creatorPeer,
		creatorPeer,
		broadcasterPeer,
		externalBootstrap,
		msbBootstrap: b4a.from(context.bootstrap.base.local.key),
		txValidity,
		contentHash,
		validatorEntryBefore: validatorEntryBefore ? { value: b4a.from(validatorEntryBefore.value) } : null,
		deployerEntryBefore: deployerEntryBefore ? { value: b4a.from(deployerEntryBefore.value) } : null,
		requesterEntryBefore: requesterEntryBefore ? { value: b4a.from(requesterEntryBefore.value) } : null
	};

	return context;
}

export async function buildTxOperationPayload(
	context,
	{
		validatorPeer = context.txOperation?.validatorPeer ?? selectValidatorPeer(context),
		broadcasterPeer = context.txOperation?.broadcasterPeer ?? selectBroadcasterPeer(context),
		externalBootstrap = context.txOperation?.externalBootstrap ?? b4a.from(broadcasterPeer.base.local.key),
		msbBootstrap = context.txOperation?.msbBootstrap ?? b4a.from(context.bootstrap.base.local.key),
		txValidity = context.txOperation?.txValidity ?? null,
		contentHash = context.txOperation?.contentHash ?? DEFAULT_CONTENT_HASH,
		writerKeyBuffer = broadcasterPeer.base.local.key
	} = {}
) {
    const resolvedTxValidity = txValidity ?? (await deriveIndexerSequenceState(validatorPeer.base));

    const partial = await applyStateMessageFactory(broadcasterPeer.wallet, config)
        .buildPartialTransactionOperationMessage(
            broadcasterPeer.wallet.address,
            writerKeyBuffer.toString('hex'),
            resolvedTxValidity.toString('hex'),
            contentHash.toString('hex'),
            externalBootstrap.toString('hex'),
            msbBootstrap.toString('hex'),
            'json'
        );

    const payload = await applyStateMessageFactory(validatorPeer.wallet, config)
        .buildCompleteTransactionOperationMessage(
            partial.address,
            b4a.from(partial.txo.tx, 'hex'),
            b4a.from(partial.txo.txv, 'hex'),
            b4a.from(partial.txo.iw, 'hex'),
            b4a.from(partial.txo.in, 'hex'),
            b4a.from(partial.txo.ch, 'hex'),
            b4a.from(partial.txo.is, 'hex'),
            b4a.from(partial.txo.bs, 'hex'),
            b4a.from(partial.txo.mbs, 'hex')
        );
    return safeEncodeApplyOperation(payload);
}

export async function buildTxOperationPayloadWithTxValidity(context, txValidity, options = {}) {
	if (!b4a.isBuffer(txValidity)) {
		throw new Error('buildTxOperationPayloadWithTxValidity requires a tx validity buffer.');
	}
	return buildTxOperationPayload(context, { ...options, txValidity });
}

export async function assertTxOperationSuccessState(
	t,
	context,
	{
		payload,
		validatorPeer = context.txOperation?.validatorPeer ?? selectValidatorPeer(context),
		deployerPeer = context.txOperation?.deployerPeer ?? selectDeployerPeer(context),
		broadcasterPeer = context.txOperation?.broadcasterPeer ?? selectBroadcasterPeer(context),
		creatorPeer = context.txOperation?.creatorPeer ?? deployerPeer,
		validatorEntryBefore = context.txOperation?.validatorEntryBefore?.value ?? null,
		deployerEntryBefore = context.txOperation?.deployerEntryBefore?.value ?? null,
		requesterEntryBefore = context.txOperation?.requesterEntryBefore?.value ?? null,
		distribution = 'standard', // 'standard' | 'requesterIsCreator' | 'validatorIsCreator'
		skipSync = false
	} = {}
) {
	if (!payload) throw new Error('assertTxOperationSuccessState requires the processed payload.');

	const decoded = safeDecodeApplyOperation(payload);
	t.ok(decoded, 'txOperation payload decodes');
	if (!decoded?.txo) return;

	const txHashBuffer = decoded.txo.tx;
	const requesterAddressBuffer = decoded.address;
	const validatorAddressBuffer = decoded.txo.va;
	const externalBootstrap = decoded.txo.bs;
	const msbBootstrap = decoded.txo.mbs;

	t.ok(requesterAddressBuffer, 'payload carries requester address');
	t.ok(validatorAddressBuffer, 'payload carries validator address');
	t.ok(txHashBuffer, 'payload exposes tx hash');
	if (externalBootstrap) {
		t.ok(b4a.equals(externalBootstrap, context.txOperation?.externalBootstrap), 'payload external bootstrap matches deployment');
	}
	if (msbBootstrap) {
		t.ok(b4a.equals(msbBootstrap, context.txOperation?.msbBootstrap), 'payload MSB bootstrap matches network');
	}

	const requesterAddress = addressUtils.bufferToAddress(requesterAddressBuffer, config.addressPrefix);
	const validatorAddress = addressUtils.bufferToAddress(validatorAddressBuffer, config.addressPrefix);

	t.is(requesterAddress, broadcasterPeer.wallet.address, 'requester matches broadcaster');
	t.is(validatorAddress, validatorPeer.wallet.address, 'validator matches selected peer');

	if (!validatorEntryBefore || !deployerEntryBefore || !requesterEntryBefore) {
		throw new Error('assertTxOperationSuccessState requires entry snapshots.');
	}

	const feeAmount = toBalance(transactionUtils.FEE);
	t.ok(feeAmount, 'fee decodes');
	if (!feeAmount) return;

	const requesterBeforeDecoded = nodeEntryUtils.decode(requesterEntryBefore);
	const validatorBeforeDecoded = nodeEntryUtils.decode(validatorEntryBefore);
	const deployerBeforeDecoded = nodeEntryUtils.decode(deployerEntryBefore);

	t.ok(requesterBeforeDecoded, 'requester entry before decodes');
	t.ok(validatorBeforeDecoded, 'validator entry before decodes');
	t.ok(deployerBeforeDecoded, 'deployer entry before decodes');
	if (!requesterBeforeDecoded || !validatorBeforeDecoded || !deployerBeforeDecoded) return;

	const requesterBalanceBefore = toBalance(requesterBeforeDecoded.balance);
	const validatorBalanceBefore = toBalance(validatorBeforeDecoded.balance);
	const deployerBalanceBefore = toBalance(deployerBeforeDecoded.balance);

	t.ok(requesterBalanceBefore, 'requester balance before decodes');
	t.ok(validatorBalanceBefore, 'validator balance before decodes');
	t.ok(deployerBalanceBefore, 'deployer balance before decodes');
	if (!requesterBalanceBefore || !validatorBalanceBefore || !deployerBalanceBefore) return;

	const expectedRequesterBalance = requesterBalanceBefore.sub(feeAmount);
	let expectedValidatorBalance = validatorBalanceBefore.add(feeAmount.percentage(PERCENT_50));
	let expectedDeployerBalance = deployerBalanceBefore.add(feeAmount.percentage(PERCENT_25));

	if (distribution === 'validatorIsCreator') {
		expectedValidatorBalance = validatorBalanceBefore.add(feeAmount.percentage(PERCENT_75));
		expectedDeployerBalance = null; // same as validator
	}

	if (distribution === 'requesterIsCreator') {
		expectedDeployerBalance = null; // requester is creator, no bonus
	}

	await context.sync();
	const requesterAfter = await validatorPeer.base.view.get(broadcasterPeer.wallet.address);
	const validatorAfter = await validatorPeer.base.view.get(validatorPeer.wallet.address);
	const deployerAfter = await validatorPeer.base.view.get(deployerPeer.wallet.address);

	t.ok(requesterAfter?.value, 'requester entry exists after tx');
	t.ok(validatorAfter?.value, 'validator entry exists after tx');
	t.ok(deployerAfter?.value, 'deployer entry exists after tx');

	const requesterDecoded = requesterAfter?.value ? nodeEntryUtils.decode(requesterAfter.value) : null;
	const validatorDecoded = validatorAfter?.value ? nodeEntryUtils.decode(validatorAfter.value) : null;
	const deployerDecoded = deployerAfter?.value ? nodeEntryUtils.decode(deployerAfter.value) : null;

	t.ok(requesterDecoded, 'requester entry decodes after tx');
	t.ok(validatorDecoded, 'validator entry decodes after tx');
	t.ok(deployerDecoded, 'deployer entry decodes after tx');
	if (!requesterDecoded || !validatorDecoded || !deployerDecoded) return;

	t.ok(
		b4a.equals(requesterDecoded.balance, expectedRequesterBalance.value),
		'requester balance reduced by full fee'
	);
	t.ok(
		b4a.equals(validatorDecoded.balance, expectedValidatorBalance.value),
		distribution === 'validatorIsCreator'
			? 'validator rewarded with 75% fee when creator'
			: 'validator rewarded with 50% fee'
	);

	if (distribution === 'standard') {
		t.ok(
			b4a.equals(deployerDecoded.balance, expectedDeployerBalance.value),
			'deployer rewarded with 25% fee'
		);
	}

	if (distribution === 'requesterIsCreator') {
		t.ok(
			b4a.equals(requesterDecoded.balance, expectedRequesterBalance.value),
			'requester pays full fee and receives no creator reward'
		);
	}

	if (distribution === 'validatorIsCreator') {
		// deployer is validator; nothing to assert on separate deployer entry
	}

	t.is(requesterDecoded.isWhitelisted, true, 'requester stays whitelisted');
	t.is(validatorDecoded.isWriter, true, 'validator remains a writer');

	const txEntry = await validatorPeer.base.view.get(txHashBuffer.toString('hex'));
	t.ok(txEntry, 'tx hash recorded for replay protection');

	const deploymentKey = `${EntryType.DEPLOYMENT}${externalBootstrap.toString('hex')}`;
	const deploymentEntry = await validatorPeer.base.view.get(deploymentKey);
	t.ok(deploymentEntry, 'deployment entry remains present after tx');
	const decodedDeployment = deploymentEntryUtils.decode(deploymentEntry?.value, config.addressLength);
	t.ok(decodedDeployment, 'deployment entry decodes after tx');
	if (decodedDeployment?.address) {
		const creatorAddress = addressUtils.bufferToAddress(decodedDeployment.address, config.addressPrefix);
		t.is(
			creatorAddress,
			creatorPeer.wallet.address,
			'deployment entry still bound to subnetwork creator'
		);
	}

	if (!skipSync) {
		await context.sync();
		const replicaTxEntry = await broadcasterPeer.base.view.get(txHashBuffer.toString('hex'));
		t.ok(replicaTxEntry, 'tx entry replicated to broadcaster');
	}
}

export async function assertTxOperationFailureState(
	t,
	context,
	{
		payload,
		validatorPeer = context.txOperation?.validatorPeer ?? selectValidatorPeer(context),
		deployerPeer = context.txOperation?.deployerPeer ?? selectDeployerPeer(context),
		broadcasterPeer = context.txOperation?.broadcasterPeer ?? selectBroadcasterPeer(context),
		validatorEntryBefore = context.txOperation?.validatorEntryBefore?.value ?? null,
		deployerEntryBefore = context.txOperation?.deployerEntryBefore?.value ?? null,
		requesterEntryBefore = context.txOperation?.requesterEntryBefore?.value ?? null
	} = {}
) {
	if (!payload) throw new Error('assertTxOperationFailureState requires payload.');

	const decoded = safeDecodeApplyOperation(payload);
	t.ok(decoded, 'invalid tx payload decodes');

	if (validatorEntryBefore) {
		const after = await validatorPeer.base.view.get(validatorPeer.wallet.address);
		t.ok(after, 'validator entry still exists after rejection');
		if (after?.value) {
			t.ok(b4a.equals(after.value, validatorEntryBefore), 'validator entry unchanged after rejection');
		}
	}

	if (deployerEntryBefore) {
		const after = await validatorPeer.base.view.get(deployerPeer.wallet.address);
		t.ok(after, 'deployer entry still exists after rejection');
		if (after?.value) {
			t.ok(b4a.equals(after.value, deployerEntryBefore), 'deployer entry unchanged after rejection');
		}
	}

	if (requesterEntryBefore) {
		const after = await validatorPeer.base.view.get(broadcasterPeer.wallet.address);
		t.ok(after, 'requester entry still exists after rejection');
		if (after?.value) {
			t.ok(b4a.equals(after.value, requesterEntryBefore), 'requester entry unchanged after rejection');
		}
	}

	const txHashBuffer = decoded?.txo?.tx;
	if (txHashBuffer) {
		const txEntry = await validatorPeer.base.view.get(txHashBuffer.toString('hex'));
		t.is(txEntry, null, 'tx hash not recorded after rejection');
	}
}

export async function appendInvalidTxPayload(context, invalidPayload) {
	const node =
		context.bootstrap ?? context.adminBootstrap ?? context.txOperation?.validatorPeer ?? context.peers?.[0];
	await node.base.append(invalidPayload);
	await node.base.update();
	await eventFlush();
}

export function mutateBootstrapEqualMbs(t, validPayload) {
	const decoded = safeDecodeApplyOperation(validPayload);
	t.ok(decoded, 'fixtures decode');
	if (!decoded?.txo) return validPayload;
	decoded.txo.bs = decoded.txo.mbs;
	return safeEncodeApplyOperation(decoded);
}

export function mutateMbsMismatch(t, validPayload) {
	const decoded = safeDecodeApplyOperation(validPayload);
	t.ok(decoded, 'fixtures decode');
	if (!decoded?.txo) return validPayload;
	decoded.txo.mbs = b4a.alloc(decoded.txo.mbs?.length ?? 32, 0x7e);
	return safeEncodeApplyOperation(decoded);
}

export function mutateValidatorSignature(t, validPayload) {
	const decoded = safeDecodeApplyOperation(validPayload);
	t.ok(decoded, 'fixtures decode');
	const parent = decoded?.txo;
	if (!parent?.vs) return validPayload;
	const mutated = b4a.from(parent.vs);
	mutated[mutated.length - 1] ^= 0xff;
	parent.vs = mutated;
	return safeEncodeApplyOperation(decoded);
}

export function mutateBootstrapUnregistered(t, validPayload) {
	const decoded = safeDecodeApplyOperation(validPayload);
	t.ok(decoded, 'fixtures decode');
	const parent = decoded?.txo;
	if (!parent?.bs) return validPayload;
	const mutated = b4a.alloc(parent.bs.length, 0x24);
	parent.bs = mutated;
	return safeEncodeApplyOperation(decoded);
}

export default {
	setupTxOperationScenario,
	buildTxOperationPayload,
	assertTxOperationSuccessState,
	assertTxOperationFailureState
};

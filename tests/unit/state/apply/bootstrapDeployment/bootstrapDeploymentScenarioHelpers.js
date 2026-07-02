import b4a from 'b4a';
import { applyStateMessageFactory } from '../../../../../src/messages/state/applyStateMessageFactory.js';
import { deriveIndexerSequenceState } from '../../../../helpers/autobaseTestHelpers.js';
import {
	setupAdminNetwork,
	initializeBalances,
	whitelistAddress
} from '../common/commonScenarioHelper.js';
import {
	promotePeerToWriter,
	assertValidatorReward
} from '../addWriter/addWriterScenarioHelpers.js';
import nodeEntryUtils from '../../../../../src/core/state/utils/nodeEntry.js';
import deploymentEntryUtils from '../../../../../src/core/state/utils/deploymentEntry.js';
import transactionUtils from '../../../../../src/core/state/utils/transaction.js';
import addressUtils from '../../../../../src/core/state/utils/address.js';
import { toBalance } from '../../../../../src/core/state/utils/balance.js';
import { EntryType } from '../../../../../src/utils/constants.js';
import { decimalStringToBigInt, bigIntTo16ByteBuffer } from '../../../../../src/utils/amountSerialization.js';
import { safeDecodeApplyOperation, safeEncodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import { config } from '../../../../helpers/config.js';

const DEFAULT_FUNDING = bigIntTo16ByteBuffer(decimalStringToBigInt('10'));

export function selectBootstrapValidatorPeer(context, offset = 0) {
	const peers = context.peers.slice(1);
	if (!peers.length) {
		throw new Error('Bootstrap deployment scenarios require a validator peer.');
	}
	return peers[Math.min(offset, peers.length - 1)];
}

export function selectBootstrapDeployerPeer(context, offset = 1) {
	const peers = context.peers.slice(1);
	if (peers.length < 2) {
		throw new Error('Bootstrap deployment scenarios require a deployer peer.');
	}
	return peers[Math.min(offset, peers.length - 1)];
}

export async function setupBootstrapDeploymentScenario(
	t,
	{
		nodes = 3,
		validatorInitialBalance = DEFAULT_FUNDING,
		deployerInitialBalance = DEFAULT_FUNDING,
		externalBootstrap = null,
		channel = null
	} = {}
) {
	const context = await setupAdminNetwork(t, { nodes: Math.max(nodes, 3) });
	const validatorPeer = selectBootstrapValidatorPeer(context);
	const deployerPeer = selectBootstrapDeployerPeer(context);

	context.addWriterScenario = { writerInitialBalance: validatorInitialBalance };

	await initializeBalances(context, [
		[validatorPeer.wallet.address, validatorInitialBalance],
		[deployerPeer.wallet.address, deployerInitialBalance]
	]);

	await whitelistAddress(context, validatorPeer.wallet.address);
	await whitelistAddress(context, deployerPeer.wallet.address);

	await promotePeerToWriter(t, context, { readerPeer: validatorPeer });
	await context.sync();

	const txValidity = await deriveIndexerSequenceState(validatorPeer.base);
	const resolvedExternalBootstrap = externalBootstrap ?? b4a.from(deployerPeer.base.local.key);
	const resolvedChannel = channel ?? b4a.from(validatorPeer.base.local.key);

	const validatorEntryBefore = await validatorPeer.base.view.get(validatorPeer.wallet.address);
	const deployerEntryBefore = await validatorPeer.base.view.get(deployerPeer.wallet.address);

	context.bootstrapDeployment = {
		validatorPeer,
		deployerPeer,
		externalBootstrap: resolvedExternalBootstrap,
		channel: resolvedChannel,
		txValidity,
		validatorEntryBefore: validatorEntryBefore
			? { value: b4a.from(validatorEntryBefore.value) }
			: null,
		deployerEntryBefore: deployerEntryBefore ? { value: b4a.from(deployerEntryBefore.value) } : null
	};

	return context;
}

export async function buildBootstrapDeploymentPayload(context, options = {}) {
	const validatorPeer =
		options.validatorPeer ??
		context.bootstrapDeployment?.validatorPeer ??
		selectBootstrapValidatorPeer(context);
	const deployerPeer =
		options.deployerPeer ??
		context.bootstrapDeployment?.deployerPeer ??
		selectBootstrapDeployerPeer(context);

	const externalBootstrap =
		options.externalBootstrap ??
		context.bootstrapDeployment?.externalBootstrap ??
		b4a.from(deployerPeer.base.local.key);
	const channel =
		options.channel ?? context.bootstrapDeployment?.channel ?? b4a.from(validatorPeer.base.local.key);
	const txValidity =
		options.txValidity ??
		context.bootstrapDeployment?.txValidity ??
		(await deriveIndexerSequenceState(validatorPeer.base));

    const partial = await applyStateMessageFactory(deployerPeer.wallet, config)
        .buildPartialBootstrapDeploymentMessage(
            deployerPeer.wallet.address,
            externalBootstrap.toString('hex'),
            channel.toString('hex'),
            txValidity.toString('hex'),
            'json'
        );

    const payload = await applyStateMessageFactory(validatorPeer.wallet, config)
        .buildCompleteBootstrapDeploymentMessage(
		partial.address,
            b4a.from(partial.bdo.tx, 'hex'),
            b4a.from(partial.bdo.txv, 'hex'),
            b4a.from(partial.bdo.bs, 'hex'),
            b4a.from(partial.bdo.ic, 'hex'),
            b4a.from(partial.bdo.in, 'hex'),
            b4a.from(partial.bdo.is, 'hex')
        );
    return safeEncodeApplyOperation(payload);
}

export async function buildBootstrapDeploymentPayloadWithTxValidity(context, txValidity, options = {}) {
	return buildBootstrapDeploymentPayload(context, {
		...options,
		txValidity
	});
}

export async function assertBootstrapDeploymentSuccessState(
	t,
	context,
	{
		payload = null,
		validatorPeer = context.bootstrapDeployment?.validatorPeer ?? selectBootstrapValidatorPeer(context),
		deployerPeer = context.bootstrapDeployment?.deployerPeer ?? selectBootstrapDeployerPeer(context),
		externalBootstrap = context.bootstrapDeployment?.externalBootstrap ?? null,
		channel = context.bootstrapDeployment?.channel ?? null,
		validatorEntryBefore = context.bootstrapDeployment?.validatorEntryBefore?.value ?? null,
		deployerEntryBefore = context.bootstrapDeployment?.deployerEntryBefore?.value ?? null,
		txValidity = context.bootstrapDeployment?.txValidity ?? null,
		skipSync = false
	} = {}
) {
	if (!payload) throw new Error('assertBootstrapDeploymentSuccessState requires the processed payload.');
	if (!validatorEntryBefore) {
		throw new Error('assertBootstrapDeploymentSuccessState requires the validator entry before execution.');
	}
	if (!deployerEntryBefore) {
		throw new Error('assertBootstrapDeploymentSuccessState requires the deployer entry before execution.');
	}

	const bootstrapBuffer = externalBootstrap ?? b4a.from(deployerPeer.base.local.key);
	const channelBuffer = channel ?? b4a.from(validatorPeer.base.local.key);
	const decodedOperation = safeDecodeApplyOperation(payload);
	t.ok(decodedOperation, 'bootstrapDeployment payload decodes');

	const requesterAddressBuffer = decodedOperation?.address;
	const validatorAddressBuffer = decodedOperation?.bdo?.va;
	const txHashBuffer = decodedOperation?.bdo?.tx;
	const txValidityBuffer = decodedOperation?.bdo?.txv;
	const channelFromPayload = decodedOperation?.bdo?.ic;
	const bootstrapFromPayload = decodedOperation?.bdo?.bs;

	t.ok(requesterAddressBuffer, 'payload carries requester address');
	t.ok(validatorAddressBuffer, 'payload carries validator address');
	t.ok(txHashBuffer, 'payload exposes tx hash');

	const requesterAddress = addressUtils.bufferToAddress(requesterAddressBuffer, config.addressPrefix);
	const validatorAddress = addressUtils.bufferToAddress(validatorAddressBuffer, config.addressPrefix);

	if (requesterAddress) {
		t.is(requesterAddress, deployerPeer.wallet.address, 'payload signed by expected requester');
	}
	if (validatorAddress) {
		t.is(validatorAddress, validatorPeer.wallet.address, 'payload signed by expected validator');
	}

	if (bootstrapFromPayload) {
		t.ok(b4a.equals(bootstrapFromPayload, bootstrapBuffer), 'payload carries external bootstrap');
	}
	if (channelFromPayload) {
		t.ok(b4a.equals(channelFromPayload, channelBuffer), 'payload carries expected channel');
	}
	if (txValidity && txValidityBuffer) {
		t.ok(b4a.equals(txValidityBuffer, txValidity), 'payload tx validity matches current indexer state');
	}

	const decodedValidatorBefore = nodeEntryUtils.decode(validatorEntryBefore);
	t.ok(decodedValidatorBefore, 'validator entry before bootstrapDeployment decodes');
	if (!decodedValidatorBefore) return;

	await assertRequesterPaidFee(t, validatorPeer.base, deployerPeer.wallet.address, deployerEntryBefore);
	await assertValidatorReward(t, validatorPeer, b4a.from(decodedValidatorBefore.balance));
	await assertDeploymentEntry(t, validatorPeer.base, {
		bootstrapBuffer,
		txHashBuffer,
		requesterAddressBuffer
	});

	if (!skipSync) {
		await context.sync();
		await assertRequesterPaidFee(t, deployerPeer.base, deployerPeer.wallet.address, deployerEntryBefore);
		await assertDeploymentEntry(t, deployerPeer.base, {
			bootstrapBuffer,
			txHashBuffer,
			requesterAddressBuffer
		});
	}
}

export async function assertBootstrapDeploymentFailureState(
	t,
	context,
	{ payload, bootstrapBufferOverride = null, skipValidatorEquality = false } = {}
) {
	if (!payload) throw new Error('assertBootstrapDeploymentFailureState requires a payload.');

	const decoded = safeDecodeApplyOperation(payload);
	t.ok(decoded, 'invalid payload decodes for assertions');
	if (!decoded) return;

	const validatorPeer = context.bootstrapDeployment?.validatorPeer ?? context.peers?.[1];
	const deployerPeer = context.bootstrapDeployment?.deployerPeer ?? context.peers?.[2];
	const validatorEntryBefore = context.bootstrapDeployment?.validatorEntryBefore?.value ?? null;
	const deployerEntryBefore = context.bootstrapDeployment?.deployerEntryBefore?.value ?? null;

	if (validatorPeer) {
		const after = await validatorPeer.base.view.get(validatorPeer.wallet.address);
		t.ok(after, 'validator entry still exists after rejection');
		if (!skipValidatorEquality && validatorEntryBefore && after?.value) {
			t.ok(b4a.equals(after.value, validatorEntryBefore), 'validator entry remains unchanged');
		}
	}

	if (deployerPeer) {
		const after = await validatorPeer.base.view.get(deployerPeer.wallet.address);
		t.ok(after, 'deployer entry still exists after rejection');
		if (deployerEntryBefore && after?.value) {
			const beforeDecoded = nodeEntryUtils.decode(deployerEntryBefore);
			const afterDecoded = nodeEntryUtils.decode(after.value);
			t.ok(beforeDecoded, 'deployer entry before decodes');
			t.ok(afterDecoded, 'deployer entry after decodes');
			if (beforeDecoded && afterDecoded) {
				t.ok(b4a.equals(afterDecoded.balance, beforeDecoded.balance), 'deployer balance unchanged');
				t.ok(
					b4a.equals(afterDecoded.stakedBalance, beforeDecoded.stakedBalance),
					'deployer stake unchanged'
				);
			}
		}
	}

	const bootstrapBuffer = bootstrapBufferOverride ?? decoded?.bdo?.bs ?? context.bootstrapDeployment?.externalBootstrap;
	if (bootstrapBuffer && validatorPeer) {
		const deploymentKey = `${EntryType.DEPLOYMENT}${bootstrapBuffer.toString('hex')}`;
		const deploymentEntry = await validatorPeer.base.view.get(deploymentKey);
		t.is(deploymentEntry, null, 'deployment entry not created after rejection');
	}

	const txHashBuffer = decoded?.bdo?.tx;
	if (txHashBuffer && validatorPeer) {
		const txEntry = await validatorPeer.base.view.get(txHashBuffer.toString('hex'));
		t.is(txEntry, null, 'tx hash not recorded after rejection');
	}
}

export function mutateToNetworkBootstrap(t, payload, context) {
	const decoded = safeDecodeApplyOperation(payload);
	t.ok(decoded, 'valid payload decodes before mutation');
	if (!decoded?.bdo) return payload;
	const bootstrapKey = context.bootstrap?.base?.local?.key;
	t.ok(bootstrapKey, 'network bootstrap key available');
	if (!bootstrapKey) return payload;
	decoded.bdo.bs = bootstrapKey;
	return safeEncodeApplyOperation(decoded);
}

export async function appendInvalidPayload(context, invalidPayload) {
	const node = context.bootstrap ?? context.adminBootstrap ?? context.peers?.[0];
	await node.base.append(invalidPayload);
	await node.base.update();
}

async function assertRequesterPaidFee(t, base, address, entryBeforeValue) {
	const beforeEntry = nodeEntryUtils.decode(entryBeforeValue);
	t.ok(beforeEntry, 'deployer entry before bootstrapDeployment decodes');
	if (!beforeEntry) return;

	const beforeBalance = toBalance(beforeEntry.balance);
	t.ok(beforeBalance, 'deployer balance before bootstrapDeployment decodes');
	if (!beforeBalance) return;

	const expectedBalance = beforeBalance.sub(toBalance(transactionUtils.FEE));
	t.ok(expectedBalance, 'deployer balance after fee computation succeeds');
	if (!expectedBalance) return;

	const entry = await base.view.get(address);
	t.ok(entry, 'deployer entry exists after bootstrapDeployment');
	const decoded = nodeEntryUtils.decode(entry?.value);
	t.ok(decoded, 'deployer entry decodes after bootstrapDeployment');
	if (!decoded) return;

	t.is(decoded.isWhitelisted, true, 'deployer remains whitelisted');
	t.is(decoded.isWriter, false, 'deployer not promoted to writer');
	t.is(decoded.isIndexer, false, 'deployer not promoted to indexer');
	t.ok(
		b4a.equals(decoded.balance, expectedBalance.value),
		'deployer balance reduced by bootstrapDeployment fee'
	);
}

async function assertDeploymentEntry(
	t,
	base,
	{ bootstrapBuffer, txHashBuffer, requesterAddressBuffer }
) {
	if (!txHashBuffer) {
		t.fail('bootstrapDeployment tx hash missing');
		return;
	}
	const deploymentKey = `${EntryType.DEPLOYMENT}${bootstrapBuffer.toString('hex')}`;
	const deploymentEntry = await base.view.get(deploymentKey);
	t.ok(deploymentEntry, 'deployment entry stored');
	const decodedDeployment = deploymentEntryUtils.decode(deploymentEntry?.value, config.addressLength);
	t.ok(decodedDeployment?.txHash, 'deployment entry decodes');
	if (!decodedDeployment || !decodedDeployment.txHash) return;

	t.ok(b4a.equals(decodedDeployment.txHash, txHashBuffer), 'deployment entry stores tx hash');
	t.ok(
		b4a.equals(decodedDeployment.address, requesterAddressBuffer),
		'deployment entry binds bootstrap to requester address'
	);

	const txEntry = await base.view.get(txHashBuffer.toString('hex'));
	t.ok(txEntry, 'bootstrapDeployment transaction recorded for replay protection');
}

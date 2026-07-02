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
import nodeEntryUtils, { ZERO_LICENSE } from '../../../../../src/core/state/utils/nodeEntry.js';
import lengthEntryUtils from '../../../../../src/core/state/utils/lengthEntry.js';
import addressUtils from '../../../../../src/core/state/utils/address.js';
import { buildAddAdminRequesterPayload } from '../addAdmin/addAdminScenarioHelpers.js';
import { config } from '../../../../helpers/config.js';

export async function setupAppendWhitelistScenario(t, { nodes = 2 } = {}) {
	const context = await setupStateNetwork({
		nodes: Math.max(nodes, 2),
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

export function selectReaderPeer(context, offset = 0) {
	const readerPeers = context.peers.slice(1);
	if (readerPeers.length === 0) {
		throw new Error('Append whitelist scenarios require at least one reader node.');
	}
	return readerPeers[Math.min(offset, readerPeers.length - 1)];
}


export async function buildAppendWhitelistPayload(context, readerAddress = null) {
	const adminNode = context.adminBootstrap;
	const targetAddress = readerAddress ?? selectReaderPeer(context).wallet.address;
	const txValidity = await deriveIndexerSequenceState(adminNode.base);
	return safeEncodeApplyOperation(
		await applyStateMessageFactory(adminNode.wallet, config)
			.buildCompleteAppendWhitelistMessage(adminNode.wallet.address, targetAddress, txValidity)
	);
}

export async function buildAppendWhitelistPayloadWithTxValidity(
	context,
	txValidity,
	readerAddress = null
) {
	const adminNode = context.adminBootstrap;
	const targetAddress = readerAddress ?? selectReaderPeer(context).wallet.address;
	return safeEncodeApplyOperation(
		await applyStateMessageFactory(adminNode.wallet, config)
			.buildCompleteAppendWhitelistMessage(adminNode.wallet.address, targetAddress, txValidity)
	);
}

export async function buildBanWriterPayload(context, readerAddress) {
	const adminNode = context.adminBootstrap;
	const txValidity = await deriveIndexerSequenceState(adminNode.base);
	return safeEncodeApplyOperation(
		await applyStateMessageFactory(adminNode.wallet, config)
			.buildCompleteBanWriterMessage(adminNode.wallet.address, readerAddress, txValidity)
	);
}

export function mutateAppendWhitelistPayloadForInvalidSchema(t, validPayload) {
	const decodedPayload = safeDecodeApplyOperation(validPayload);
	t.ok(decodedPayload, 'fixtures decode');
	decodedPayload.aco.tx = b4a.alloc(decodedPayload.aco.tx.length);
	return safeEncodeApplyOperation(decodedPayload);
}

export async function assertAppendWhitelistFailureState(
	t,
	context,
	{ skipSync = false, readerAddress = null, expectedLicenseCount = 1 } = {}
) {
	const adminNode = context.adminBootstrap;
	const targetAddress = readerAddress ?? selectReaderPeer(context).wallet.address;

	await assertReaderAbsent(t, adminNode.base, targetAddress);

	if (typeof expectedLicenseCount === 'number') {
		await assertLicenseCount(t, adminNode.base, expectedLicenseCount);
	}

	if (!skipSync) {
		await context.sync();
		for (const peer of context.peers) {
			await assertReaderAbsent(t, peer.base, targetAddress);
		}
	}
}

export async function assertAppendWhitelistSuccessState(
	t,
	context,
	{ readerAddress = null, expectedLicenseCount = 2, skipSync = false } = {}
) {
	const adminNode = context.adminBootstrap;
	const targetAddress = readerAddress ?? selectReaderPeer(context).wallet.address;

	await assertReaderWhitelisted(t, adminNode.base, targetAddress, {
		expectedLicenseCount
	});

	if (!skipSync) {
		await context.sync();
		const peer = findPeerByAddress(context, targetAddress);
		if (peer) {
			await assertReaderWhitelisted(t, peer.base, targetAddress, {
				expectedLicenseCount
			});
		}
	}
}

export async function assertReaderWhitelisted(
	t,
	base,
	readerAddress,
	{ expectedLicenseCount } = {}
) {
	const nodeEntryRecord = await base.view.get(readerAddress);
	t.ok(nodeEntryRecord, 'reader node entry exists');

	const decodedEntry = nodeEntryUtils.decode(nodeEntryRecord.value);
	t.ok(decodedEntry, 'reader node entry decodes');
	t.is(decodedEntry.isWhitelisted, true, 'reader flagged as whitelisted');
	t.is(decodedEntry.isWriter, false, 'reader not flagged as writer');
	t.is(decodedEntry.isIndexer, false, 'reader not flagged as indexer');
	t.ok(!b4a.equals(decodedEntry.license, ZERO_LICENSE), 'reader license assigned');

	const licenseId = lengthEntryUtils.decodeBE(decodedEntry.license);
	const licenseIndexEntry = await base.view.get(`${EntryType.LICENSE_INDEX}${licenseId}`);
	t.ok(licenseIndexEntry, 'license index entry exists for reader');
	const readerAddressBuffer = addressUtils.addressToBuffer(readerAddress, config.addressPrefix);
	t.ok(readerAddressBuffer.length > 0, 'reader address encodes to buffer');
	t.ok(
		licenseIndexEntry && b4a.equals(licenseIndexEntry.value, readerAddressBuffer),
		'license index stores reader address'
	);

	if (typeof expectedLicenseCount === 'number') {
		await assertLicenseCount(t, base, expectedLicenseCount);
	}

	return { licenseId, decodedEntry };
}

async function bootstrapAdmin(context) {
	const adminNode = context.adminBootstrap;
	const payload = await buildAddAdminRequesterPayload(context);

	await adminNode.base.append(payload);
	await adminNode.base.update();
	await eventFlush();
}

async function assertReaderAbsent(t, base, address) {
	const nodeEntryRecord = await base.view.get(address);
	t.is(nodeEntryRecord, null, 'reader node entry remains absent');
}

function findPeerByAddress(context, address) {
	return context.peers.find(peer => peer.wallet?.address === address) ?? null;
}

async function assertLicenseCount(t, base, expected) {
	const licenseCountEntry = await base.view.get(EntryType.LICENSE_COUNT);
	t.ok(licenseCountEntry, 'license count entry exists');
	const licenseCount = lengthEntryUtils.decodeBE(licenseCountEntry.value);
	t.is(licenseCount, expected, 'license count matches expected value');
}

export default setupAppendWhitelistScenario;
export {
	assertReaderAbsent
};

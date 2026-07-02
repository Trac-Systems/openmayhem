import { setupStateNetwork } from '../../../../helpers/StateNetworkFactory.js';
import {
	seedBootstrapIndexer,
	defaultOpenHyperbeeView,
	deriveIndexerSequenceState,
	eventFlush
} from '../../../../helpers/autobaseTestHelpers.js';
import { applyStateMessageFactory } from '../../../../../src/messages/state/applyStateMessageFactory.js';
import { safeEncodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import { AUTOBASE_VALUE_ENCODING } from '../../../../../src/utils/constants.js';
import { buildAddAdminRequesterPayload } from '../addAdmin/addAdminScenarioHelpers.js';
import { config } from '../../../../helpers/config.js';

/**
 * Boots a network with an initialized admin node and returns the shared context.
 */
export async function setupAdminNetwork(t, { nodes = 2 } = {}) {
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

/**
 * Ensures the first reader peer receives funds and becomes whitelisted.
 */
export async function setupAdminAndWhitelistedReaderNetwork(
	t,
	{ nodes = 2, readerInitialBalance = null } = {}
) {
	const context = await setupAdminNetwork(t, { nodes });
	const readerPeers = context.peers.slice(1);
	const firstReader = readerPeers[0];
	if (!firstReader) return context;

	if (readerInitialBalance) {
		await initializeBalances(context, [[firstReader.wallet.address, readerInitialBalance]]);
	}

	await whitelistAddress(context, firstReader.wallet.address);
	return context;
}

/**
 * Appends balance initialization payloads for provided pairs.
 * @param {Array<[string, Buffer]>} recipients - address/balance tuples.
 */
export async function initializeBalances(context, recipients) {
	const adminNode = context.adminBootstrap;
	if (!adminNode || !Array.isArray(recipients) || recipients.length === 0) return;

	const txValidity = await deriveIndexerSequenceState(adminNode.base);
	for (const [recipientAddress, balanceBuffer] of recipients) {
		const payload = await applyStateMessageFactory(adminNode.wallet, config)
			.buildCompleteBalanceInitializationMessage(
				adminNode.wallet.address,
				recipientAddress,
				balanceBuffer,
				txValidity
			);
		const encoded = safeEncodeApplyOperation(payload);
		await adminNode.base.append(encoded);
		await adminNode.base.update();
		await eventFlush();
	}
}

export async function whitelistAddress(context, address) {
	const adminNode = context.adminBootstrap;
	if (!adminNode || !address) return;

	const txValidity = await deriveIndexerSequenceState(adminNode.base);
	const payload = await applyStateMessageFactory(adminNode.wallet, config)
		.buildCompleteAppendWhitelistMessage(adminNode.wallet.address, address, txValidity);
	await adminNode.base.append(safeEncodeApplyOperation(payload));
	await adminNode.base.update();
	await eventFlush();
}

async function bootstrapAdmin(context) {
	const adminNode = context.adminBootstrap;
	const payload = await buildAddAdminRequesterPayload(context);
	await adminNode.base.append(payload);
	await adminNode.base.update();
	await eventFlush();
}

export default {
	setupAdminNetwork,
	setupAdminAndWhitelistedReaderNetwork,
	initializeBalances,
	whitelistAddress
};

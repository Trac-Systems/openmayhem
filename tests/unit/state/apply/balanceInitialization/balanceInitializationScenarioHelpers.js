import b4a from 'b4a';
import { setupStateNetwork } from '../../../../helpers/StateNetworkFactory.js';
import {
	seedBootstrapIndexer,
	defaultOpenHyperbeeView,
	deriveIndexerSequenceState,
	eventFlush
} from '../../../../helpers/autobaseTestHelpers.js';
import { applyStateMessageFactory } from '../../../../../src/messages/state/applyStateMessageFactory.js';
import { AUTOBASE_VALUE_ENCODING } from '../../../../../src/utils/constants.js';
import { toTerm } from '../../../../../src/core/state/utils/balance.js';
import { safeDecodeApplyOperation, safeEncodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import { buildAddAdminRequesterPayload } from '../addAdmin/addAdminScenarioHelpers.js';
import { config } from '../../../../helpers/config.js';

export async function setupBalanceInitializationScenario(t, { recipientCount = 2 } = {}) {
	const context = await setupStateNetwork({
		nodes: Math.max(recipientCount + 1, 2),
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

async function bootstrapAdmin(context) {
	const adminNode = context.adminBootstrap;
	const addAdminPayload = await buildAddAdminRequesterPayload(context);

	await adminNode.base.append(addAdminPayload);
	await adminNode.base.update();
	await eventFlush();
}

export async function buildBalanceInitializationPayload(context, recipientAddress, balanceBuffer) {
	const adminNode = context.adminBootstrap;
	const txValidity = await deriveIndexerSequenceState(adminNode.base);
	const payload = await applyStateMessageFactory(adminNode.wallet, config)
		.buildCompleteBalanceInitializationMessage(
			adminNode.wallet.address,
			recipientAddress,
			balanceBuffer,
			txValidity
		);
	return safeEncodeApplyOperation(payload);
}

export async function buildBalanceInitializationPayloadWithTxValidity({
	context,
	validPayload,
	mutatedTxValidity
}) {
	const decoded = safeDecodeApplyOperation(validPayload);
	if (!decoded?.bio?.ia || !decoded?.bio?.am) {
		return validPayload;
	}

	const adminNode = context.adminBootstrap;
	const payload = await applyStateMessageFactory(adminNode.wallet, config)
		.buildCompleteBalanceInitializationMessage(
			adminNode.wallet.address,
			decoded.bio.ia,
			decoded.bio.am,
			mutatedTxValidity
		);
	return safeEncodeApplyOperation(payload);
}

export async function buildDefaultBalanceInitializationPayload(context) {
	const recipientPeer = selectRecipientPeer(context);
	return buildBalanceInitializationPayload(context, recipientPeer.wallet.address, toTerm(25n));
}

export async function assertBalanceInitializationFailureState(t, context, { skipSync = false } = {}) {
	const adminNode = context.adminBootstrap;
	const recipientPeer = selectRecipientPeer(context);

	await assertRecipientAbsent(t, adminNode.base, recipientPeer.wallet.address);

	if (!skipSync) {
		await context.sync();
		await assertRecipientAbsent(t, recipientPeer.base, recipientPeer.wallet.address);
	}
}

export function mutateBalanceInitializationPayloadForInvalidSchema(t, payload) {
	const decodedPayload = safeDecodeApplyOperation(payload);
	t.ok(decodedPayload, 'fixtures decode');

	decodedPayload.bio.tx = b4a.alloc(decodedPayload.bio.tx.length);
	return safeEncodeApplyOperation(decodedPayload);
}

function selectRecipientPeer(context, offset = 0) {
	const readerPeers = context.peers.slice(1);
	return readerPeers[Math.min(offset, readerPeers.length - 1)];
}

async function assertRecipientAbsent(t, base, address) {
	const nodeEntryRecord = await base.view.get(address);
	t.is(nodeEntryRecord, null, 'recipient node entry remains absent');
}

export default setupBalanceInitializationScenario;

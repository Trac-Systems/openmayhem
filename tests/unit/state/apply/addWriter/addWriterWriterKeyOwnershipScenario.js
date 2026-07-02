import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import {
	setupAddWriterScenario,
	buildAddWriterPayload,
	assertAddWriterFailureState,
	selectWriterPeer,
	defaultWriterFunding
} from './addWriterScenarioHelpers.js';
import { initializeBalances, whitelistAddress } from '../common/commonScenarioHelper.js';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';

async function setupScenarioWithForeignKey(t) {
	const context = await setupAddWriterScenario(t, { nodes: 3 });
	const foreignPeer = selectWriterPeer(context, 1);
	await fundAndWhitelistPeer(context, foreignPeer);
	await promotePeerToWriter(context, foreignPeer);
	context.foreignWriterKey = foreignPeer.base.local.key;
	return context;
}

async function fundAndWhitelistPeer(context, peer) {
	const funding = context.addWriterScenario?.writerInitialBalance ?? defaultWriterFunding;
	await initializeBalances(context, [[peer.wallet.address, funding]]);
	await whitelistAddress(context, peer.wallet.address);
}

async function promotePeerToWriter(context, peer) {
	const payload = await buildAddWriterPayload(context, { readerPeer: peer });
	await appendPayload(context, payload);
}

async function appendPayload(context, payload) {
	const node = context.adminBootstrap ?? context.bootstrap;
	if (!node?.base) {
		throw new Error('Writer key ownership scenario requires an admin node with a writable base.');
	}
	await node.base.append(payload);
	await node.base.update();
	await eventFlush();
	await context.sync();
}

export default function addWriterWriterKeyOwnershipScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply addWriter rejects payloads when writer key belongs to another node',
		setupScenario: setupScenarioWithForeignKey,
		buildValidPayload: context =>
			buildAddWriterPayload(context, { writerKeyBuffer: context.foreignWriterKey }),
		mutatePayload: (_t, payload) => payload,
		applyInvalidPayload: appendPayload,
		assertStateUnchanged: assertAddWriterFailureState,
		expectedLogs: ['Invalid writer key: either not owned by requester or different from assigned key.']
	}).performScenario();
}

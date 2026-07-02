import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import {
	setupAddWriterScenario,
	buildAddWriterPayload,
	buildRemoveWriterPayload,
	assertAddWriterFailureState,
	selectWriterPeer,
	applyWithRequesterWriterKeyMismatch
} from './addWriterScenarioHelpers.js';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';

async function setupScenarioWithRegisteredKey(t) {
	const context = await setupAddWriterScenario(t);
	const writerPeer = selectWriterPeer(context);
	await promotePeerToWriter(context, writerPeer);
	context.registeredWriterKey = writerPeer.base.local.key;
	await demoteWriterPeer(context, writerPeer);
	return context;
}

async function promotePeerToWriter(context, peer) {
	const payload = await buildAddWriterPayload(context, { readerPeer: peer });
	await appendPayload(context, payload);
}

async function demoteWriterPeer(context, peer) {
	const payload = await buildRemoveWriterPayload(context, { readerPeer: peer });
	await appendPayload(context, payload);
}

async function appendPayload(context, payload) {
	const node = context.adminBootstrap ?? context.bootstrap;
	if (!node?.base) {
		throw new Error('Writer key mismatch scenario requires an admin node with a writable base.');
	}
	await node.base.append(payload);
	await node.base.update();
	await eventFlush();
	await context.sync();
}

export default function addWriterWriterKeyMismatchScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply addWriter rejects payloads when node entry writing key mismatches registered key',
		setupScenario: setupScenarioWithRegisteredKey,
		buildValidPayload: context =>
			buildAddWriterPayload(context, { writerKeyBuffer: context.registeredWriterKey }),
		mutatePayload: (_t, payload) => payload,
		applyInvalidPayload: applyWithRequesterWriterKeyMismatch,
		assertStateUnchanged: (t, context) =>
			assertAddWriterFailureState(t, context, { skipSync: true, expectRegistryEntry: true }),
		expectedLogs: ['Invalid writer key: either not owned by requester or different from assigned key.']
	}).performScenario();
}

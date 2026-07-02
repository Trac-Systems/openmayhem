import OperationValidationScenarioBase from '../base/OperationValidationScenarioBase.js';
import { eventFlush } from '../../../../../helpers/autobaseTestHelpers.js';

const passThroughPayload = (_t, payload) => payload;

export default class IndexerSequenceStateInvalidScenario extends OperationValidationScenarioBase {
	constructor({
		title,
		setupScenario,
		buildValidPayload,
		assertStateUnchanged,
		mutatePayload = passThroughPayload,
		applyInvalidPayload = applyWithIndexerSequenceFailure,
		expectedLogs
	}) {
		super({
			title,
			setupScenario,
			buildValidPayload,
			mutatePayload,
			applyInvalidPayload,
			assertStateUnchanged,
			expectedLogs
		});
	}
}

async function applyWithIndexerSequenceFailure(context, invalidPayload) {
	const node = context.bootstrap ?? context.adminBootstrap ?? context.peers?.[0];
	if (!node?.base?.system) {
		return;
	}

	const system = node.base.system;
	const originalDescriptor = Object.getOwnPropertyDescriptor(system, 'indexers');
	const originalValue = system.indexers;
	let injected = false;

	Object.defineProperty(system, 'indexers', {
		configurable: true,
		enumerable: true,
		get() {
			if (!injected) {
				injected = true;
				throw new Error('forced indexer sequence state failure');
			}
			return originalValue;
		},
		set(value) {
			// preserve setter semantics
			return Reflect.set(system, 'indexers', value);
		}
	});

	try {
		await node.base.append(invalidPayload).catch(() => {});
		await node.base.update().catch(() => {});
	} finally {
		if (originalDescriptor) {
			Object.defineProperty(system, 'indexers', originalDescriptor);
		} else {
			system.indexers = originalValue;
		}
		await eventFlush();
	}
}

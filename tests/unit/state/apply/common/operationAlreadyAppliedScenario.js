import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import OperationValidationScenarioBase from './base/OperationValidationScenarioBase.js';

export default class OperationAlreadyAppliedScenario extends OperationValidationScenarioBase {
	constructor({
		title,
		setupScenario,
		buildValidPayload,
		assertStateUnchanged,
		mutatePayload = passThroughPayload,
		selectNode = defaultSelectNode,
		applyInvalidPayload,
		beforeInvalidApply,
		expectedLogs
	}) {
		const applyPayload =
			applyInvalidPayload ??
			((context, invalidPayload, t, validPayload) =>
				defaultApplyInvalidPayload({
					context,
					invalidPayload,
					validPayload,
					selectNode,
					beforeInvalidApply,
					t
				}));

		super({
			title,
			setupScenario,
			buildValidPayload,
			mutatePayload,
			applyInvalidPayload: applyPayload,
			assertStateUnchanged,
			expectedLogs
		});
	}
}

function passThroughPayload(_t, payload) {
	return payload;
}

function defaultSelectNode(context) {
	return context.adminBootstrap ?? context.bootstrap ?? context.peers?.[0] ?? null;
}

async function defaultApplyInvalidPayload({
	context,
	invalidPayload,
	validPayload,
	selectNode,
	beforeInvalidApply,
	t
}) {
	const node = selectNode(context);
	if (!node?.base) {
		throw new Error('Operation already applied scenario requires a writable node.');
	}

	await node.base.append(validPayload);
	await node.base.update();
	await eventFlush();

	let cleanup;
	if (beforeInvalidApply) {
		cleanup = await beforeInvalidApply({
			context,
			node,
			invalidPayload,
			validPayload,
			t
		});
	}

	try {
		await node.base.append(invalidPayload);
		await node.base.update();
		await eventFlush();
	} finally {
		if (typeof cleanup === 'function') {
			await cleanup();
		}
	}
}

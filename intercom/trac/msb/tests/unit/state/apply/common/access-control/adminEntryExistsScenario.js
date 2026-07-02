import { eventFlush } from '../../../../../helpers/autobaseTestHelpers.js';
import OperationValidationScenarioBase from '../base/OperationValidationScenarioBase.js';

export default class AdminEntryExistsScenario extends OperationValidationScenarioBase {
	constructor({
		title,
		setupScenario,
		buildValidPayload,
		assertStateUnchanged,
		buildDuplicatePayload,
		selectInitialNode = defaultSelectNode,
		selectDuplicateNode,
		beforeDuplicateApply,
		applyInitialPayload = true,
		expectedLogs
	}) {
		const mutatePayload =
			buildDuplicatePayload
				? ((t, validPayload, context) => buildDuplicatePayload(context, t, validPayload))
				: ((_, validPayload) => validPayload);

		const applyInvalidPayload = createApplyInvalidPayload({
			selectInitialNode,
			selectDuplicateNode: selectDuplicateNode ?? selectInitialNode,
			beforeDuplicateApply,
			applyInitialPayload
		});

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

function createApplyInvalidPayload({
	selectInitialNode,
	selectDuplicateNode,
	beforeDuplicateApply,
	applyInitialPayload
}) {
	return async (context, invalidPayload, t, validPayload) => {
		const initialNode = selectInitialNode?.(context);
		const duplicateNode = selectDuplicateNode?.(context);

		if (applyInitialPayload && !initialNode?.base) {
			throw new Error('Admin entry exists scenario requires an initial node with a writable base.');
		}

		if (!duplicateNode?.base) {
			throw new Error('Admin entry exists scenario requires a duplicate node with a writable base.');
		}

		if (applyInitialPayload) {
			await applyPayload(initialNode, validPayload);
		}

		let cleanup;
		if (beforeDuplicateApply) {
			cleanup = await beforeDuplicateApply({
				context,
				t,
				initialPayload: validPayload,
				duplicatePayload: invalidPayload,
				initialNode,
				duplicateNode
			});
		}

		try {
			await applyPayload(duplicateNode, invalidPayload);
		} finally {
			if (typeof cleanup === 'function') {
				await cleanup();
			}
		}
	};
}

async function applyPayload(node, payload) {
	await node.base.append(payload);
	await node.base.update();
	await eventFlush();
}

function defaultSelectNode(context) {
	return context.adminBootstrap ?? context.bootstrap ?? context.peers?.[0] ?? null;
}

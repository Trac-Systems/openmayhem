import { eventFlush } from '../../../../../helpers/autobaseTestHelpers.js';
import OperationValidationScenarioBase from '../base/OperationValidationScenarioBase.js';

export default class AdminOnlyGuardScenario extends OperationValidationScenarioBase {
	constructor({
		title,
		setupScenario,
		buildValidPayload,
		assertStateUnchanged,
		selectNode = defaultSelectNode,
		selectReader = defaultSelectReader,
		beforeApply,
		expectedLogs
	}) {
			super({
				title,
				setupScenario,
				buildValidPayload,
				mutatePayload: passThroughPayload,
				applyInvalidPayload: createApplyInvalidPayload({
					selectNode,
					selectReader,
					beforeApply
				}),
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

function defaultSelectReader(context) {
	const readers = context.peers?.slice(1) ?? [];
	return readers[0] ?? null;
}

function createApplyInvalidPayload({ selectNode, selectReader, beforeApply }) {
	return async (context, payload, t, validPayload) => {
		const node = selectNode(context);
		const fakeNode = selectReader(context);

		if (!node?.base) {
			throw new Error('Admin-only guard scenario requires a writable node.');
		}

		if (!fakeNode?.base) {
			throw new Error('Admin-only guard scenario requires a fake reader node.');
		}

		const cleanupFns = [];

		if (beforeApply) {
			const cleanup = await beforeApply({
				context,
				t,
				payload,
				validPayload,
				node,
				impersonator: fakeNode
			});
			if (typeof cleanup === 'function') {
				cleanupFns.push(cleanup);
			}
		}

		cleanupFns.push(patchRequesterKey(node.base, fakeNode.base.local.key));

		try {
			await applyPayload(node, payload);
		} finally {
			while (cleanupFns.length) {
				const cleanup = cleanupFns.pop();
				await cleanup?.();
			}
		}
	};
}

function patchRequesterKey(base, impersonatorKey) {
	const originalApply = base._handlers.apply;
	let shouldPatchNextApply = true;

	base._handlers.apply = async (nodes, view, baseCtx) => {
		if (!shouldPatchNextApply) {
			return originalApply(nodes, view, baseCtx);
		}

		shouldPatchNextApply = false;
		const patchedNodes = nodes.map(node => {
			if (!node?.from) return node;
			return {
				...node,
				from: new Proxy(node.from, {
					// We only need to spoof node.from.key. Using a Proxy lets us override that single
					// property while delegating every other access back to the original object via Reflect.
					// This keeps the Hypercore/Autobase node shape intact without manual cloning.
					get(target, prop, receiver) {
						if (prop === 'key') {
							return impersonatorKey;
						}
						return Reflect.get(target, prop, receiver);
					}
				})
			};
		});

		return originalApply(patchedNodes, view, baseCtx);
	};

	return () => {
		base._handlers.apply = originalApply;
	};
}

async function applyPayload(node, payload) {
	await node.base.append(payload);
	await node.base.update();
	await eventFlush();
}

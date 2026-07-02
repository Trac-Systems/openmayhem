import { test } from 'brittle';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';

import {
	setupAddAdminScenario,
	buildAddAdminRequesterPayload,
	assertAddAdminRequesterFailureStateLocal
} from './addAdminScenarioHelpers.js';

/**
 * Verifies that ADD_ADMIN rejects payloads appended by any node other than the bootstrap writer.
 *
 * Only the bootstrap node has permission to run this operation. During the test we let the bootstrap
 * append a valid payload, but intercept the ensuing `State.apply` call and mutate `node.from.key`
 * so that the handler thinks the operation was produced by a reader node. That should trigger the
 * "Node is not a bootstrap node." guard, and the state must remain untouched.
 */
export default function addAdminNonBootstrapNodeScenario() {
	test('State.apply addAdmin rejects non-bootstrap requesters', async t => {
		const networkContext = await setupAddAdminScenario(t);
		const adminNode = networkContext.adminBootstrap;
		const readerNode = networkContext.peers[1];

		const originalApply = adminNode.base._handlers.apply;
		let shouldMutateNextCall = true;

		adminNode.base._handlers.apply = async (nodes, view, baseCtx) => {
			if (!shouldMutateNextCall) {
				return originalApply(nodes, view, baseCtx);
			}

			shouldMutateNextCall = false;
			adminNode.base._handlers.apply = originalApply;

			const readerKey = readerNode.base.local.key;
			const mutatedNodes = nodes.map(node => {
				if (!node?.from) return node;
				return {
					...node,
				from: new Proxy(node.from, {
						// We only need to fake node.from.key. A Proxy lets us override just that property
						// while Reflect delegates every other access to the original object, so the node
						// shape Autobase expects stays untouched.
						get(target, prop, receiver) {
							if (prop === 'key') {
								return readerKey;
							}
							return Reflect.get(target, prop, receiver);
						}
					})
				};
			});

			return originalApply(mutatedNodes, view, baseCtx);
		};

		t.teardown(() => {
			adminNode.base._handlers.apply = originalApply;
		});

		const payload = await buildAddAdminRequesterPayload(networkContext);
		await adminNode.base.append(payload);
		await adminNode.base.update();
		await eventFlush();

		await assertAddAdminRequesterFailureStateLocal(t, networkContext);
	});
}
